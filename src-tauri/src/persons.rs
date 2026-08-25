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

/// persons.db 路径：与 python/vcr/config.py 的 DATA_DIR/persons.db 同源
fn persons_db_path() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project = manifest.parent().unwrap_or(manifest);
    project.join("python").join("data").join("persons.db")
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
    conn.query_row(
        "SELECT photo_path, bbox FROM faces WHERE person_id = ?1 ORDER BY created_at ASC, id ASC LIMIT 1",
        rusqlite::params![pid],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .ok()
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

#[cfg(test)]
mod tests {
    use super::*;

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
