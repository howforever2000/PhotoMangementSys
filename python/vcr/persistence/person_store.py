"""持久层：人物注册表（SQLite）

表结构：
  persons(id TEXT PK, name TEXT, centroid BLOB, face_count INT, created_at TEXT)
    centroid: 512 维归一化均值向量（float32 小端，ArcFace 嵌入）
  faces(id INTEGER PK AUTOINCREMENT, person_id TEXT, photo_path TEXT,
        bbox TEXT, embedding BLOB, created_at TEXT)
  meta(key TEXT PK, value TEXT)
    emb_model：登记本库嵌入向量所用的 ArcFace 模型文件名。**换模型即换向量
    空间**：register() 发现不匹配会直接报错（而不是静默把新向量与旧质心
    余弦后随机归簇/全部新建），由宿主引导用户重建人物库。

职责：
  - match(embedding) → 与各 person 质心做余弦相似度，≥FACE_SIM 返回最相近者
  - register(embedding, photo, bbox) → 命中则并入，否则新建 P 编号
  - emb_model_info() → 宿主扫描前预检（registered vs active）
  - rebuild() → 备份后清空全库（换模型/修复错乱后的显式动作）
  - merge/rename/list/delete 供前端管理人物
仅 Python 侧使用，与 Rust 相册库完全解耦（独立 persons.db）。
"""
import os
import re
import sqlite3
import time

import numpy as np

from .. import config

EMB_DIM = 128

# face_overrides.person_id 的哨兵值：用户明确「这个人脸不属于任何人物」
NO_PERSON = "__none__"


