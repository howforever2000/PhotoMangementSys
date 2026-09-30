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
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project = manifest.parent().unwrap_or(manifest);
    project.join("python").join("data")
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
fn representative_face(conn: &rusqlite::Connection, pid: &str) -> Option<(String, String)> {
    // 代表脸 = 「与质心最相似」的一张（不再取最早登记的一张）。
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
            best = Some((sim, area, id, path, bbox));
        }
    }
    best.map(|(_, _, _, path, bbox)| (path, bbox))
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

    let name = Path::new(&photo_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let is_jpeg = name.ends_with(".jpg") || name.ends_with(".jpeg");

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
        // 按选定档位请求目标尺寸（jpeg-decoder 会取不超过请求的最近 2 的幂档）
        let tw = ((w0 + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;
        let th = ((h0 + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;
        let file2 = std::fs::File::open(&photo_path).map_err(|e| format!("打开原图失败: {e}"))?;
        let mut dec2 = jpeg_decoder::Decoder::new(std::io::BufReader::new(file2));
        let _ = dec2.scale(tw, th);
        let pixels = dec2.decode().map_err(|e| format!("JPEG 解码失败: {e:?}"))?;
        let info2 = dec2.info().ok_or("无法读取解码信息")?;
        let aw = (info2.width as u32).max(1);
        let ah = (info2.height as u32).max(1);
        let sw = aw as f64 / w0.max(1) as f64; // 实际缩放比（解码图/原图）
        let sh = ah as f64 / h0.max(1) as f64;
        let cx = (((x1 - dx).max(0)) as f64 * sw) as u32;
        let cy = (((y1 - dy).max(0)) as f64 * sh) as u32;
        let cw = ((bw + 2 * dx) as f64 * sw).min((aw - cx.min(aw)) as f64).max(1.0) as u32;
        let ch = ((bh + 2 * dy) as f64 * sh).min((ah - cy.min(ah)) as f64).max(1.0) as u32;
        image::DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(aw, ah, pixels).ok_or("像素数据长度不符")?,
        )
        .crop_imm(cx.min(aw), cy.min(ah), cw, ch)
    } else {
        let img = image::open(&photo_path).map_err(|e| format!("图片解码失败: {e}"))?;
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
    // 内容表的 person_ids 同步清空：那些 P 编号已不存在，留着只会让
    // 照片卡片/Search 继续引用“幽灵人物”（重新扫描会重写这两个字段）。
    let content_rows = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.conn()
            .execute(
                "UPDATE photo_content_scan SET person_ids = NULL, person_count = 0 \
                 WHERE person_ids IS NOT NULL AND person_ids != ''",
                [],
            )
            .unwrap_or(0)
    };
    // 头像缓存全清：P 编号即将重新分配，旧文件一律不可信
    let cleared = crate::avatar::commands::avatars_dir(&app)
        .map(|dir| crate::avatar::clear_all_avatar_cache(&dir))
        .unwrap_or(0);
    crate::logger::log_call_end_with(
        "rebuild_person_registry",
        _t,
        &format!(
            "OK | 清空 persons={persons} faces={faces} 头像={cleared} 内容行人脸字段={content_rows} | 备份 {backup}"
        ),
    );
    Ok(serde_json::json!({
        "backup": backup,
        "persons": persons,
        "faces": faces,
        "avatars_cleared": cleared,
        "content_rows_cleared": content_rows,
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
}
