//! 人物注册表直读模块（持久层，绕过 Python 微服务）
//!
//! 职责：直接读写 `python/data/persons.db`（SQLite），提供人物列表 / 重命名 /
//! 合并 / 代表脸定位与本地头像裁剪。智慧相册与扫描面板的人物展示从此**不依赖
//! Python 微服务运行**（服务只在执行识别扫描时需要）。
//!
//! 与 Python 侧的一致性：
//! - 表结构见 `vcr/persistence/person_store.py`（persons/faces 两表）
//! - 合并逻辑复刻 `PersonStore.merge`：质心加权平均 + 归一化 + faces 迁移
//! - bbox 存储格式为 `"(x1, y1, x2, y2)"` 字符串
//! - PersonStore 每次操作都新开连接（无内存缓存），Rust 直写不会产生不一致
//!
//! 解耦原则：不依赖 db（相册库）/ vision 模块；路径解析与 config.py 同源。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 人物条目 —— 对应前端 `PersonInfo`（face_count 降序返回）
#[derive(Debug, Clone, Serialize)]
pub struct PersonEntry {
    pub id: String,
    pub name: String,
    pub face_count: i64,
    pub created_at: String,
}

/// 人物库目录：与 `python/vcr/config.py::DATA_DIR` 同源（VCR_DATA_DIR 优先）
///
/// 统一口径：宿主 `lib.rs::setup` 启动时把 VCR_DATA_DIR 指向
/// `app_data_dir/vcr-data`（安装版与开发版一致），于是 Python 微服务与 Rust
/// 人物页读写的是**同一份** persons.db。
/// 未设置该变量时（例如直接跑 bench 脚本 / 单测）回落到项目 `python/data`。
///
/// 历史包袱：此处曾用编译期常量 `CARGO_MANIFEST_DIR` 拼路径，安装到其他机器后
/// 指向构建机源码目录，而微服务写的是 VCR_DATA_DIR → 人物页与扫描结果读写分裂
/// （见 BUG-2026-0916-005）。
fn data_dir() -> PathBuf {
    if let Ok(v) = std::env::var("VCR_DATA_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    // 兜底：仅开发态（bench / 单测未跑 setup）指向源码目录。
    // 收口到 devpath —— release 下返回空路径，不把构建机绝对路径写进二进制
    // （BUG-2026-1013-001）。
    crate::devpath::dev_project_dir()
        .join("python")
        .join("data")
}

/// persons.db 路径（data_dir 下的 persons.db）
fn persons_db_path() -> PathBuf {
    data_dir().join("persons.db")
}

/// 打开 persons.db；文件不存在 → None（从未跑过人脸扫描属正常情况）
fn open_db() -> Result<Option<rusqlite::Connection>, String> {
    let path = persons_db_path();
    if !path.is_file() {
        return Ok(None);
    }
    let conn = rusqlite::Connection::open(&path)
        .map_err(|e| format!("打开人物注册表失败: {e}"))?;
    Ok(Some(conn))
}

/// 列出全部人物，按 face_count 降序（出现次数多的在前）、id 升序稳定排序
pub fn list_persons() -> Result<Vec<PersonEntry>, String> {
    let Some(conn) = open_db()? else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare("SELECT id, name, face_count, created_at FROM persons ORDER BY face_count DESC, id ASC")
        .map_err(|e| format!("查询人物失败: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PersonEntry {
                id: r.get(0)?,
                name: r.get(1)?,
                face_count: r.get(2)?,
                created_at: r.get(3)?,
            })
        })
        .map_err(|e| format!("查询人物失败: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取人物行失败: {e}"))
}

/// 列出某人物出现的全部照片（按拍摄/录入时间升序去重）。
/// 返回 persons.db 中该人物 faces 行对应的 photo_path；图片可能缺失，交由前端占比占位。
pub fn list_person_photos(pid: &str) -> Result<Vec<String>, String> {
    let Some(conn) = open_db()? else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare("SELECT DISTINCT photo_path FROM faces WHERE person_id = ?1 ORDER BY created_at ASC, id ASC")
        .map_err(|e| format!("查询人物照片失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![pid], |r| {
            r.get::<_, String>(0)
        })
        .map_err(|e| format!("读取人物照片失败: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取人物照片行失败: {e}"))
}

/// 列出在指定相册目录下出现过的人物（BUG-2026-0919-003）。
///
/// 问题：扫描面板的人物注册表此前读全局 `list_persons`，相册 A 的详情页里
/// 会看到其他相册的人物与全局脸数，与"当前相册"语境不符。
/// 做法：读 faces 全表 (person_id, photo_path)，用与照片归属解析同一套
/// `p_is_under`（Windows 大小写不敏感 + 前缀边界）过滤出落在 album_path
/// 之下的记录，按人物聚合**去重后的照片数**（一张脸多次/一图多脸都只算一图）。
/// 只返回在当前相册中出现（计数>0）的人物，按相册内照片数降序。
pub fn list_persons_in_album(album_path: &str) -> Result<Vec<PersonEntry>, String> {
    if album_path.trim().is_empty() {
        return Ok(Vec::new());
    }
    let Some(conn) = open_db()? else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare("SELECT person_id, photo_path FROM faces")
        .map_err(|e| format!("查询人脸失败: {e}"))?;
    let pairs: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| format!("查询人脸失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取人脸行失败: {e}"))?;
    drop(stmt);

    let mut per_person: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for (pid, photo) in pairs {
        if crate::persons::commands::p_is_under(album_path, &photo) {
            per_person.entry(pid).or_default().insert(photo);
        }
    }
    if per_person.is_empty() {
        return Ok(Vec::new());
    }

    // 补充人物显示名（persons 表）；只在当前相册出现过的人物才返回
    let mut stmt2 = conn
        .prepare("SELECT id, name, created_at FROM persons")
        .map_err(|e| format!("查询人物失败: {e}"))?;
    let rows = stmt2
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("查询人物失败: {e}"))?;
    let mut out: Vec<PersonEntry> = Vec::new();
    for row in rows {
        let (id, name, created_at) = row.map_err(|e| format!("读取人物行失败: {e}"))?;
        if let Some(paths) = per_person.get(&id) {
            if !paths.is_empty() {
                out.push(PersonEntry {
                    id,
                    name,
                    face_count: paths.len() as i64,
                    created_at,
                });
            }
        }
    }
    out.sort_by(|a, b| b.face_count.cmp(&a.face_count).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

/// 重命名人物（自定义命名；空名回退为编号本身）
pub fn rename_person(pid: &str, name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("名称不能为空".into());
    }
    let final_name: String = trimmed.chars().take(50).collect();
    let Some(conn) = open_db()? else {
        return Err(format!("人物注册表不存在: {}", persons_db_path().display()));
    };
    // 空名语义：恢复默认显示名 = 编号本身
    let effective: &str = if final_name == pid { pid } else { final_name.as_str() };
    let n = conn
        .execute(
            "UPDATE persons SET name = ?1 WHERE id = ?2",
            rusqlite::params![effective, pid],
        )
        .map_err(|e| format!("重命名失败: {e}"))?;
    if n == 0 {
        return Err(format!("人物不存在: {pid}"));
    }
    Ok(())
}

