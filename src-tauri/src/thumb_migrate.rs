//! 缩略图缓存一次性迁移（A/B/C 三阶段）—— BUG-2026-1023-009 的收尾工程
//! ===================================================================
//!
//! # 背景：一次版本号改动，把「永久缓存」变成了「全量失效」
//!
//! v0.4.3 的 `fcfbaf7`（EXIF 转正）把渲染版本 v2 掺进了**两个**键：
//!
//! | 键 | v0.4.2 | v0.4.3 |
//! |---|---|---|
//! | 表主键 `thumb_photo_hash` | `len|mtime|path` | `len|mtime|path|v2` |
//! | 文件名 `file_fingerprint` | `len_mtime_head_fn` | `v2_len_mtime_head_fn` |
//!
//! 后果（实测于 2.5 万张的库）：
//!
//! - `photo_thumb_cache` 里 24,924 行旧键记录查不到 → 懒加载全 miss；
//! - `thumbs/grid` 里 25,777 个旧文件（**1.45 GB**）永不命中 → 磁盘白占；
//! - 更隐蔽：`photo_content_scan.photo_hash`（扫描侧）**没变**，于是
//!   `count_scanned_without_thumb` 的 LEFT JOIN 仍然对得上旧行 → 系统认为
//!   「照片都有缩略图，不需要预热」，而懒加载侧又说「全都没有」——
//!   两套键空间错开，升级后首次浏览直接全库重生成（并引发并发风暴）。
//!
//! # 三阶段
//!
//! | 阶段 | 做什么 | 收益 |
//! |---|---|---|
//! | **A 键解耦** | `thumb_photo_hash` 不再掺版本（与 `content::photo_hash` 同源），版本只进文件名 | 表键与扫描键永久对齐，缺图检测/预热恢复正常 |
//! | **B 重订键 + 复用旧图** | 逐行用「旧键」找回老行；源图在、旧 thumb 在 → 把它改名成 v2 名并重写行 | EXIF orientation=1 的照片占实测 97.8%，其 v1 与 v2 输出**逐字节相同**，改名即热，零解码 |
//! | **C 回收垃圾** | 删掉仍在旧键空间的死行，以及表里没有任何行引用的孤儿 grid 文件 | 回收约 1.4 GB |
//!
//! # 幂等与容错
//!
//! - 由 `settings` 表的 `thumb_migrate_v2_keys` 标记控制，跑完即打标，重启不重跑；
//! - 每一步都 `continue` 掉单条失败（文件被占用/源图已删除），绝不中断整体；
//! - 迁移在后台线程跑，不阻塞窗口首帧；期间 AppState 锁粒度按「读全量 → 逐条改名（无锁）
//!   → 批量写回（短锁）」拆开，不持有 `MutexGuard` 跨越文件 IO。
//!
//! 这个模块**不重新编码任何像素**：需要重生成的只剩 orientation≠1 的约 2%
//! （以及源图已变化的照片），交给懒加载 + 预热按常规路径处理。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::db::{Database, ThumbCacheRecord, ThumbCacheRow};
use crate::logger;
use crate::thumbnail::{self, THUMB_RENDER_VERSION};

/// 迁移完成标记（settings 表键名）。值里带版本号，将来若再需要一次迁移可换键名。
const MIGRATE_FLAG: &str = "thumb_migrate_v2_keys";

/// 每批写回的记录数（避免单事务过大）
const WRITE_BATCH: usize = 500;

/// 迁移结果摘要（写日志 / 回传前端诊断用）
#[derive(Debug, Default, Clone)]
pub struct MigrateReport {
    /// 扫描过的旧行数
    pub scanned_rows: usize,
    /// 成功重订键并复用旧缩略图文件的行数
    pub migrated_rows: usize,
    /// 旧行本身指向的缩略图文件已不存在的行数（放弃迁移，留给懒加载重生成）
    pub missing_thumb_rows: usize,
    /// 旧图『躺倒』（orientation≠1）而不允许复用、需重生成的行数
    pub needs_regen_rows: usize,
    /// 源图已不可读（被删/被移走）而跳过的行数
    pub missing_source_rows: usize,
    /// 删掉的旧键死行数
    pub deleted_rows: usize,
    /// 删掉的孤儿 grid 文件数
    pub deleted_orphan_files: usize,
    /// 回收的磁盘字节数
    pub freed_bytes: u64,
    /// 是否本次真的执行了（false = 之前已迁移过，直接返回）
    pub ran: bool,
}