class PersonStore:
    def __init__(self, db_path: str = config.PERSONS_DB):
        self.db_path = db_path
        os.makedirs(os.path.dirname(db_path), exist_ok=True)
        self._init_schema()

    def _conn(self) -> sqlite3.Connection:
        conn = sqlite3.connect(self.db_path)
        conn.row_factory = sqlite3.Row
        return conn

    def _init_schema(self):
        with self._conn() as conn:
            conn.execute(
                """CREATE TABLE IF NOT EXISTS persons (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    centroid BLOB NOT NULL,
                    face_count INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL)"""
            )
            conn.execute(
                """CREATE TABLE IF NOT EXISTS faces (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    person_id TEXT NOT NULL,
                    photo_path TEXT NOT NULL,
                    bbox TEXT NOT NULL,
                    embedding BLOB NOT NULL,
                    created_at TEXT NOT NULL)"""
            )
            conn.execute(
                """CREATE TABLE IF NOT EXISTS meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL)"""
            )
            # 人工修正覆盖表：用户把某张脸「移到别的人物 / 移出所有人物」后，
            # 记录 (照片, 人脸框) → 目标人物，扫描时优先于质心匹配。
            # person_id 为 NO_PERSON 表示「明确不属于任何人」。
            # 为什么必须单开一张表：faces 表在重扫时是「再插一行」的，
            # 光改 faces 的 person_id 挡不住下次扫描按质心重新认错。
            conn.execute(
                """CREATE TABLE IF NOT EXISTS face_overrides (
                    photo_path TEXT NOT NULL,
                    bbox TEXT NOT NULL,
                    person_id TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (photo_path, bbox))"""
            )
            # 旧库迁移：历史库没有 meta.emb_model，但全部是 w600k_mbf 时代的向量。
            # 补记真实模型，让「换到 r50」的预检能正确报错，而不是把新旧向量
            # 当同一空间静默混聚。库为空（全新安装）则不记，首次 register 时写。
            n = conn.execute("SELECT COUNT(*) FROM persons").fetchone()[0]
            has_meta = conn.execute(
                "SELECT 1 FROM meta WHERE key='emb_model'"
            ).fetchone()
            if n > 0 and not has_meta:
                conn.execute(
                    "INSERT INTO meta(key, value) VALUES('emb_model', 'w600k_mbf.onnx')"
                )

    # ------------------------------------------------------------------
    @staticmethod
    def _to_blob(emb: np.ndarray) -> bytes:
        return np.asarray(emb, dtype=np.float32).tobytes()

    @staticmethod
    def _from_blob(blob: bytes) -> np.ndarray:
        return np.frombuffer(blob, dtype=np.float32).copy()

    @staticmethod
    def _active_rec_model() -> str | None:
        """当前实际会加载的识别模型（当前档位的文件真实存在才返回；都没有 → None）。

        ！不要回落到“候选列表的第一个名字”：（BUG-2026-1001-016）模型文件缺失时
        报出一个并不存在的档位，宿主预检会误判“模型已切换/一致”，而人脸通道其实是
        静默降级（person_ids 全空）—— 用户看到的只是“扫完没人物”。
        """
        for m in config.face_rec_models():
            if os.path.isfile(os.path.join(config.MODEL_DIR, m)):
                return m
        return None

    @staticmethod
    def missing_face_models() -> list[str]:
        """缺失的人脸模型文件名（当前档位的检测+识别，供宿主报清楚“缺什么、在哪找”）。"""
        want = list(config.face_rec_models()) + list(config.face_det_models())
        return [m for m in want if not os.path.isfile(os.path.join(config.MODEL_DIR, m))]

    def _emb_model(self, conn: sqlite3.Connection) -> str | None:
        row = conn.execute("SELECT value FROM meta WHERE key='emb_model'").fetchone()
        return row[0] if row is not None else None

    def emb_model_info(self) -> dict:
        """宿主扫描前预检用：登记模型 vs 当前将加载的模型 + 人物数 + 模型文件就绪情况。"""
        with self._conn() as conn:
            registered = self._emb_model(conn)
            n = conn.execute("SELECT COUNT(*) FROM persons").fetchone()[0]
        active = self._active_rec_model()
        missing = self.missing_face_models()
        return {
            "registered": registered,
            "active": active,
            "persons": n,
            "ready": active is not None and not missing,
            "missing": missing,
            "model_dir": config.MODEL_DIR,
            "tier": config.active_face_tier(),
        }

    def rebuild(self) -> dict:
        """备份后清空全库（换模型 / 修复错乱的显式动作，宿主二次确认后调用）。

        备份文件与库同目录（persons.db.bak-YYYYmmdd-HHMMSS）；清库后 meta 一并
        清空，下次 register 会写入当前激活模型——期间宿主预检（registered=None）
        放行，行为与全新安装一致。
        """
        stamp = time.strftime("%Y%m%d-%H%M%S")
        backup = f"{self.db_path}.bak-{stamp}"
        # 先统计（库为空也允许重建），再 checkpoint 合入 wal，最后物理备份 ——
        # wal_checkpoint(TRUNCATE) 不能在自身写事务内执行，顺序必须这样
        with self._conn() as conn:
            persons = conn.execute("SELECT COUNT(*) FROM persons").fetchone()[0]
            faces = conn.execute("SELECT COUNT(*) FROM faces").fetchone()[0]
        conn = self._conn()
        try:
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        finally:
            conn.close()
        import shutil

        shutil.copy2(self.db_path, backup)
        with self._conn() as conn:
            conn.execute("BEGIN IMMEDIATE")
            try:
                conn.execute("DELETE FROM faces")
                conn.execute("DELETE FROM persons")
                conn.execute("DELETE FROM meta")
                conn.commit()
            except Exception:
                conn.rollback()
                raise
        return {"backup": backup, "persons": persons, "faces": faces}

    def _next_id(self, conn: sqlite3.Connection) -> str:
        row = conn.execute("SELECT MAX(CAST(SUBSTR(id,2) AS INTEGER)) AS m FROM persons").fetchone()
        n = (row["m"] or 0) + 1
        return f"P{n:03d}"

    # ------------------------------------------------------------------
    # 人工修正覆盖（宿主在人物页执行「移出 / 移动到其他人物」时写入）
    #
    # 为什么要有这张表：`register()` 每次扫描都是「匹配质心 → 插一行 faces」，
    # 光改 faces.person_id 挡不住下次扫描按质心重新认错。覆盖表以
    # (photo_path, bbox) 为主键，扫描时优先于质心匹配，且与 faces 解耦 ——
    # 重建人物库清空 faces 时覆盖记录仍在（目标人物不存在则自动失效回退）。
    # ------------------------------------------------------------------
    def lookup_override(self, photo_path: str, bbox: str) -> str | None:
        """返回该人脸的覆盖目标人物；无记录返回 None，NO_PERSON 表示移出全部人物。"""
        with self._conn() as conn:
            row = conn.execute(
                "SELECT person_id FROM face_overrides WHERE photo_path=? AND bbox=?",
                (photo_path, bbox),
            ).fetchone()
        return row["person_id"] if row else None

    def set_overrides(self, items: list[tuple[str, str, str]]) -> int:
        """批量写覆盖项 (photo_path, bbox, person_id)；person_id=NO_PERSON 表示移出。"""
        if not items:
            return 0
        now = time.strftime("%Y-%m-%d %H:%M:%S")
        with self._conn() as conn:
            conn.executemany(
                "INSERT OR REPLACE INTO face_overrides(photo_path, bbox, person_id, created_at)"
                " VALUES(?,?,?,?)",
                [(p, b, pid, now) for (p, b, pid) in items],
            )
        return len(items)

    def clear_overrides(self, photo_paths: list[str]) -> int:
        """清掉这些照片的全部覆盖（清除识别数据时调用，让照片回到全新状态）。"""
        n = 0
        paths = [p for p in photo_paths if p]
        with self._conn() as conn:
            for i in range(0, len(paths), 400):
                chunk = paths[i:i + 400]
                holders = ",".join("?" * len(chunk))
                n += conn.execute(
                    f"DELETE FROM face_overrides WHERE photo_path IN ({holders})", chunk
                ).rowcount
        return n

    def _person_exists(self, pid: str) -> bool:
        with self._conn() as conn:
            return conn.execute("SELECT 1 FROM persons WHERE id=?", (pid,)).fetchone() is not None


    # ------------------------------------------------------------------
    def match(self, emb: np.ndarray) -> tuple[str | None, float]:
        """返回 (person_id, sim) 或 (None, 0)。只与质心比较。"""
        with self._conn() as conn:
            rows = conn.execute("SELECT id, centroid FROM persons").fetchall()
        best_id, best_sim = None, 0.0
        for r in rows:
            centroid = self._from_blob(r["centroid"])
            sim = float(np.dot(emb, centroid) / (np.linalg.norm(emb) * np.linalg.norm(centroid) + 1e-9))
            if sim > best_sim:
                best_sim, best_id = sim, r["id"]
        return (best_id, best_sim) if best_sim >= config.FACE_SIM else (None, best_sim)

    def register(
        self,
        emb: np.ndarray,
        photo_path: str,
        bbox: str,
        *,
        allow_new: bool = True,
        update_centroid: bool = True,
    ) -> tuple[str | None, float]:
        """匹配或新建人物，返回 (person_id, sim)。

        allow_new/update_centroid 是给“边缘质量脸”（极端侧脸/小脸/轻模糊）用的：
        它们是真脸，并入已有簇无害，但**不能用来新建人物、也不该更新质心**——
        实测垃圾簇（如 P111）都是从“一张烂脸开出一个新 P 编号、把质心拖向自己、
        再吸更多烂脸”开始。默认 True 保持原有调用点语义不变。

        allow_new=False 且未匹配上 → 返回 (None, sim)，**不入库**（宁可不标号）。
        """
        emb = np.asarray(emb, dtype=np.float32)
        norm = np.linalg.norm(emb)
        if norm > 0:
            emb = emb / norm
        # 人工修正优先：用户把这个 (照片, 人脸框) 指定给了某个人物，或指定为
        # 「不属于任何人」。命中覆盖就不再跑质心匹配 —— 这就是「重新扫描
        # 不覆盖人工修正」的落点。目标人物已被删除（如重建人物库）时覆盖失效，
        # 回退到正常匹配，避免把脸挂到不存在的编号上。
        override = self.lookup_override(photo_path, bbox)
        if override == NO_PERSON:
            return None, 0.0
        if override is not None and not self._person_exists(override):
            override = None
        if override is not None:
            person_id, sim = override, 1.0
        else:
            person_id, sim = self.match(emb)
        if person_id is None and not allow_new:
            return None, sim
        now = time.strftime("%Y-%m-%d %H:%M:%S")
        active = self._active_rec_model()
        if active is None:
            # 模型文件缺失：绝不在 meta 里写“幻觉档位”，也不静默把脸归入新建人物
            # （那样会粉饰成“扫出来了”。必须报错，由宿主报给用户）
            raise RuntimeError(
                f"人脸识别模型文件缺失（{config.MODEL_DIR}）：缺少 "
                + "、".join(self.missing_face_models())
            )
        with self._conn() as conn:
            # 向量空间防混（换模型后旧质心与新嵌入不可比，混聚 = 静默错挂）：
            # 已有登记但模型不匹配 → 直接报错，由宿主引导重建人物库。
            # 库为空/meta 缺失（全新安装或刚重建）→ 记下当前模型后正常登记。
            registered = self._emb_model(conn)
            if registered is not None and registered != active:
                raise RuntimeError(
                    f"人脸识别模型已切换（{registered} → {active}），"
                    "旧人物库的向量与新模型不可比；请在「人物」页执行「重建人物库」后重新扫描"
                )
            if registered is None:
                conn.execute(
                    "INSERT OR REPLACE INTO meta(key, value) VALUES('emb_model', ?)",
                    (active,),
                )
            if person_id is None:
                person_id = self._next_id(conn)
                conn.execute(
                    "INSERT INTO persons(id, name, centroid, face_count, created_at) VALUES(?,?,?,1,?)",
                    (person_id, person_id, self._to_blob(emb), now),
                )
            else:
                row = conn.execute(
                    "SELECT centroid, face_count FROM persons WHERE id=?", (person_id,)
                ).fetchone()
                if update_centroid:
                    # 增量均值并归一化
                    c = self._from_blob(row["centroid"])
                    n = row["face_count"]
                    c = (c * n + emb) / (n + 1)
                    c = c / (np.linalg.norm(c) + 1e-9)
                    conn.execute(
                        "UPDATE persons SET centroid=?, face_count=? WHERE id=?",
                        (self._to_blob(c), n + 1, person_id),
                    )
                else:
                    # 边缘质量脸：计数照加（“张脸”要真实），但质心不动
                    conn.execute(
                        "UPDATE persons SET face_count=? WHERE id=?",
                        (row["face_count"] + 1, person_id),
                    )
            conn.execute(
                "INSERT INTO faces(person_id, photo_path, bbox, embedding, created_at) VALUES(?,?,?,?,?)",
                (person_id, photo_path, bbox, self._to_blob(emb), now),
            )
        return person_id, sim

    # ------------------------------------------------------------------
    def list_persons(self) -> list[dict]:
        with self._conn() as conn:
            rows = conn.execute(
                "SELECT id, name, face_count, created_at FROM persons ORDER BY id"
            ).fetchall()
        return [dict(r) for r in rows]

    def merge(self, target: str, source: str) -> bool:
        """把 source 的人脸与计数并入 target，删除 source。"""
        with self._conn() as conn:
            t = conn.execute("SELECT centroid, face_count FROM persons WHERE id=?", (target,)).fetchone()
            s = conn.execute("SELECT centroid, face_count FROM persons WHERE id=?", (source,)).fetchone()
            if t is None or s is None or target == source:
                return False
            tc = self._from_blob(t["centroid"]) * t["face_count"]
            sc = self._from_blob(s["centroid"]) * s["face_count"]
            nc = (tc + sc) / (t["face_count"] + s["face_count"])
            nc = nc / (np.linalg.norm(nc) + 1e-9)
            conn.execute(
                "UPDATE persons SET centroid=?, face_count=? WHERE id=?",
                (self._to_blob(nc), t["face_count"] + s["face_count"], target),
            )
            conn.execute("UPDATE faces SET person_id=? WHERE person_id=?", (target, source))
            # 覆盖记录跟着人脸走：否则下次扫描会被覆盖表拉回已删除的 source。
            conn.execute("UPDATE face_overrides SET person_id=? WHERE person_id=?", (target, source))
            conn.execute("DELETE FROM persons WHERE id=?", (source,))
        return True

    def rename(self, person_id: str, name: str) -> bool:
        with self._conn() as conn:
            cur = conn.execute("UPDATE persons SET name=? WHERE id=?", (name, person_id))
            return cur.rowcount > 0

    def delete(self, person_id: str) -> bool:
        with self._conn() as conn:
            conn.execute("DELETE FROM faces WHERE person_id=?", (person_id,))
            # 人物被删 = 放弃这组聚类，覆盖记录一并清掉，让这些脸能被重新聚类。
            conn.execute("DELETE FROM face_overrides WHERE person_id=?", (person_id,))
            cur = conn.execute("DELETE FROM persons WHERE id=?", (person_id,))
            return cur.rowcount > 0

    def representative_face(self, person_id: str) -> tuple[str, str] | None:
        """返回该人物的代表脸 (photo_path, bbox)，无脸时返回 None。

        代表脸取「与质心最相似」的一张（而非最早登记）：质心是该人物所有脸的
        均值方向，与它最相似的脸就是最典型的正面大脸——最早登记的可能是远处
        小脸/侧脸，裁出来糊成一片（FEAT：人物封面质量）。并列时取 bbox 面积
        更大者，再并列取 id 小者，保证稳定不跳变。
        bbox 为 "(x1, y1, x2, y2)" 字符串，由检测端写入。
        """
        with self._conn() as conn:
            prow = conn.execute(
                "SELECT centroid FROM persons WHERE id=?", (person_id,)
            ).fetchone()
            if prow is None:
                return None
            centroid = self._from_blob(prow[0])
            cn = np.linalg.norm(centroid)
            if cn <= 0:
                return None
            rows = conn.execute(
                "SELECT id, photo_path, bbox, embedding FROM faces WHERE person_id=? "
                "ORDER BY created_at ASC, id ASC",
                (person_id,),
            ).fetchall()
        best: tuple[float, int, int, str, str] | None = None  # (-sim, -area, id, path, bbox)
        for r in rows:
            e = self._from_blob(r[3])
            en = np.linalg.norm(e)
            if en <= 0:
                continue
            sim = float(np.dot(e, centroid) / (en * cn))
            nums = [int(v) for v in re.findall(r"-?\d+", r[2])][:4]
            area = (nums[2] - nums[0]) * (nums[3] - nums[1]) if len(nums) >= 4 else 0
            key = (-sim, -area, r[0], r[1], r[2])
            if best is None or key[:3] < best[:3]:
                best = key
        if best is None:
            return None
        return (best[3], best[4])


_store: PersonStore | None = None


def get_store() -> PersonStore:
    global _store
    if _store is None:
        _store = PersonStore()
    return _store