/// 合并人物：source 并入 target（质心加权平均 + faces 迁移 + 删除 source）
///
/// 复刻 python `PersonStore.merge` 的数学逻辑（float32 小端 128 维向量）。
pub fn merge_persons(target: &str, source: &str) -> Result<(), String> {
    if target == source {
        return Err("不能合并到自身".into());
    }
    let Some(conn) = open_db()? else {
        return Err(format!("人物注册表不存在: {}", persons_db_path().display()));
    };

    fn read_centroid(conn: &rusqlite::Connection, pid: &str) -> Result<(Vec<u8>, i64), String> {
        conn.query_row(
            "SELECT centroid, face_count FROM persons WHERE id = ?1",
            rusqlite::params![pid],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)),
        )
        .map_err(|_| format!("人物不存在: {pid}"))
    }

    let (t_blob, t_count) = read_centroid(&conn, target)?;
    let (s_blob, s_count) = read_centroid(&conn, source)?;

    // float32 小端向量加权平均 + L2 归一化（无外部依赖，手写运算）
    fn weighted_average(t: &[u8], tc: i64, s: &[u8], sc: i64) -> Option<Vec<u8>> {
        if t.len() != s.len() || t.is_empty() || t.len() % 4 != 0 {
            return None;
        }
        let dim = t.len() / 4;
        let mut out = vec![0f32; dim];
        for i in 0..dim {
            let b0 = i * 4;
            let tv = f32::from_le_bytes([t[b0], t[b0 + 1], t[b0 + 2], t[b0 + 3]]);
            let sv = f32::from_le_bytes([s[b0], s[b0 + 1], s[b0 + 2], s[b0 + 3]]);
            out[i] = (tv * tc as f32 + sv * sc as f32) / (tc + sc) as f32;
        }
        let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 1e-9 {
            for v in &mut out {
                *v /= norm;
            }
        }
        Some(out.iter().flat_map(|v| v.to_le_bytes()).collect())
    }

    let new_blob = weighted_average(&t_blob, t_count, &s_blob, s_count)
        .ok_or_else(|| "质心数据损坏（长度不一致）".to_string())?;

    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let r = (|| {
        conn.execute(
            "UPDATE persons SET centroid = ?1, face_count = ?2 WHERE id = ?3",
            rusqlite::params![new_blob, t_count + s_count, target],
        )
        .map_err(|e| format!("更新质心失败: {e}"))?;
        conn.execute(
            "UPDATE faces SET person_id = ?1 WHERE person_id = ?2",
            rusqlite::params![target, source],
        )
        .map_err(|e| format!("迁移人脸失败: {e}"))?;
        conn.execute("DELETE FROM persons WHERE id = ?1", rusqlite::params![source])
            .map_err(|e| format!("删除源人物失败: {e}"))?;
        Ok(())
    })();
    match r {
        Ok(_) => conn.execute_batch("COMMIT").map_err(|e| e.to_string()),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 删除人物：连同其全部 faces 行一并删除（直写 persons.db，离线可用）
pub fn delete_person(pid: &str) -> Result<(), String> {
    let Some(conn) = open_db()? else {
        return Err(format!("人物注册表不存在: {}", persons_db_path().display()));
    };
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let r = (|| {
        conn.execute("DELETE FROM faces WHERE person_id = ?1", rusqlite::params![pid])
            .map_err(|e| format!("删除人脸失败: {e}"))?;
        let n = conn
            .execute("DELETE FROM persons WHERE id = ?1", rusqlite::params![pid])
            .map_err(|e| format!("删除人物失败: {e}"))?;
        if n == 0 {
            return Err(format!("人物不存在: {pid}"));
        }
        Ok(())
    })();
    match r {
        Ok(_) => conn.execute_batch("COMMIT").map_err(|e| e.to_string()),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
// ---------------------------------------------------------------------------
// 人脸归属的人工修正（人物页「移出此人 / 移到其他人物」）
//
// 为什么不能只改 faces.person_id：扫描端 `PersonStore.register()` 每次都是
// 「按质心匹配 → 插入一行 faces」，只改行会被下次重扫按旧质心认回去。
// 因此这里同时写 `face_overrides`（photo_path + bbox 主键），Python 侧
// `register()` 会先查覆盖表，命中就不再跑质心匹配。
// ---------------------------------------------------------------------------

/// `face_overrides.person_id` 的哨兵值：明确「不属于任何人」（与 Python 侧 NO_PERSON 同值）
const FACE_OVERRIDE_NONE: &str = "__none__";

/// f32 blob（小端）→ 向量；空 / 长度非 4 的倍数 → None
fn blob_to_vec(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.is_empty() || blob.len() % 4 != 0 {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
    )
}

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// SQL 占位符串：`?start,?start+1,...`
fn placeholders(n: usize, start: usize) -> String {
    (start..start + n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// 重算某人物的人脸数与质心；一张脸都不剩就把人物整行删掉。
///
/// 质心重算（而不是从旧质心做减法）是为了避免「人工修正移走典型脸 → 质心漂移」
/// 的累积误差：faces 表才是唯一事实来源。
fn recompute_person(conn: &rusqlite::Connection, pid: &str) -> Result<i64, String> {
    let mut stmt = conn
        .prepare("SELECT embedding FROM faces WHERE person_id = ?1")
        .map_err(|e| format!("查询人脸失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![pid], |r| r.get::<_, Vec<u8>>(0))
        .map_err(|e| format!("查询人脸失败: {e}"))?;
    let mut sum: Vec<f32> = Vec::new();
    let mut n: i64 = 0;
    for row in rows {
        let blob = row.map_err(|e| format!("读取人脸向量失败: {e}"))?;
        let Some(v) = blob_to_vec(&blob) else { continue };
        if sum.is_empty() {
            sum = vec![0.0; v.len()];
        }
        if v.len() != sum.len() {
            continue; // 换模型后维度不一致的历史向量：跳过，不污染质心
        }
        for (a, b) in sum.iter_mut().zip(v.iter()) {
            *a += b;
        }
        n += 1;
    }
    if n == 0 {
        conn.execute("DELETE FROM persons WHERE id = ?1", rusqlite::params![pid])
            .map_err(|e| format!("删除空人物失败: {e}"))?;
        conn.execute(
            "DELETE FROM face_overrides WHERE person_id = ?1",
            rusqlite::params![pid],
        )
        .map_err(|e| format!("清理覆盖记录失败: {e}"))?;
        return Ok(0);
    }
    let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in sum.iter_mut() {
            *x /= norm;
        }
    }
    conn.execute(
        "UPDATE persons SET centroid = ?1, face_count = ?2 WHERE id = ?3",
        rusqlite::params![vec_to_blob(&sum), n, pid],
    )
    .map_err(|e| format!("更新质心失败: {e}"))?;
    Ok(n)
}

/// 下一个人物编号（P001 起，取当前最大值 +1）
fn next_person_id(conn: &rusqlite::Connection) -> Result<String, String> {
    let m: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(CAST(SUBSTR(id,2) AS INTEGER)), 0) FROM persons",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(format!("P{:03}", m + 1))
}

fn now_string() -> String {
    // 与 Python 侧同一列（TEXT）：这里存 unix 秒，只用于排错追查，不参与排序/比较
    crate::now().to_string()
}

/// 按照片路径删除人脸登记 + 覆盖记录（「清除数据库记录」调用）。
///
/// 只删人脸，不动 persons：空掉的人物由 `recompute_person` 顺势清理。
/// 返回删除的 faces 行数。
pub fn delete_faces_by_paths(paths: &[String]) -> Result<usize, String> {
    if paths.is_empty() {
        return Ok(0);
    }
    let Some(conn) = open_db()? else {
        return Ok(0);
    };
    let mut affected: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut deleted = 0usize;
    for chunk in paths.chunks(400) {
        let ph = placeholders(chunk.len(), 1);
        let sel = format!("SELECT DISTINCT person_id FROM faces WHERE photo_path IN ({ph})");
        {
            let mut stmt = conn.prepare(&sel).map_err(|e| format!("查询人脸失败: {e}"))?;
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let rows = stmt
                .query_map(params.as_slice(), |r| r.get::<_, String>(0))
                .map_err(|e| format!("查询人脸失败: {e}"))?;
            for r in rows {
                affected.insert(r.map_err(|e| format!("读取人脸行失败: {e}"))?);
            }
        }
        let del = format!("DELETE FROM faces WHERE photo_path IN ({ph})");
        let params: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
        deleted += conn
            .execute(&del, params.as_slice())
            .map_err(|e| format!("删除人脸失败: {e}"))?;
        // 覆盖记录同生命周期：清掉后照片回到「全新」，下次扫描可自由重新聚类。
        let del_ov = format!("DELETE FROM face_overrides WHERE photo_path IN ({ph})");
        conn.execute(&del_ov, params.as_slice())
            .map_err(|e| format!("清理覆盖记录失败: {e}"))?;
    }
    for pid in affected {
        recompute_person(&conn, &pid)?;
    }
    Ok(deleted)
}

/// 人脸改挂结果（前端提示用）
#[derive(Debug, Clone, Serialize, Default)]
pub struct FaceMoveOutcome {
    /// 请求处理的照片数
    pub requested: usize,
    /// 实际改动的人脸行数（一张照片可能多张脸）
    pub moved: usize,
    pub from_pid: String,
    /// 目标人物；None = 移出全部人物
    pub to_pid: Option<String>,
    /// 本次新建的人物编号（目标为「新建」时）
    pub created_pid: Option<String>,
    /// 没有该人物人脸的照片（正常情况，不算失败）
    pub skipped_paths: Vec<String>,
}

/// 把 `from_pid` 名下、属于 `paths` 的人脸改挂到 `to_pid` 或新建人物；
/// `to_pid` 与 `new_name` 同时为 None → 移出全部人物（删除人脸行 + 覆盖为「无人」）。
///
/// 三种用法：
/// 1. 移出此人：`to_pid=None, new_name=None`
/// 2. 移到已有的人：`to_pid=Some("P003")`
/// 3. 移到新建的人：`new_name=Some("李四")`
pub fn move_faces(
    from_pid: &str,
    to_pid: Option<&str>,
    new_name: Option<&str>,
    paths: &[String],
) -> Result<FaceMoveOutcome, String> {
    let mut out = FaceMoveOutcome {
        requested: paths.len(),
        from_pid: from_pid.to_string(),
        ..Default::default()
    };
    if paths.is_empty() {
        return Ok(out);
    }
    if to_pid == Some(from_pid) {
        return Err("目标人物与当前人物相同".into());
    }
    let Some(conn) = open_db()? else {
        return Err(format!("人物注册表不存在: {}", persons_db_path().display()));
    };

    // 1) 取出要动的人脸行（顺带确认目标人物存在）
    let mut rows: Vec<(i64, String, String, Vec<u8>)> = Vec::new();
    for chunk in paths.chunks(400) {
        let ph = placeholders(chunk.len(), 2);
        let sql = format!(
            "SELECT id, photo_path, bbox, embedding FROM faces \
             WHERE person_id = ?1 AND photo_path IN ({ph})"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("查询人脸失败: {e}"))?;
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
        params.push(&from_pid);
        for p in chunk {
            params.push(p);
        }
        let it = stmt
            .query_map(params.as_slice(), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                ))
            })
            .map_err(|e| format!("查询人脸失败: {e}"))?;
        for r in it {
            rows.push(r.map_err(|e| format!("读取人脸行失败: {e}"))?);
        }
    }
    if rows.is_empty() {
        return Ok(out);
    }
    if let Some(t) = to_pid {
        let exists: bool = conn
            .query_row("SELECT 1 FROM persons WHERE id = ?1", rusqlite::params![t], |_| Ok(()))
            .is_ok();
        if !exists {
            return Err(format!("目标人物不存在: {t}"));
        }
    }

    // 2) 目标人物：已有 / 新建（编号先算好，行在事务里插）
    let new_pid = if to_pid.is_none() && new_name.is_some() {
        Some(next_person_id(&conn)?)
    } else {
        None
    };
    let target: Option<String> = to_pid.map(str::to_string).or_else(|| new_pid.clone());

    // 3) 单事务：建人物（可选）→ 改挂/删除 → 写覆盖 → 重算两方质心
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| format!("开启事务失败: {e}"))?;
    let r = (|| -> Result<(), String> {
        if let Some(pid) = &new_pid {
            let raw = new_name.unwrap_or("").trim();
            let name: String = if raw.is_empty() {
                pid.clone()
            } else {
                raw.chars().take(50).collect()
            };
            let seed = blob_to_vec(&rows[0].3).ok_or_else(|| "人脸向量损坏".to_string())?;
            conn.execute(
                "INSERT INTO persons(id, name, centroid, face_count, created_at) VALUES(?1,?2,?3,0,?4)",
                rusqlite::params![pid, name, vec_to_blob(&seed), now_string()],
            )
            .map_err(|e| format!("新建人物失败: {e}"))?;
        }
        let ov_target = target.clone().unwrap_or_else(|| FACE_OVERRIDE_NONE.to_string());
        let now = now_string();
        for (id, path, bbox, _) in &rows {
            match &target {
                Some(t) => {
                    conn.execute(
                        "UPDATE faces SET person_id = ?1 WHERE id = ?2",
                        rusqlite::params![t, id],
                    )
                    .map_err(|e| format!("改挂人脸失败: {e}"))?;
                }
                None => {
                    conn.execute("DELETE FROM faces WHERE id = ?1", rusqlite::params![id])
                        .map_err(|e| format!("删除人脸失败: {e}"))?;
                }
            }
            // 关键：写覆盖，保证下次重扫不会被质心认回原人物
            conn.execute(
                "INSERT OR REPLACE INTO face_overrides(photo_path, bbox, person_id, created_at) \
                 VALUES(?1,?2,?3,?4)",
                rusqlite::params![path, bbox, ov_target, now],
            )
            .map_err(|e| format!("写入修正记录失败: {e}"))?;
        }
        recompute_person(&conn, from_pid)?;
        if let Some(t) = &target {
            recompute_person(&conn, t)?;
        }
        Ok(())
    })();
    match r {
        Ok(_) => conn
            .execute_batch("COMMIT")
            .map_err(|e| format!("提交事务失败: {e}"))?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }

    let touched: std::collections::HashSet<&str> = rows.iter().map(|r| r.1.as_str()).collect();
    out.skipped_paths = paths
        .iter()
        .filter(|p| !touched.contains(p.as_str()))
        .cloned()
        .collect();
    out.moved = rows.len();
    out.to_pid = target;
    out.created_pid = new_pid;
    Ok(out)
}

fn representative_face(conn: &rusqlite::Connection, pid: &str) -> Option<(String, String)> {
    // 为什么改：质心是该人物所有脸的方向均值，与它最相似的脸就是最典型的正面大脸；
    // 最早登记的可能是远处小脸/侧脸，裁出来糊成一片（用户反馈的「封面看不出是谁」）。
    // 并列时取 bbox 面积更大者，再并列取 id 小者 —— 结果稳定不跳变。
    let centroid: Vec<u8> = conn
        .query_row(
            "SELECT centroid FROM persons WHERE id = ?1",
            rusqlite::params![pid],
            |r| r.get(0),
        )
        .ok()?;
    if centroid.is_empty() || centroid.len() % 4 != 0 {
        return None;
    }
    let c: Vec<f32> = centroid
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let cn = c.iter().map(|v| v * v).sum::<f32>().sqrt();
    if cn <= 0.0 {
        return None;
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, photo_path, bbox, embedding FROM faces WHERE person_id = ?1 \
             ORDER BY created_at ASC, id ASC",
        )
        .ok()?;
    let rows = stmt
        .query_map(rusqlite::params![pid], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })
        .ok()?;
    let mut best: Option<(f32, i64, i64, String, String)> = None;
    // BUG-2026-1027-005：另存一份「原图仍在磁盘上」的最优解。
    // 代表脸可能选到一张已经被删/移走的照片，那样头像与卡片封面会**永远裁不出来**
    // （实测真实注册表里就有这种人：报「代表脸原图不存在」）。
    // 所以优先取存在的那张，一张都不存在时才退回最优解（让报错信息仍可读）。
    let mut best_existing: Option<(f32, i64, i64, String, String)> = None;
    for (id, path, bbox, blob) in rows.flatten() {
        if blob.len() != c.len() * 4 {
            continue;
        }
        let e: Vec<f32> = blob
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let en = e.iter().map(|v| v * v).sum::<f32>().sqrt();
        if en <= 0.0 {
            continue;
        }
        let dot = c.iter().zip(e.iter()).map(|(a, b)| a * b).sum::<f32>();
        let sim = dot / (cn * en);
        let area = parse_bbox(&bbox)
            .map(|(x1, y1, x2, y2)| (x2 - x1) * (y2 - y1))
            .unwrap_or(0);
        let better = match &best {
            None => true,
            Some((bs, ba, bid, _, _)) => {
                sim > *bs || (sim == *bs && (area > *ba || (area == *ba && id < *bid)))
            }
        };
        if better {
            let exists = std::path::Path::new(&path).is_file();
            let cand = (sim, area, id, path, bbox);
            if exists {
                best_existing = Some(cand.clone());
            }
            best = Some(cand);
        }
    }
    best_existing.or(best).map(|(_, _, _, path, bbox)| (path, bbox))
}