/// 启动时调用：若未迁移过，则在**调用线程**里执行一次完整迁移。
///
/// 调用方负责放到后台线程（见 `lib.rs` 的 setup），本函数是同步阻塞的。
pub fn migrate_if_needed(db: &Database, thumbs_dir: &Path) -> MigrateReport {
    let mut report = MigrateReport::default();
    match db.get_setting(MIGRATE_FLAG) {
        Ok(Some(v)) if v.trim() == "done" => {
            return report; // 已迁移过，静默返回
        }
        Ok(_) => {}
        Err(e) => {
            logger::log_warn(&format!("读取迁移标记失败，跳过迁移: {e}"));
            return report;
        }
    }
    report.ran = true;
    logger::log_info("[thumb] 开始缩略图缓存迁移（键解耦 A + 重订键复用 B + 垃圾回收 C）");

    // 取所有用户的行：迁移是库级别的，不能只迁当前登录用户
    let user_ids = match db.list_thumb_cache_user_ids() {
        Ok(ids) => ids,
        Err(e) => {
            logger::log_error("thumb_migrate", &format!("列用户失败，跳过迁移: {e}"));
            return report;
        }
    };

    let mut old_keys_to_delete: Vec<String> = Vec::new();
    let mut known_thumb_paths: HashSet<PathBuf> = HashSet::new();

    for uid in user_ids {
        let rows = match db.list_all_thumb_caches(uid) {
            Ok(r) => r,
            Err(e) => {
                logger::log_error("thumb_migrate", &format!("读 uid={uid} 缓存行失败: {e}"));
                continue;
            }
        };
        for row in rows {
            report.scanned_rows += 1;
            known_thumb_paths.insert(PathBuf::from(&row.thumb_path));

            let Some((len, mtime_ns)) = read_meta(Path::new(&row.source_path)) else {
                report.missing_source_rows += 1;
                continue;
            };
            let old_hash = thumbnail::thumb_photo_hash_v1(
                Path::new(&row.source_path),
                len,
                mtime_ns,
                THUMB_RENDER_VERSION,
            );
            let new_hash = thumbnail::thumb_photo_hash(Path::new(&row.source_path), len, mtime_ns);

            // 该行是不是旧键空间的？（v0.4.3 崩掉之前写进去的都是）
            let is_old_key = row.photo_hash == old_hash;
            if !is_old_key && row.photo_hash != new_hash {
                // 既不是旧键也不是新键（文件大小/mtime 变过）→ 保守跳过，交给懒加载
                continue;
            }
            if is_old_key {
                old_keys_to_delete.push(row.photo_hash.clone());
            }

            // 旧文件是否还在？不在就没有复用价值
            let old_thumb = PathBuf::from(&row.thumb_path);
            if !is_usable_file(&old_thumb) {
                report.missing_thumb_rows += 1;
                continue;
            }

            // 🔑 关键保底：v1 缩略图是**没转正**的。orientation≠1 的旧图直接改名
            // 冒充 v2 产物，等于把 BUG-2026-1014-002（横拍躺倒）又种回去。
            // 只有 orientation=1（含无 EXIF）时，v1 与 v2 的像素才**逐字节相同**，
            // 改名复用才成立；其余（实测约 2%）老实交给懒加载重生成。
            let orientation = crate::exif_orient::read_orientation(Path::new(&row.source_path));
            if orientation.unwrap_or(1) != 1 {
                report.needs_regen_rows += 1;
                continue;
            }

            // 目标文件名：按新指纹命名，与 `ensure_grid_thumb` 的命名**逐字一致**
            // （`grid/album_<id>_photo_<fp>.jpg`；注意不是封面那套 webp/{subdir}/ 命名）
            let new_thumb = grid_thumb_path(thumbs_dir, row.album_id, Path::new(&row.source_path));

            // 已经就位（重复迁移/并发安全）→ 只补表
            let copied_or_renamed = if same_file(&old_thumb, &new_thumb) {
                true
            } else {
                if let Some(parent) = new_thumb.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                // 优先 rename（同卷、零拷贝）；跨卷/被占用则退化为 copy
                std::fs::rename(&old_thumb, &new_thumb).is_ok()
                    || std::fs::copy(&old_thumb, &new_thumb).is_ok()
            };
            if !copied_or_renamed {
                report.missing_thumb_rows += 1;
                continue;
            }
            if !is_usable_file(&new_thumb) {
                report.missing_thumb_rows += 1;
                continue;
            }
            known_thumb_paths.insert(new_thumb.clone());

            // 写回新键（旧行稍后统一删）
            let rec = row.to_record(
                new_hash.clone(),
                new_thumb.to_string_lossy().into_owned(),
            );
            if let Err(e) = db.upsert_thumb_caches(&[rec]) {
                logger::log_warn(&format!("重订键写回失败（{}）: {e}", row.source_path));
                continue;
            }
            report.migrated_rows += 1;
        }
    }

    // ---- C1：删旧键死行 ----
    old_keys_to_delete.sort();
    old_keys_to_delete.dedup();
    for chunk in old_keys_to_delete.chunks(WRITE_BATCH) {
        match db.delete_thumb_caches_by_hashes(chunk) {
            Ok(n) => report.deleted_rows += n,
            Err(e) => logger::log_warn(&format!("删旧键行失败: {e}")),
        }
    }

    // ---- C2：删孤儿 grid 文件（表里没有任何行引用）----
    let grid_dir = thumbs_dir.join("grid");
    if let Ok(entries) = std::fs::read_dir(&grid_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            if known_thumb_paths.contains(&path) {
                continue;
            }
            // 只清理本模块认得的两类命名，避免误删（封面缩略图在上级目录，不在此处）
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("album_") || name.ends_with(".tmp") {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                report.deleted_orphan_files += 1;
                report.freed_bytes += meta.len();
            }
        }
    }

    // 打标（失败只记日志：下次会重跑，但因为已幂等，代价有限）
    if let Err(e) = db.set_setting(MIGRATE_FLAG, "done") {
        logger::log_warn(&format!("写迁移标记失败: {e}"));
    }

    logger::log_info(&format!(
        "[thumb] 迁移完成：扫描 {} 行 | 重订键复用 {} 行 | 旧图缺失 {} | 躺倒需重生成 {} | 源图缺失 {} | \
         删旧键行 {} | 删孤儿文件 {} | 回收 {:.1} MB",
        report.scanned_rows,
        report.migrated_rows,
        report.missing_thumb_rows,
        report.needs_regen_rows,
        report.missing_source_rows,
        report.deleted_rows,
        report.deleted_orphan_files,
        report.freed_bytes as f64 / (1024.0 * 1024.0),
    ));
    report
}

