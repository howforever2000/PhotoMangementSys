"""持久层：人物注册表（SQLite）

表结构：
  persons(id TEXT PK, name TEXT, centroid BLOB, face_count INT, created_at TEXT)
    centroid: 128 维归一化均值向量（float32 小端）
  faces(id INTEGER PK AUTOINCREMENT, person_id TEXT, photo_path TEXT,
        bbox TEXT, embedding BLOB, created_at TEXT)

职责：
  - match(embedding) → 与各 person 质心做余弦相似度，≥FACE_SIM 返回最相近者
  - register(embedding, photo, bbox) → 命中则并入，否则新建 P 编号
  - merge/rename/list/delete 供前端管理人物
仅 Python 侧使用，与 Rust 相册库完全解耦（独立 persons.db）。
"""
import os
import sqlite3
import time

import numpy as np

from .. import config

EMB_DIM = 128


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

    # ------------------------------------------------------------------
    @staticmethod
    def _to_blob(emb: np.ndarray) -> bytes:
        return np.asarray(emb, dtype=np.float32).tobytes()

    @staticmethod
    def _from_blob(blob: bytes) -> np.ndarray:
        return np.frombuffer(blob, dtype=np.float32).copy()

    def _next_id(self, conn: sqlite3.Connection) -> str:
        row = conn.execute("SELECT MAX(CAST(SUBSTR(id,2) AS INTEGER)) AS m FROM persons").fetchone()
        n = (row["m"] or 0) + 1
        return f"P{n:03d}"

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

    def register(self, emb: np.ndarray, photo_path: str, bbox: str) -> tuple[str, float]:
        """匹配或新建人物，返回 (person_id, sim)。"""
        emb = np.asarray(emb, dtype=np.float32)
        norm = np.linalg.norm(emb)
        if norm > 0:
            emb = emb / norm
        person_id, sim = self.match(emb)
        now = time.strftime("%Y-%m-%d %H:%M:%S")
        with self._conn() as conn:
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
                # 增量均值并归一化
                c = self._from_blob(row["centroid"])
                n = row["face_count"]
                c = (c * n + emb) / (n + 1)
                c = c / (np.linalg.norm(c) + 1e-9)
                conn.execute(
                    "UPDATE persons SET centroid=?, face_count=? WHERE id=?",
                    (self._to_blob(c), n + 1, person_id),
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
            conn.execute("DELETE FROM persons WHERE id=?", (source,))
        return True

    def rename(self, person_id: str, name: str) -> bool:
        with self._conn() as conn:
            cur = conn.execute("UPDATE persons SET name=? WHERE id=?", (name, person_id))
            return cur.rowcount > 0

    def delete(self, person_id: str) -> bool:
        with self._conn() as conn:
            conn.execute("DELETE FROM faces WHERE person_id=?", (person_id,))
            cur = conn.execute("DELETE FROM persons WHERE id=?", (person_id,))
            return cur.rowcount > 0


_store: PersonStore | None = None


def get_store() -> PersonStore:
    global _store
    if _store is None:
        _store = PersonStore()
    return _store