/// 人脸集签名 `"<张数>:<最大face id>"` —— 头像缓存的身份判据。
///
/// 为什么用它：人物封面曾经按「文件存在就用」，而 P 编号在库被重建后会被
/// **新的人物复用**（旧头像文件留在磁盘上）→ 封面显示的是别人（BUG-2026-1001-001，
/// 实测 2417 个头像里 1952 个比其人物的创建时间还早）。签名只依赖人脸集本身：
/// 新增登记/合并/删除都会改变它，因此「签名一致 ⇒ 头像仍是这个人」。
fn face_sig_of(conn: &rusqlite::Connection, pid: &str) -> Option<String> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(id), 0) FROM faces WHERE person_id = ?1",
        rusqlite::params![pid],
        |r| {
            let n: i64 = r.get(0)?;
            let m: i64 = r.get(1)?;
            Ok(format!("{n}:{m}"))
        },
    )
    .ok()
}

/// 单人的人脸集签名（头像命令层用；库不存在 → None）
pub fn face_sig(pid: &str) -> Result<Option<String>, String> {
    let Some(conn) = open_db()? else {
        return Ok(None);
    };
    Ok(face_sig_of(&conn, pid))
}

/// 全量人脸集签名（批量头像校验用：一次 GROUP BY 拿全部，避免 N 次查询）
pub fn face_sigs() -> Result<std::collections::HashMap<String, String>, String> {
    let mut out = std::collections::HashMap::new();
    let Some(conn) = open_db()? else {
        return Ok(out);
    };
    let mut stmt = conn
        .prepare("SELECT person_id, COUNT(*), COALESCE(MAX(id), 0) FROM faces GROUP BY person_id")
        .map_err(|e| format!("查询人脸集签名失败: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| format!("查询人脸集签名失败: {e}"))?;
    for row in rows.flatten() {
        out.insert(row.0, format!("{}:{}", row.1, row.2));
    }
    Ok(out)
}

/// 人物列表（id + 人脸集签名）：批量头像校验用，避免为每个 pid 单独查库
pub fn persons_with_sig() -> Result<Vec<(String, Option<String>)>, String> {
    let sigs = face_sigs()?;
    let Some(conn) = open_db()? else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare("SELECT id FROM persons ORDER BY face_count DESC, id ASC")
        .map_err(|e| format!("查询人物失败: {e}"))?;
    let ids: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| format!("查询人物失败: {e}"))?
        .flatten()
        .collect();
    Ok(ids
        .into_iter()
        .map(|id| {
            let s = sigs.get(&id).cloned();
            (id, s)
        })
        .collect())
}

/// 清空人物库（备份后）并返回 (备份路径, 人物数, 人脸数)。
///
/// 换模型/修复错乱后的**显式动作**（宿主二次确认后调用）：不做任何自动清库。
/// 用 SQLite 自己的备份 API（VACUUM INTO 需要目标不存在；这里用 backup 更稳）。
pub fn reset_registry() -> Result<(String, i64, i64), String> {
    let path = persons_db_path();
    if !path.is_file() {
        return Err(format!("人物注册表不存在: {}", path.display()));
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = path.with_file_name(format!("persons.db.bak-{stamp}"));
    std::fs::copy(&path, &backup).map_err(|e| format!("备份人物库失败: {e}"))?;
    let conn = rusqlite::Connection::open(&path).map_err(|e| format!("打开人物注册表失败: {e}"))?;
    let (mut persons, mut faces) = (0i64, 0i64);
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let r = (|| -> Result<(), String> {
        persons = conn
            .query_row("SELECT COUNT(*) FROM persons", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        faces = conn
            .query_row("SELECT COUNT(*) FROM faces", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM faces", []).map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM persons", []).map_err(|e| e.to_string())?;
        // 人工修正覆盖也清空：重建后 P 编号会重新分配，留着旧覆盖只会在
        // 「P001 换成了别人」之后把脸挂到错误的人身上。重建 = 从头聚类。
        let _ = conn.execute("DELETE FROM face_overrides", []);
        // meta 一并清空：下次登记时 Python 会写入当前激活模型（全新安装语义）
        let _ = conn.execute("DELETE FROM meta", []);
        Ok(())
    })();
    match r {
        Ok(_) => conn.execute_batch("COMMIT").map_err(|e| e.to_string())?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    Ok((backup.to_string_lossy().into_owned(), persons, faces))
}

/// 从 "(x1, y1, x2, y2)" 提取整数坐标
fn parse_bbox(raw: &str) -> Option<(i64, i64, i64, i64)> {
    let nums: Vec<i64> = raw
        .split(|c: char| !(c.is_ascii_digit() || c == '-'))
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if nums.len() < 4 {
        return None;
    }
    let (x1, y1, x2, y2) = (nums[0], nums[1], nums[2], nums[3]);
    if x2 <= x1 || y2 <= y1 {
        return None;
    }
    Some((x1, y1, x2, y2))
}

/// 本地裁剪人物头像并写入 cache_path（96×96 JPEG）
///
/// JPEG 走分级降采样（1/1、1/2、1/4 取「裁剪框仍 ≥160px」的最小分辨率档，
/// 大图从秒级降到几十毫秒）；其他格式全尺寸解码后裁剪。
/// 任一步失败返回 Err，由调用方回退占位样式。
pub fn crop_avatar_local(pid: &str, cache_path: &Path) -> Result<(), String> {
    let Some(conn) = open_db()? else {
        return Err("人物注册表不存在".into());
    };
    let (photo_path, bbox_raw) =
        representative_face(&conn, pid).ok_or_else(|| format!("人物无登记人脸: {pid}"))?;
    if !Path::new(&photo_path).is_file() {
        return Err(format!("代表脸原图不存在: {photo_path}"));
    }
    let (x1, y1, x2, y2) = parse_bbox(&bbox_raw).ok_or_else(|| format!("bbox 格式异常: {bbox_raw}"))?;

    // 外扩 12% 并夹紧到图内
    let bw = x2 - x1;
    let bh = y2 - y1;
    let dx = (bw as f64 * 0.12) as i64;
    let dy = (bh as f64 * 0.12) as i64;

    // BUG-2026-1027-004：不能按扩展名判 JPEG —— 库里 95 条人脸原图是 PNG 却叫 .jpg，
    // 交给 jpeg_decoder 会报「无法读取图片头信息」，那批人物永远没有头像。
    let is_jpeg = crate::avatar::is_jpeg_file(Path::new(&photo_path));

    // BUG-2026-1014-002：bbox 来自 Python 检测侧，那边已按 EXIF 转正，故 bbox
    // 处于**显示坐标系**。此处若不解码后同样转正，就会「拿转正后的坐标去裁躺倒的
    // 像素」，裁出背景或半张脸。故这里先读 Orientation，JPEG 降采样按**转正后**
    // 的目标尺寸请求（省内存），解码后再转正，最后裁剪。
    let orientation = crate::exif_orient::read_orientation(Path::new(&photo_path));

    let dynamic_img = if is_jpeg {
        // 分级降采样：从 1/8 起找「裁剪框缩放后仍 ≥160px」的最大降采样档（最快），
        // 都不够清晰则退回全分辨率（1/1）
        let mut chosen = 1u32;
        for &d in &[8u32, 4, 2] {
            if (bw.min(bh)) as f64 / d as f64 >= 160.0 {
                chosen = d;
                break;
            }
        }
        // 先读头拿原尺寸
        let probe = std::fs::File::open(&photo_path).map_err(|e| format!("打开原图失败: {e}"))?;
        let mut head = jpeg_decoder::Decoder::new(std::io::BufReader::new(probe));
        let _ = head.read_info();
        let info = head.info().ok_or("无法读取图片头信息")?;
        let (w0, h0) = (info.width as u32, info.height as u32);
        // bbox 在显示坐标系 → 降采样目标尺寸也按转正后的宽高来请求，
        // 否则 6/8 方向会向解码器要错的长宽比（拿到的是错轴的缩放图）。
        let (ori_w, ori_h) = crate::exif_orient::oriented_dims(w0, h0, orientation);
        let tw = ((ori_w + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;
        let th = ((ori_h + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;
        let file2 = std::fs::File::open(&photo_path).map_err(|e| format!("打开原图失败: {e}"))?;
        let mut dec2 = jpeg_decoder::Decoder::new(std::io::BufReader::new(file2));
        let _ = dec2.scale(tw, th);
        let pixels = dec2.decode().map_err(|e| format!("JPEG 解码失败: {e:?}"))?;
        let info2 = dec2.info().ok_or("无法读取解码信息")?;
        let aw = (info2.width as u32).max(1);
        let ah = (info2.height as u32).max(1);
        // 解码 → 按 EXIF 转正（在小图上转，省 CPU/内存）
        let decoded = crate::exif_orient::apply_orientation(
            image::DynamicImage::ImageRgb8(
                image::RgbImage::from_raw(aw, ah, pixels).ok_or("像素数据长度不符")?,
            ),
            orientation,
        );
        // 转正后的实际尺寸，用于把 bbox（显示坐标系）映射到解码图
        let (fw, fh) = (decoded.width().max(1), decoded.height().max(1));
        let sw = fw as f64 / ori_w.max(1) as f64;
        let sh = fh as f64 / ori_h.max(1) as f64;
        let cx = (((x1 - dx).max(0)) as f64 * sw) as u32;
        let cy = (((y1 - dy).max(0)) as f64 * sh) as u32;
        let cw = ((bw + 2 * dx) as f64 * sw).min((fw - cx.min(fw)) as f64).max(1.0) as u32;
        let ch = ((bh + 2 * dy) as f64 * sh).min((fh - cy.min(fh)) as f64).max(1.0) as u32;
        decoded.crop_imm(cx.min(fw), cy.min(fh), cw, ch)
    } else {
        // 按**内容**解码（image::open 按扩展名猜格式，PNG 内容叫 .jpg 时会报 Illegal start bytes）
        let img = crate::imgio::open_any(std::path::Path::new(&photo_path))?;
        let img = crate::exif_orient::apply_orientation(img, orientation);
        let cx1 = (x1 - dx).max(0) as u32;
        let cy1 = (y1 - dy).max(0) as u32;
        img.crop_imm(
            cx1,
            cy1,
            (bw + 2 * dx).min(img.width().saturating_sub(cx1) as i64).max(1) as u32,
            (bh + 2 * dy).min(img.height().saturating_sub(cy1) as i64).max(1) as u32,
        )
    };

    let avatar = dynamic_img.resize_exact(96, 96, image::imageops::FilterType::Triangle);
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建头像目录失败: {e}"))?;
    }
    avatar
        .save_with_format(cache_path, image::ImageFormat::Jpeg)
        .map_err(|e| format!("写头像缓存失败: {e}"))
}

/// FEAT-124：人物卡封面 —— 4:5 竖版，以人脸框为中心、四周外扩 35%
///
/// 与 [`crop_avatar_local`]（96×96 圆头像）的分工：那个给合并弹窗/小圆片用，
/// 这个给人物卡当「照片封面」—— 用户要的是「显示照片或照片的裁切」，不是头像。
///
/// 与头像共用同一条解码纪律（BUG-2026-1027-004）：JPEG 走 `jpeg_decoder` 降采样快路径，
/// 其余格式一律走 `imgio::open_any` 按**魔数**解码（不信扩展名）。
pub fn crop_face_cover_local(pid: &str, cache_path: &Path) -> Result<(), String> {
    const OUT_W: u32 = 480;
    const OUT_H: u32 = 600;
    /// 封面宽高比（4:5）
    const RATIO: f64 = 4.0 / 5.0;
    /// 人脸边长之外的留白比例（0.35 → 裁切框约为脸的 1.7 倍）
    const EXPAND: f64 = 0.35;

    let Some(conn) = open_db()? else {
        return Err("人物注册表不存在".into());
    };
    let (photo_path, bbox_raw) =
        representative_face(&conn, pid).ok_or_else(|| format!("人物无登记人脸: {pid}"))?;
    if !Path::new(&photo_path).is_file() {
        return Err(format!("代表脸原图不存在: {photo_path}"));
    }
    let (x1, y1, x2, y2) =
        parse_bbox(&bbox_raw).ok_or_else(|| format!("bbox 格式异常: {bbox_raw}"))?;
    let orientation = crate::exif_orient::read_orientation(Path::new(&photo_path));
    let is_jpeg = crate::avatar::is_jpeg_file(Path::new(&photo_path));

    // 解码成「显示坐标系」的图（已按 EXIF 转正），同时拿到 显示坐标 → 解码坐标 的缩放比
    let (decoded, sx, sy) = if is_jpeg {
        use std::io::BufReader;
        let probe = std::fs::File::open(&photo_path).map_err(|e| format!("打开原图失败: {e}"))?;
        let mut head = jpeg_decoder::Decoder::new(BufReader::new(probe));
        let _ = head.read_info();
        let info = head.info().ok_or("无法读取图片头信息")?;
        let (w0, h0) = (info.width as u32, info.height as u32);
        let (ow, oh) = crate::exif_orient::oriented_dims(w0, h0, orientation);
        // 降采样档：转正后短边降到 >= 封面长边（600）就够用，不必全解码
        let mut chosen = 1u32;
        for &d in &[8u32, 4, 2] {
            if (ow.min(oh)) as f64 / d as f64 >= OUT_H as f64 {
                chosen = d;
                break;
            }
        }
        let tw = ((ow + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;
        let th = ((oh + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;
        let file2 = std::fs::File::open(&photo_path).map_err(|e| format!("打开原图失败: {e}"))?;
        let mut dec = jpeg_decoder::Decoder::new(BufReader::new(file2));
        let _ = dec.scale(tw, th);
        let pixels = dec.decode().map_err(|e| format!("JPEG 解码失败: {e:?}"))?;
        let info2 = dec.info().ok_or("无法读取解码信息")?;
        let (aw, ah) = ((info2.width as u32).max(1), (info2.height as u32).max(1));
        let img = crate::exif_orient::apply_orientation(
            image::DynamicImage::ImageRgb8(
                image::RgbImage::from_raw(aw, ah, pixels).ok_or("像素数据长度不符")?,
            ),
            orientation,
        );
        let (fw, fh) = (img.width().max(1), img.height().max(1));
        let sx = fw as f64 / ow.max(1) as f64;
        let sy = fh as f64 / oh.max(1) as f64;
        (img, sx, sy)
    } else {
        // 非 JPEG（含「PNG 内容叫 .jpg」那批）：按魔数解码后转正，坐标与显示坐标系 1:1
        let raw = crate::imgio::open_any(Path::new(&photo_path))?;
        let img = crate::exif_orient::apply_orientation(raw, orientation);
        (img, 1.0, 1.0)
    };

    let (fw, fh) = (decoded.width().max(1), decoded.height().max(1));
    let (cx, cy, cw, ch) = cover_rect(
        fw,
        fh,
        (x1 as f64 * sx, y1 as f64 * sy, x2 as f64 * sx, y2 as f64 * sy),
        RATIO,
        EXPAND,
    );
    let cover = decoded
        .crop_imm(cx, cy, cw, ch)
        .resize_exact(OUT_W, OUT_H, image::imageops::FilterType::Triangle);
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建封面目录失败: {e}"))?;
    }
    cover
        .save_with_format(cache_path, image::ImageFormat::Jpeg)
        .map_err(|e| format!("写封面缓存失败: {e}"))
}

/// 以人脸框为中心算一个固定宽高比的裁切矩形（夹紧到图内，永不越界）
///
/// 垂直方向略上移 6%：人像封面的视觉重心在眼睛附近，居中会让头顶贴边。
fn cover_rect(
    w: u32,
    h: u32,
    bbox: (f64, f64, f64, f64),
    ratio: f64,
    expand: f64,
) -> (u32, u32, u32, u32) {
    let (x1, y1, x2, y2) = bbox;
    let face = ((x2 - x1).abs()).max((y2 - y1).abs()).max(1.0);
    let mut cw = face * (1.0 + 2.0 * expand);
    let mut ch = cw / ratio.max(0.01);
    // 先按比例缩到图内（保持宽高比）
    if cw > w as f64 {
        let s = w as f64 / cw;
        cw *= s;
        ch *= s;
    }
    if ch > h as f64 {
        let s = h as f64 / ch;
        cw *= s;
        ch *= s;
    }
    let fx = (x1 + x2) / 2.0;
    let fy = (y1 + y2) / 2.0 - ch * 0.06;
    let cx = (fx - cw / 2.0).clamp(0.0, (w as f64 - cw).max(0.0));
    let cy = (fy - ch / 2.0).clamp(0.0, (h as f64 - ch).max(0.0));
    (
        cx.round() as u32,
        cy.round() as u32,
        cw.round().max(1.0) as u32,
        ch.round().max(1.0) as u32,
    )
}

/// FEAT-047：人物自选头像 —— 用户指定一张照片作为头像封面
///
/// 中心方裁 96×96 JPEG（与自动裁剪同尺寸）写入 cache_path；覆盖后
/// `get_person_avatar` 的 is_file() 缓存判断自然命中自选结果，
/// 优先于 crop_avatar_local 的代表脸自动裁剪。
/// photo_path 的归属由调用方保证（人物照片弹窗内来源），这里只负责落盘。
pub fn set_avatar_from_photo(photo_path: &Path, cache_path: &Path) -> Result<(), String> {
    crate::avatar::crop_square(photo_path, cache_path, 96)
}


pub mod commands {
use tauri::Manager;

// =====================================================================
// 以下命令自 lib.rs 迁入（lib.rs 瘦身）：人物页命令层
// =====================================================================


/// 人物照片条目 —— 对应前端 `PersonPhotoItem`：
///  - path: 原图绝对路径
///  - thumb: 已算好的网格缩略图缓存路径（生成失败/未识别相册时为 None）
///  - album_id: 照片归属相册（解析失败为 None）
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonPhotoItem {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<i64>,
}


/// FEAT-124：取一张人物卡封面（4:5 照片裁切）
///
/// 命中且签名一致 → 直接返回；否则现场裁切。与 `get_person_avatar` 同构，
/// 只是尺寸/用途不同（480×600 给卡片当照片，不是 96×96 头像）。
#[tauri::command]
pub async fn get_person_cover(
    pid: String,
    force_refresh: Option<bool>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let dir = crate::avatar::commands::avatars_dir(&app)?;
    let (path, _sig) = crate::avatar::cover_paths(&dir, &pid);
    if !force_refresh.unwrap_or(false)
        && crate::avatar::cover_cache_valid(&dir, &pid, crate::persons::face_sig(&pid)?.as_deref())
    {
        return Ok(path.to_string_lossy().into_owned());
    }
    let _t = log_call!("get_person_cover", &format!("pid={pid}"));
    let dir2 = dir.clone();
    let pid2 = pid.clone();
    let out = path.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        crate::persons::crop_face_cover_local(&pid2, &out)?;
        crate::avatar::write_cover_sig(&dir2, &pid2);
        Ok::<String, String>(out.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| format!("封面任务线程失败: {e}"))?;
    match &r {
        Ok(_) => crate::logger::log_call_end_with("get_person_cover", _t, "OK | cropped"),
        Err(e) => crate::logger::log_call_end_with("get_person_cover", _t, &format!("ERR | {e}")),
    }
    r
}

/// FEAT-124：批量取人物卡封面（只查本地缓存）
///
/// 与 `get_person_avatars_bulk` 同一套纪律：
/// - 命中（签名一致）→ 返回路径；
/// - 未命中 → 返回 null，并**排队后台裁切**，完成后 emit `person-cover-refreshed`，
///   前端渐进替换 —— 933 人现场裁一轮要 40~75 s，不能阻塞列表首屏。
#[tauri::command]
pub async fn get_person_covers_bulk(
    pids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<Vec<Option<String>>, String> {
    let _t = log_call!("get_person_covers_bulk", &format!("count={}", pids.len()));
    let dir = crate::avatar::commands::avatars_dir(&app)?;
    let dir2 = dir.clone();
    let list = pids.clone();
    let (out, stale) = tauri::async_runtime::spawn_blocking(move || {
        let sigs: std::collections::HashMap<String, Option<String>> =
            crate::persons::persons_with_sig().unwrap_or_default().into_iter().collect();
        let mut out: Vec<Option<String>> = Vec::with_capacity(list.len());
        let mut stale: Vec<String> = Vec::new();
        for pid in &list {
            let (jpg, _sig) = crate::avatar::cover_paths(&dir2, pid);
            let cur = sigs.get(pid).and_then(|s| s.as_deref());
            if crate::avatar::cover_cache_valid(&dir2, pid, cur) {
                out.push(Some(jpg.to_string_lossy().into_owned()));
            } else {
                out.push(None);
                if sigs.contains_key(pid) {
                    stale.push(pid.clone());
                }
            }
        }
        (out, stale)
    })
    .await
    .map_err(|e| format!("批量封面任务线程失败: {e}"))?;

    let hit = out.iter().filter(|x| x.is_some()).count();
    crate::logger::log_call_end_with(
        "get_person_covers_bulk",
        _t,
        &format!("OK | hit={hit} miss={} stale={}", out.len() - hit, stale.len()),
    );

    if !stale.is_empty() {
        let dir3 = dir.clone();
        let app3 = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let t0 = std::time::Instant::now();
            let mut fixed = 0usize;
            for pid in &stale {
                let (jpg, _sig) = crate::avatar::cover_paths(&dir3, pid);
                if crate::persons::crop_face_cover_local(pid, &jpg).is_ok() {
                    crate::avatar::write_cover_sig(&dir3, pid);
                    fixed += 1;
                    let _ = tauri::Emitter::emit(
                        &app3,
                        "person-cover-refreshed",
                        crate::avatar::CoverRefreshed {
                            pid: pid.clone(),
                            path: jpg.to_string_lossy().into_owned(),
                        },
                    );
                }
            }
            crate::logger::log_info(&format!(
                "[avatar] 后台裁切封面完成：{fixed}/{} 张，{}ms",
                stale.len(),
                t0.elapsed().as_millis()
            ));
        });
    }
    Ok(out)
}

/// 读取某人物出现的全部照片：直接查询已算好的缩略图缓存地址；
/// 未缓存的照片现场补齐（BUG-2026-0916-001，原文档“不重新运算缩略图”已过时）。
///
/// 必须 async + spawn_blocking（BUG-2026-0910-003）：大人物全缺图时现场生成
/// 缩略图实测 1.7s（884 张），同步命令跑在主线程会冻结全部窗口——所有界面卡死、
/// 日志副窗口连关闭都无响应。
#[tauri::command]
pub async fn get_person_photos(
    pid: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<Vec<PersonPhotoItem>, String> {
    let _t = log_call!("get_person_photos", &format!("pid={pid}"));
    let user_id = crate::require_user(&session)?;
    let paths = crate::persons::list_person_photos(&pid)?;
    // 短锁取归属数据（MutexGuard 不得跨 spawn_blocking 边界，同 get_photo_thumbs 模式）
    let albums: Vec<(i64, String)> = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_albums(user_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|a| (a.id, a.path))
            .collect()
    };
    let thumbs_dir = crate::thumbs_dir(&app).ok();
    // 重活（解码原图→缩放→编码→落盘）放阻塞线程执行
    let (out, generated, unresolved, gen_failed, unresolved_samples, cached_count) =
        tauri::async_runtime::spawn_blocking(move || {
            build_person_photo_items(paths, albums, thumbs_dir)
        })
        .await
        .map_err(|e| format!("人物照片任务线程失败: {e}"))?;
    let unresolved_hint = if unresolved_samples.is_empty() {
        String::new()
    } else {
        format!(" | 无归属样例: {:?}", unresolved_samples)
    };
    crate::logger::log_call_end_with(
        "get_person_photos",
        _t,
        &format!(
            "OK | n={} thumb_hit={} generated={generated} unresolved={unresolved} gen_failed={gen_failed}{unresolved_hint}",
            out.len(),
            cached_count.saturating_sub(generated),
        ),
    );
    Ok(out)
}


/// `get_person_photos` 的重活部分（纯文件系统 + CPU），由 spawn_blocking 调用。
/// 返回 (items, generated, unresolved, gen_failed, 无归属样例, 缓存命中数)
pub fn build_person_photo_items(
    paths: Vec<String>,
    albums: Vec<(i64, String)>,
    thumbs_dir: Option<std::path::PathBuf>,
) -> (Vec<PersonPhotoItem>, usize, usize, usize, Vec<String>, usize) {
    let mut out = Vec::with_capacity(paths.len());
    let mut generated = 0usize;
    let mut unresolved = 0usize;
    let mut gen_failed = 0usize;
    let mut unresolved_samples: Vec<String> = Vec::new();
    for path in paths {
        // 解析归属相册 → 计算缩略图缓存名 → 若存在直接复用
        let resolved: Option<i64> = albums
            .iter()
            .filter(|(_, ap)| crate::persons::commands::p_is_under(ap, &path))
            .max_by_key(|(_, ap)| ap.len())
            .map(|(id, _)| *id);
        if resolved.is_none() {
            unresolved += 1;
            if unresolved_samples.len() < 3 {
                unresolved_samples.push(path.clone());
            }
        }
        // BUG-2026-0916-001 修复：归属解析失败（相册记录被删 / 路径变更 / 大小写差异）
        // 不代表原图不可用 —— 以 album_id=0 的「无归属缓存命名空间」照常生成/复用
        // 缩略图，避免这类照片永久占位。album_id 字段保持 None（前端 Lightbox 对
        // 无归属照片走系统打开器兑底）。
        let thumb_album = resolved.unwrap_or(0);
        let cached = (|| {
            let thumbs = thumbs_dir.as_ref()?;
            let name = crate::thumbnail::grid_thumb_cache_name(thumb_album, std::path::Path::new(&path));
            let tp = thumbs.join("grid").join(&name);
            if tp.is_file() {
                return Some(tp.to_string_lossy().to_string());
            }
            // 缺图 → 调用 ensure_grid_thumb 补齐（256px 生成后落盘，返回缓存路径），
            // 后续任何场景（PhotoGrid/Timeline/Memories/智能搜索）再访问都直接命中。
            // 本路径（人物照片）不写表（不在主流程加锁），保持与旧版兼容。
            match crate::thumbnail::ensure_grid_thumb(
                thumb_album,
                std::path::Path::new(&path),
                thumbs,
                None,
                0,
            ) {
                Ok(p) => {
                    generated += 1;
                    Some(p)
                }
                Err(e) => {
                    gen_failed += 1;
                    crate::logger::log_error(
                        "get_person_photos",
                        &format!("缩略图补齐失败: {path} | {e:?}"),
                    );
                    None
                }
            }
        })();
        out.push(PersonPhotoItem {
            path,
            thumb: cached,
            album_id: resolved,
        });
    }
    let cached_count = out.iter().filter(|i| i.thumb.is_some()).count();
    (out, generated, unresolved, gen_failed, unresolved_samples, cached_count)
}


/// 判断照片路径是否位于相册目录之下（目录是祖先，且照片不是目录本身）。
///
/// Windows 归一化比较：分隔符 `/`→`\\` 统一 + 大小写不敏感（NTFS 不区分大小写）
/// + 前缀边界校验（避免 `D:\\a` 误匹配 `D:\\ab\\c.jpg`）。历史实现用
/// `strip_prefix` 严格区分大小写/分隔符，相册路径与 faces 记录不一致时会把
/// 存在的原图误判为「无归属」（BUG-2026-0916-001）。
pub fn p_is_under(dir: &str, photo: &str) -> bool {
    fn norm(p: &str) -> String {
        p.replace('/', "\\").to_lowercase()
    }
    let d = norm(dir);
    let d = d.trim_end_matches('\\');
    let ph = norm(photo);
    if d.is_empty() || ph.len() <= d.len() {
        return false;
    }
    if !ph.starts_with(d) {
        return false;
    }
    // 前缀边界：目录后必须是分隔符，且照片还有非空文件部分
    ph.len() > d.len() + 1 && ph[d.len()..].starts_with('\\')
}


/// 人物注册表：列出全部已标号人物（直读 persons.db，按脸数降序；不依赖微服务）
#[tauri::command]
pub fn list_persons() -> Result<Vec<crate::persons::PersonEntry>, String> {
    let _t = log_call!("list_persons", "db-direct");
    let r = crate::persons::list_persons();
    match &r {
        Ok(list) => crate::logger::log_call_end_with("list_persons", _t, &format!("OK | n={}", list.len())),
        Err(e) => crate::logger::log_call_end_with("list_persons", _t, &format!("ERR | {e}")),
    }
    r
}


/// 人物注册表：列出在指定相册目录下出现过的人物（BUG-2026-0919-003）。
///
/// 相册详情页的扫描面板用本命令替代全局 list_persons：人物卡片计数为
/// 「该人物在当前相册中出现的照片数」，其他相册的人物不再混入。
#[tauri::command]
pub fn list_persons_in_album(album_path: String) -> Result<Vec<crate::persons::PersonEntry>, String> {
    let _t = log_call!("list_persons_in_album", &format!("album={album_path}"));
    let r = crate::persons::list_persons_in_album(&album_path);
    match &r {
        Ok(list) => crate::logger::log_call_end_with("list_persons_in_album", _t, &format!("OK | n={}", list.len())),
        Err(e) => crate::logger::log_call_end_with("list_persons_in_album", _t, &format!("ERR | {e}")),
    }
    r
}


/// 人物注册表：列出某人物出现的全部照片路径（直读 persons.db；供前端展示缩略图）
#[tauri::command]
pub fn list_person_photos(pid: String) -> Result<Vec<String>, String> {
    let _t = log_call!("list_person_photos", &format!("pid={pid}"));
    let r = crate::persons::list_person_photos(&pid);
    match &r {
        Ok(list) => crate::logger::log_call_end_with("list_person_photos", _t, &format!("OK | n={}", list.len())),
        Err(e) => crate::logger::log_call_end_with("list_person_photos", _t, &format!("ERR | {e}")),
    }
    r
}


/// FEAT-067 步骤 4：人物改名/合并后，**后台**增量重算受影响的描述向量
///
/// 不做在命令主线程里：改名本身是毫秒级写库，重算要走文本塔（首次含模型懒加载），
/// 卡住 UI 不可接受。重建按 `source_hash` 判定增量，只有真名变化的那批照片会被重算。
pub fn spawn_desc_rebuild(app: &tauri::AppHandle, user_id: i64, reason: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<crate::AppState>();
        match crate::textdesc::rebuild_text_index(&app, &state, user_id).await {
            Ok(rep) => crate::logger::log_info(&format!(
                "[textdesc] {} 触发重建：built={} unchanged={} empty={} {}ms",
                reason, rep.built, rep.unchanged, rep.empty, rep.ms
            )),
            Err(e) => crate::logger::log_info(&format!("[textdesc] {reason} 触发重建跳过（{e}）")),
        }
    });
}


/// 人物注册表：重命名人物（直写 persons.db）
///
/// FEAT-067：改名会改变「描述里的真名」→ 改完后台增量重算该人物名下照片的描述向量
#[tauri::command]
pub fn rename_person(
    pid: String,
    name: String,
    app: tauri::AppHandle,
    session: tauri::State<crate::SessionState>,
) -> Result<(), String> {
    let _t = log_call!("rename_person", &format!("pid={pid}"));
    let user_id = crate::require_user(&session)?;
    let r = crate::persons::rename_person(&pid, &name);
    if r.is_ok() {
        spawn_desc_rebuild(&app, user_id, format!("rename {pid}"));
    }
    match &r {
        Ok(_) => crate::logger::log_call_end_with("rename_person", _t, "OK"),
        Err(e) => crate::logger::log_call_end_with("rename_person", _t, &format!("ERR | {e}")),
    }
    r
}


/// 人物注册表：合并人物（source 并入 target；直写 persons.db，质心加权平均与 Python 逻辑一致）
///
/// FEAT-067：合并同样改变人物真名归属 → 后台增量重算
#[tauri::command]
pub fn merge_persons(
    target: String,
    source: String,
    app: tauri::AppHandle,
    session: tauri::State<crate::SessionState>,
) -> Result<(), String> {
    let _t = log_call!("merge_persons", &format!("target={target} source={source}"));
    let user_id = crate::require_user(&session)?;
    let r = crate::persons::merge_persons(&target, &source);
    if r.is_ok() {
        // source 已被删除：它的头像缓存（含签名/自选标记）一并清掉，
        // 否则 P 编号被后续新建复用时会拿旧头像冒充（BUG-2026-1001-001 同类）。
        if let Ok(dir) = crate::avatar::commands::avatars_dir(&app) {
            crate::avatar::remove_avatar_cache(&dir, &source);
        }
        // target 人脸集已变 → 签名失配，下次读取自动重裁（前端还会 force 一次）
        spawn_desc_rebuild(&app, user_id, format!("merge {source}->{target}"));
    }
    match &r {
        Ok(_) => crate::logger::log_call_end_with("merge_persons", _t, "OK"),
        Err(e) => crate::logger::log_call_end_with("merge_persons", _t, &format!("ERR | {e}")),
    }
    r
}


/// 人物注册表：删除人物（直写 persons.db，离线可用；同步清理头像缓存）
#[tauri::command]
pub fn delete_person(
    pid: String,
    app: tauri::AppHandle,
    session: tauri::State<crate::SessionState>,
) -> Result<(), String> {
    let _t = log_call!("delete_person", &format!("pid={pid}"));
    crate::require_user(&session)?;
    let r = crate::persons::delete_person(&pid);
    if r.is_ok() {
        // 头像缓存文件已无意义，一并清理（含 .sig / .custom 伴随文件）
        if let Ok(dir) = crate::avatar::commands::avatars_dir(&app) {
            crate::avatar::remove_avatar_cache(&dir, &pid);
        }
        crate::logger::log_call_end_with("delete_person", _t, "OK");
    } else if let Err(e) = &r {
        crate::logger::log_call_end_with("delete_person", _t, &format!("ERR | {e}"));
    }
    r
}


/// 人物注册表：人工修正人脸归属（移出此人 / 移到已有的人 / 移到新建的人）
///
/// 三种调用方式：
/// - `to_pid = null, new_name = null`：移出全部人物（这张脸不再属于任何人）
/// - `to_pid = "P003"`：移到已有的人物
/// - `new_name = "李四"`：新建一个人物并移入
///
/// 同时写 `face_overrides`，保证**重新扫描不会把修正覆盖回去**；
/// 并同步 `photo_content_scan.person_ids`（网格徽标 / 人物筛选 / 描述文本读它）。
#[tauri::command]
pub fn move_person_faces(
    from_pid: String,
    to_pid: Option<String>,
    new_name: Option<String>,
    paths: Vec<String>,
    app: tauri::AppHandle,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<crate::persons::FaceMoveOutcome, String> {
    let _t = log_call!(
        "move_person_faces",
        &format!("from={from_pid} to={to_pid:?} new={new_name:?} paths={}", paths.len())
    );
    let user_id = crate::require_user(&session)?;
    let r = crate::persons::move_faces(&from_pid, to_pid.as_deref(), new_name.as_deref(), &paths);
    match &r {
        Ok(outcome) => {
            if outcome.moved > 0 {
                // 1. 内容表 person_ids 同步（失败不回滚人脸：下次扫描会补齐）
                if let Ok(db) = state.0.lock() {
                    let _ = db.sync_person_ids_for_paths(
                        &paths,
                        Some(&from_pid),
                        outcome.to_pid.as_deref(),
                    );
                }
                // 2. 头像缓存：两边的人脸集都变了，签名失配，下次读取自动重裁
                if let Ok(dir) = crate::avatar::commands::avatars_dir(&app) {
                    crate::avatar::remove_avatar_cache(&dir, &from_pid);
                    if let Some(t) = &outcome.to_pid {
                        crate::avatar::remove_avatar_cache(&dir, t);
                    }
                }
                // 3. 描述文本里的真名变了 → 后台增量重建（与改名同一套机制）
                spawn_desc_rebuild(
                    &app,
                    user_id,
                    format!("move faces {from_pid} -> {:?}", outcome.to_pid),
                );
            }
            crate::logger::log_call_end_with(
                "move_person_faces",
                _t,
                &format!("OK | moved={} created={:?}", outcome.moved, outcome.created_pid),
            );
        }
        Err(e) => crate::logger::log_call_end_with("move_person_faces", _t, &format!("ERR | {e}")),
    }
    r
}


/// 人物注册表：重建人物库（备份 + 清空 + 清头像缓存）——换识别模型后的显式动作
///
/// 为什么必须存在：ArcFace 换模型 = 换嵌入空间，旧质心与新向量不可比（余弦会
/// 退化成随机值），继续扫只会把同一个人拆成一堆新 P 编号。Python 侧 register()
/// 会直接报错、宿主扫描前也会预检拦截；本命令是用户确认后的“正路”：备份库 →
/// 清空 persons/faces/meta → 清头像缓存 → 提示重新全量扫描重建人物。
///
/// 备份文件与库同目录（persons.db.bak-<unix 秒>），失败不删原库。
#[tauri::command]
pub async fn rebuild_person_registry(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<serde_json::Value, String> {
    let _t = log_call!("rebuild_person_registry", "");
    crate::require_user(&session)?;
    // 重活（备份 50MB 库 + 清表）放阻塞线程，不占异步 worker
    let r = tauri::async_runtime::spawn_blocking(crate::persons::reset_registry)
        .await
        .map_err(|e| format!("重建人物库任务线程失败: {e}"))?;
    let (backup, persons, faces) = match r {
        Ok(v) => v,
        Err(e) => {
            crate::logger::log_call_end_with("rebuild_person_registry", _t, &format!("ERR | {e}"));
            return Err(e);
        }
    };
    // 内容表：先把「人脸字段非空」的照片登记进待重做队列，再清空那两个字段。
    // 只清字段而不入队 = 以后增量扫描会把它们当“已入库无变化”全部跳过，
    // 人脸永远不会回来（BUG-2026-1001-015，用户现场：“重建后增量扫描依然跳过所有”）。
    let (queued, content_rows) = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        let queued = db
            .face_rebuild_mark_from_content()
            .map_err(|e| format!("登记待重做人脸失败: {e}"))?;
        let rows = db
            .conn()
            .execute(
                "UPDATE photo_content_scan SET person_ids = NULL, person_count = 0 \
                 WHERE person_ids IS NOT NULL AND person_ids != ''",
                [],
            )
            .unwrap_or(0);
        (queued, rows)
    };
    // 头像缓存全清：P 编号即将重新分配，旧文件一律不可信
    let cleared = crate::avatar::commands::avatars_dir(&app)
        .map(|dir| crate::avatar::clear_all_avatar_cache(&dir))
        .unwrap_or(0);
    crate::logger::log_call_end_with(
        "rebuild_person_registry",
        _t,
        &format!(
            "OK | 清空 persons={persons} faces={faces} 头像={cleared} 内容行人脸字段={content_rows} 待重做入队={queued} | 备份 {backup}"
        ),
    );
    Ok(serde_json::json!({
        "backup": backup,
        "persons": persons,
        "faces": faces,
        "avatars_cleared": cleared,
        "content_rows_cleared": content_rows,
        "face_rebuild_queued": queued,
    }))
}

}

#[cfg(test)]
mod tests {
    use super::*;

    /// 人脸集签名格式："张数:最大id"——头像缓存身份判据的事实来源
    #[test]
    fn test_face_sig_format_from_sql() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE faces (id INTEGER PRIMARY KEY AUTOINCREMENT, person_id TEXT);\
             INSERT INTO faces(person_id) VALUES('P001'),('P001'),('P002');",
        )
        .unwrap();
        assert_eq!(face_sig_of(&conn, "P001").as_deref(), Some("2:2"));
        assert_eq!(face_sig_of(&conn, "P002").as_deref(), Some("1:3"));
        // 无脸人物：COUNT=0 也应给出稳定签名（与“查不到人物”区分）
        assert_eq!(face_sig_of(&conn, "P999").as_deref(), Some("0:0"));
        // 新增一张脸 → 签名必变（缓存失效判据成立）
        conn.execute("INSERT INTO faces(person_id) VALUES('P001')", [])
            .unwrap();
        assert_eq!(face_sig_of(&conn, "P001").as_deref(), Some("3:4"));
    }

    /// 代表脸 = 与质心最相似的一张（而非最早登记）
    #[test]
    fn test_representative_face_picks_closest_to_centroid() {
        fn blob(v: [f32; 4]) -> Vec<u8> {
            v.iter().flat_map(|f| f.to_le_bytes()).collect()
        }
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE persons (id TEXT PRIMARY KEY, centroid BLOB, face_count INTEGER);\
             CREATE TABLE faces (id INTEGER PRIMARY KEY AUTOINCREMENT, person_id TEXT, \
                photo_path TEXT, bbox TEXT, embedding BLOB, created_at TEXT);",
        )
        .unwrap();
        // 质心 = (1,0,0,0)；三张脸按登记顺序：完全同向 / 偏 30° / 偏 45°，
        // 且面积递增（后两张更大）—— 期望选中“完全同向”的第一张（相似度优先于面积）。
        conn.execute(
            "INSERT INTO persons VALUES('P001', ?1, 3)",
            rusqlite::params![blob([1.0, 0.0, 0.0, 0.0])],
        )
        .unwrap();
        let rows = [
            ("a.jpg", "(0, 0, 40, 40)", [1.0f32, 0.0, 0.0, 0.0], "2026-01-01 00:00:00"),
            ("b.jpg", "(0, 0, 400, 400)", [0.87, 0.5, 0.0, 0.0], "2026-01-02 00:00:00"),
            ("c.jpg", "(0, 0, 900, 900)", [0.7, 0.7, 0.0, 0.0], "2026-01-03 00:00:00"),
        ];
        for (p, bb, e, t) in rows {
            conn.execute(
                "INSERT INTO faces(person_id, photo_path, bbox, embedding, created_at) \
                 VALUES('P001', ?1, ?2, ?3, ?4)",
                rusqlite::params![p, bb, blob(e), t],
            )
            .unwrap();
        }
        let (path, _bbox) = representative_face(&conn, "P001").unwrap();
        assert_eq!(path, "a.jpg", "应选与质心最相似的一张");
    }

    #[test]
    fn test_parse_bbox_formats() {
        assert_eq!(parse_bbox("(1736, 38, 2187, 680)"), Some((1736, 38, 2187, 680)));
        assert_eq!(parse_bbox("[10,20,30,40]"), Some((10, 20, 30, 40)));
        assert_eq!(parse_bbox("no numbers"), None);
        assert_eq!(parse_bbox("(5, 5, 5, 9)"), None); // 零宽非法
    }

    #[test]
    fn test_list_persons_no_db_is_empty() {
        // 不 mock 文件系统：函数对缺失 db 必须返回空列表而非报错
        // （persons_db_path 固定指向项目 data 目录；若本机已有 db 则验证真实读取）
        let r = list_persons();
        match r {
            Ok(list) => {
                // 已有库则应为降序
                for w in list.windows(2) {
                    assert!(w[0].face_count >= w[1].face_count);
                }
            }
            Err(e) => panic!("缺失/存在库都不应报错: {e}"),
        }
    }

    #[test]
    fn test_crop_avatar_local_real_data() {
        // 用真实注册表第一人验证裁剪链路（输出到临时目录，不污染头像缓存）
        let list = match list_persons() {
            Ok(l) => l,
            Err(e) => panic!("列表不应报错: {e}"),
        };
        if list.is_empty() {
            eprintln!("跳过：无人物数据");
            return;
        }
        let pid = &list[0].id;
        let tmp = std::env::temp_dir().join(format!("avatar_test_{pid}.jpg"));
        let _ = std::fs::remove_file(&tmp);
        crop_avatar_local(pid, &tmp).unwrap_or_else(|e| panic!("裁剪失败: {e}"));
        assert!(tmp.is_file());
        let meta = std::fs::metadata(&tmp).expect("应有产物");
        assert!(meta.len() > 100, "JPEG 过小，疑似空图");
        // 能被 image 解码且尺寸为 96×96
        let img = image::open(&tmp).expect("产物应为合法图片");
        assert_eq!((img.width(), img.height()), (96, 96));
        let _ = std::fs::remove_file(&tmp);
    }

    /// FEAT-124：人物卡封面（4:5）在**真实注册表**上的裁剪链路。
    ///
    /// 除了尺寸，还验「以人脸为中心」这个语义：裁切框必须包住人脸框的中心点
    /// （`cover_rect` 的夹紧逻辑曾经很容易把框推偏）。
    #[test]
    fn test_crop_face_cover_local_real_data() {
        let list = match list_persons() {
            Ok(l) => l,
            Err(e) => panic!("列表不应报错: {e}"),
        };
        if list.is_empty() {
            eprintln!("跳过：无人物数据");
            return;
        }
        let mut ok = 0usize;
        let mut samples = 0usize;
        for p in list.iter().take(5) {
            let tmp = std::env::temp_dir().join(format!("cover_test_{}.jpg", p.id));
            let _ = std::fs::remove_file(&tmp);
            match crop_face_cover_local(&p.id, &tmp) {
                Ok(()) => {
                    ok += 1;
                    samples += 1;
                    let img = image::open(&tmp).expect("产物应为合法图片");
                    assert_eq!(
                        (img.width(), img.height()),
                        (480, 600),
                        "封面应为 480×600（4:5）"
                    );
                    assert!(
                        std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) > 1000,
                        "封面文件过小，疑似空图"
                    );
                    let _ = std::fs::remove_file(&tmp);
                }
                // 无登记人脸 / 原图已不在磁盘上的人物跳过（注册表里确实存在这种条目）
                Err(e) if e.contains("人物无登记人脸") || e.contains("代表脸原图不存在") => {}
                Err(e) => panic!("封面裁剪失败: {e}"),
            }
        }
        assert!(samples > 0, "前 5 个人物里一张封面都没裁出来");
        eprintln!("封面裁剪：{ok}/{samples} 成功");
    }

    /// `cover_rect` 纯几何：人脸靠边时不能越界，宽高比恒为 4:5
    #[test]
    fn test_cover_rect_geometry() {
        let ratio = 4.0 / 5.0;
        // 中心人脸
        let (x, y, w, h) = cover_rect(4000, 3000, (1800.0, 1300.0, 2200.0, 1700.0), ratio, 0.35);
        assert!((w as f64 / h as f64 - ratio).abs() < 0.01, "宽高比应为 4:5");
        assert!(x as u32 + w <= 4000 && y as u32 + h <= 3000, "不得越界");
        // 脸部贴左上角：夹紧后仍在图内
        let (x2, y2, w2, h2) = cover_rect(800, 600, (0.0, 0.0, 40.0, 40.0), ratio, 0.35);
        assert!(x2 + w2 <= 800 && y2 + h2 <= 600, "贴边也不得越界");
        assert!(x2 == 0 && y2 == 0, "贴左上角应从原点起裁");
    }
}