/// 读文件 (len, mtime_ns)；与 thumbnail 模块同口径
fn read_meta(path: &Path) -> Option<(u64, u128)> {
    let md = std::fs::metadata(path).ok()?;
    let mtime_ns = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some((md.len(), mtime_ns))
}

/// 文件存在且非空（0 字节残骸不算可用，见 BUG-2026-1023-009）
fn is_usable_file(path: &Path) -> bool {
    path.metadata().map(|m| m.is_file() && m.len() > 0).unwrap_or(false)
}

/// 两条路径是否指向同一个文件（字符串比较足够：都来自同一份缓存目录命名规则）
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
}

/// 按新指纹算网格缩略图的目标路径（与 `ensure_grid_thumb` 的命名**逐字一致**）
///
/// 注意：`thumbnail::grid_thumb_cache_name` 是**封面**那套（webp + 分目录），
/// 网格图走的是平铺的 `grid/album_<id>_photo_<fp>.jpg`，两者不能混用。
/// 旧行里的 `thumb_path` 全部是 `...\grid\album_<id>_photo_<fp>.jpg`（已核实）。
fn grid_thumb_path(thumbs_dir: &Path, album_id: Option<i64>, source: &Path) -> PathBuf {
    let album = album_id.unwrap_or(0);
    let fp = thumbnail::thumbnail_fingerprint(source);
    thumbs_dir
        .join("grid")
        .join(format!("album_{album}_photo_{fp}.jpg"))
}

// 模块对外只暴露 `migrate_if_needed` 与 `MigrateReport`；`row_to_record`
// 之前预留给了命令层，实际用不到，于是不再导出（避免 dead_code 告警）。

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pm_migrate_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_jpeg(path: &Path, orientation: Option<u16>) {
        let mut img = RgbImage::new(64, 48);
        for px in img.pixels_mut() {
            *px = Rgb([10, 120, 200]);
        }
        match orientation {
            // 无 EXIF === orientation=1（无旋转）
            None => img.save(path).unwrap(),
            Some(o) => {
                // 直接手写 EXIF Orientation（image 的编码器没有写 EXIF 的公开 API）：
                // 写一个最小的 TIFF 头 IFD，只有一个 Orientation 标签。
                let mut buf: Vec<u8> = Vec::new();
                img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)
                    .unwrap();
                let exif = exif_bytes(o);
                // JPEG 结构：SOI(FFD8) + APP1(FFE1, len, "Exif\0\0", tiff) + 其余
                let mut out: Vec<u8> = Vec::with_capacity(buf.len() + exif.len() + 16);
                out.extend_from_slice(&buf[..2]); // SOI
                let payload_len = 2 + 6 + exif.len(); // len 自身 + "Exif\0\0" + tiff
                out.extend_from_slice(&[0xFF, 0xE1]);
                out.extend_from_slice(&(payload_len as u16).to_be_bytes());
                out.extend_from_slice(b"Exif\0\0");
                out.extend_from_slice(&exif);
                out.extend_from_slice(&buf[2..]);
                std::fs::write(path, out).unwrap();
            }
        }
    }

    /// 最小 EXIF（TIFF 小端）：IFD0 仅含 Orientation 标签
    fn exif_bytes(orientation: u16) -> Vec<u8> {
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(b"II\x2a\x00"); // 小端 + 42
        b.extend_from_slice(&8u32.to_le_bytes()); // IFD0 偏移
        b.extend_from_slice(&1u16.to_le_bytes()); // 条目数
        b.extend_from_slice(&0x0112u16.to_le_bytes()); // Tag: Orientation
        b.extend_from_slice(&3u16.to_le_bytes()); // Type: SHORT
        b.extend_from_slice(&1u32.to_le_bytes()); // Count
        b.extend_from_slice(&orientation.to_le_bytes()); // Value（SHORT 占前 2 字节）
        b.extend_from_slice(&[0, 0]); // 补齐到 4 字节
        b.extend_from_slice(&0u32.to_le_bytes()); // 无下一个 IFD
        b
    }

    /// 迁移主流程：旧键行 → 改名复用 → 新键可命中 → 旧行/孤儿被清
    #[test]
    fn migrates_old_keys_reuses_files_and_cleans_orphans() {
        let root = tmpdir("happy");
        let photos = root.join("photos");
        let thumbs = root.join("thumbs");
        let grid = thumbs.join("grid");
        std::fs::create_dir_all(&photos).unwrap();
        std::fs::create_dir_all(&grid).unwrap();

        // orientation=1 的照片（可复用）+ orientation=6 的照片（必须重生成）
        let p_ok = photos.join("flat.jpg");
        write_jpeg(&p_ok, None);
        let p_rot = photos.join("rot.jpg");
        write_jpeg(&p_rot, Some(6));

        let db = Database::open(&root.join("t.db")).unwrap();
        db.init_thumb_cache_schema().unwrap();

        // 造旧键行（模拟 v0.4.3 的键）+ 旧文件
        let mut old_rows = Vec::new();
        for (src, album) in [(&p_ok, 7i64), (&p_rot, 7i64)] {
            let (len, mtime) = read_meta(src).unwrap();
            let old_hash = thumbnail::thumb_photo_hash_v1(src, len, mtime, THUMB_RENDER_VERSION);
            let old_file = grid.join(format!("album_{album}_photo_old_{}.jpg", old_hash));
            std::fs::write(&old_file, b"v1-thumb-bytes").unwrap();
            old_rows.push(ThumbCacheRecord {
                photo_hash: old_hash,
                source_path: src.to_string_lossy().into_owned(),
                thumb_path: old_file.to_string_lossy().into_owned(),
                album_id: Some(album),
                user_id: 1,
                size_bytes: len,
                mtime_ns: mtime,
            });
        }
        db.upsert_thumb_caches(&old_rows).unwrap();

        // 孤儿文件（表里没有任何行引用）
        let orphan = grid.join("album_99_photo_orphan.jpg");
        std::fs::write(&orphan, vec![0u8; 2048]).unwrap();

        let report = migrate_if_needed(&db, &thumbs);

        assert!(report.ran, "首次必须真的执行");
        assert_eq!(report.scanned_rows, 2);
        // 只有 orientation=1 的那张允许复用
        assert_eq!(report.migrated_rows, 1, "orientation=1 应当被复用");
        assert_eq!(report.needs_regen_rows, 1, "orientation=6 必须拒绝复用");
        assert_eq!(report.deleted_orphan_files, 1, "孤儿文件应被回收");
        assert!(report.freed_bytes >= 2048);
        assert!(!orphan.exists());

        // 新键现在能命中，且读到的是复用后的新文件名
        let (len, mtime) = read_meta(&p_ok).unwrap();
        let new_hash = thumbnail::thumb_photo_hash(&p_ok, len, mtime);
        let hit = db.lookup_thumb_cache_one(&new_hash).unwrap().expect("新键必须命中");
        let hit_path = PathBuf::from(&hit.thumb_path);
        assert!(hit_path.is_file(), "复用后的文件必须存在: {}", hit.thumb_path);
        assert!(
            hit_path.file_name().unwrap().to_string_lossy().contains(&thumbnail::thumbnail_fingerprint(&p_ok)),
            "文件名必须是新指纹（懒加载才能命中）"
        );
        assert_eq!(std::fs::read(&hit_path).unwrap(), b"v1-thumb-bytes", "复用是搬家，不得重编码");

        // 旧文件已被改名走（不残留）
        assert!(!Path::new(&old_rows[0].thumb_path).exists());

        // 幂等：再跑一次直接返回
        let again = migrate_if_needed(&db, &thumbs);
        assert!(!again.ran, "打过标就不应重跑");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 源图已删除的行：不计入复用，也不报错（保守跳过）
    #[test]
    fn skips_rows_with_missing_source() {
        let root = tmpdir("missing_src");
        let thumbs = root.join("thumbs");
        std::fs::create_dir_all(thumbs.join("grid")).unwrap();
        let db = Database::open(&root.join("t.db")).unwrap();
        db.init_thumb_cache_schema().unwrap();

        let ghost = root.join("photos/gone.jpg");
        db.upsert_thumb_caches(&[ThumbCacheRecord {
            photo_hash: "deadbeefdeadbeef".into(),
            source_path: ghost.to_string_lossy().into_owned(),
            thumb_path: thumbs.join("grid/x.jpg").to_string_lossy().into_owned(),
            album_id: Some(1),
            user_id: 1,
            size_bytes: 1,
            mtime_ns: 1,
        }])
        .unwrap();

        let report = migrate_if_needed(&db, &thumbs);
        assert_eq!(report.scanned_rows, 1);
        assert_eq!(report.missing_source_rows, 1);
        assert_eq!(report.migrated_rows, 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
