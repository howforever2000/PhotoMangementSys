//! 缩略图生成模块
//!
//! 职责：扫描相册文件夹，找到第一张图片，生成缩略图并缓存。
//! 缓存目录：`app_data_dir/thumbs/<子目录>/<指纹>.webp`
//!
//! P1 缩略图优化：
//! - WebP 编码（质量 85，压缩率比 JPEG 高 30~50%）
//! - 按指纹前 2 位分目录（避免单目录文件过多，IO 性能更好）

use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use image::ImageFormat;
use serde::{Deserialize, Serialize};

/// 支持的图片扩展名（按优先级排列，作为"第一张图"的扫描顺序）
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

/// 缩略图边长（需求文档提到 256px 缩略图）
const THUMB_SIZE: u32 = 256;

/// 缩略图模块错误
#[derive(Debug)]
pub enum ThumbError {
    Io(std::io::Error),
    Image(image::ImageError),
    /// JPEG 降采样解码失败（详见 jpeg_decoder::Error）
    Jpeg(jpeg_decoder::Error),
    /// 解码结果尺寸信息缺失/像素缓冲无效
    Decode,
}

impl std::fmt::Display for ThumbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThumbError::Io(e) => write!(f, "IO 错误: {e}"),
            ThumbError::Image(e) => write!(f, "图片处理错误: {e}"),
            ThumbError::Jpeg(e) => write!(f, "JPEG 解码错误: {e}"),
            ThumbError::Decode => write!(f, "图片解码结果无效"),
        }
    }
}

impl From<std::io::Error> for ThumbError {
    fn from(e: std::io::Error) -> Self {
        ThumbError::Io(e)
    }
}
impl From<image::ImageError> for ThumbError {
    fn from(e: image::ImageError) -> Self {
        ThumbError::Image(e)
    }
}
impl From<jpeg_decoder::Error> for ThumbError {
    fn from(e: jpeg_decoder::Error) -> Self {
        ThumbError::Jpeg(e)
    }
}

/// 判断文件名是否是支持的图片
fn is_image_file(file_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    IMAGE_EXTS.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// 相册目录单次扫描结果（一次 walkdir 同时完成数量/大小/首图三项统计）
///
/// 替代原先 count_images + folder_size + find_first_image 的三次独立遍历，
/// 列表加载时每相册从 3~4 次全目录遍历降为 1 次。
pub struct AlbumScan {
    /// 支持的图片文件数
    pub photo_count: usize,
    /// 目录真实占用空间（字节，累加所有文件）
    pub size_bytes: u64,
    /// 扫描到的第一张图片路径（无图片则为 None）
    pub first_image: Option<PathBuf>,
}

/// 判断 walkdir 条目是否应被当作「文件」计入（BUG-2026-1010-002）
///
/// 背景：遍历一律用 `follow_links(false)`（防止软链成环导致无限递归），
/// 但该模式下符号链接的 `file_type()` 是 `symlink` 而非 `file`，
/// 于是 `file_type().is_file()` 会把**所有**软链图片判掉 —— 表现为
/// 「相册明明有照片却显示无照片」。桌面环境（尤其 Deepin 的
/// /usr/share/wallpapers 软链）是重灾区。
///
/// 判定：普通文件直接算；符号链接则用 `metadata()`（跟随链接）确认
/// 其最终指向一个真实文件才算，从而在「不跟随链接递归」与「软链图片可见」
/// 之间取得平衡。断链（指向已删除目标）仍会被正确排除。
pub fn entry_is_file(entry: &walkdir::DirEntry) -> bool {
    let ft = entry.file_type();
    if ft.is_file() {
        return true;
    }
    if ft.is_symlink() {
        // metadata() 跟随符号链接；目标不存在时返回 Err，断链自然被排除。
        return entry.path().metadata().map(|m| m.is_file()).unwrap_or(false);
    }
    false
}

/// 单次遍历统计相册目录：图片数量 + 总大小 + 第一张图片
///
/// 递归遍历目录（含子目录），跳过隐藏目录/文件。
/// 轻量变更探测：递归统计目录内文件总数（只数不读，不判格式）
///
/// 用于统计缓存的脏检测信号：与 album_stats.file_count 比对，不一致才触发全量重扫。
/// 成本远低于 `scan_album_dir`（后者还要累加大小/读 EXIF/生成缩略图）。
pub fn count_files_recursive(dir: &Path) -> u64 {
    if !dir.is_dir() {
        return 0;
    }
    let mut count = 0u64;
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        let Ok(e) = entry else { continue };
        if entry_is_file(&e) {
            count += 1;
        }
    }
    count
}

pub fn scan_album_dir(dir: &Path) -> AlbumScan {
    let mut scan = AlbumScan {
        photo_count: 0,
        size_bytes: 0,
        first_image: None,
    };
    if !dir.is_dir() {
        return scan;
    }
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        let Ok(e) = entry else { continue };
        if !entry_is_file(&e) {
            continue;
        }
        // 大小：累加所有文件（不限于图片，保持与旧 folder_size 语义一致）
        if let Ok(meta) = e.metadata() {
            scan.size_bytes += meta.len();
        }
        // 图片计数 + 首图：仅图片文件
        let name = e.file_name().to_string_lossy().to_string();
        if is_image_file(&name) {
            scan.photo_count += 1;
            if scan.first_image.is_none() {
                scan.first_image = Some(e.into_path());
            }
        }
    }
    scan
}

/// 网格缩略图缓存子目录（与封面缩略图隔离，避免 cleanup_album_auto_thumbs 误删）
const GRID_THUMBS_SUBDIR: &str = "grid";

/// 缩略图渲染算法版本（**只参与磁盘文件名**，不参与表主键）。
///
/// 语义：**只影响像素内容**的改动才需要 bump。v2 = 按 EXIF Orientation 转正
/// （BUG-2026-1014-002）；v1 = 初始实现。
///
/// ⚠️ BUG-2026-1023-009 的教训：本版本号曾经被同时掺进 `thumb_photo_hash`
/// （`photo_thumb_cache` 的主键），一次 bump 就把 2.5 万行缓存记录和扫描侧的
/// `photo_content_scan.photo_hash` 撕成两套键空间——扫描侧 `count_scanned_without_thumb`
/// 仍然能对得上旧行、认为「全都有缩略图所以不必预热」，而懒加载侧查新键又一条不中，
/// 于是升级后第一次浏览变成全库重生成。
///
/// 现约定：**主键只由 path+size+mtime 决定（与 `content::photo_hash` 完全同源）；
/// 渲染版本只进文件名**。这样 bump 只让旧**文件**失效，表行可原地重订键复用。
pub(crate) const THUMB_RENDER_VERSION: u32 = 2;

/// FEAT-044：计算与 `photo_content_scan.photo_hash` 同源的指纹
///
/// 与 `content::photo_hash` 逻辑一致（路径+大小+mtime 纳秒的 FNV-1a 64），
/// 之所以在 thumbnail.rs 中复制实现，是为了让“仅调用缩略图模块”的场景不依赖
/// content 模块（如别处动态生成缩略图但不入库）。
/// 跨模块一致性靠**集成测试**保证（见 `tests_thumb_cache_consistency`）。
///
/// ❗ 不要再往 input 里掺渲染版本 —— 那会把两套键空间拆开（BUG-2026-1023-009）。
pub fn thumb_photo_hash(path: &Path, len: u64, mtime_ns: u128) -> String {
    let path_str = path.to_string_lossy();
    let input = format!("{len}|{mtime_ns}|{path_str}");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in input.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 历史 v1 键（**掺了渲染版本**），仅供一次性迁移把老行认回来。
///
/// BUG-2026-1023-009：v1→v2 升级时主键掺了 `|v2`，老库里那 2.4 万行用的就是
/// 这个函数算出的键。迁移时用同样的算法才算得到它们，否则永远找不回旧行。
/// **新代码不要调用它**（新键见 [`thumb_photo_hash`]）。
pub fn thumb_photo_hash_v1(path: &Path, len: u64, mtime_ns: u128, version: u32) -> String {
    let path_str = path.to_string_lossy();
    let input = format!("{len}|{mtime_ns}|{path_str}|v{version}");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in input.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 从 `Path` 读取 `(len, mtime_ns)`，失败返回 `None`
fn read_file_meta(path: &Path) -> Option<(u64, u128)> {
    let md = std::fs::metadata(path).ok()?;
    let len = md.len();
    let mtime_ns = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some((len, mtime_ns))
}

/// FEAT-044：表命中后返表的 thumb_path，但需额外 stat 确认文件存在
/// （表里记录可能由于人为删除缓存 / 磁盘清理失效）
fn is_thumb_file_alive(thumb_path: &str) -> bool {
    // 长度必须 > 0：生成中途进程被打断会留下 0 字节文件（BUG-2026-1023-009
    // 崩溃现场就有 20 个），只判 `is_file()` 会把它当成缓存命中，于是
    // 网格里那一格永远是一张碎图且再也不会自愈。
    !thumb_path.is_empty()
        && std::path::Path::new(thumb_path)
            .metadata()
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false)
}

/// FEAT-044：表命中后构建记录并 upsert
///
/// 仅在表查不到或表命中但文件已丢失时调用；生成缩略图后调用此函数写表，
/// 以便下次 0 IO 命中。
fn record_thumb_cache(
    db: &super::db::Database,
    album_id: i64,
    user_id: i64,
    source: &Path,
    thumb_path: &str,
) -> Result<(), super::db::DbError> {
    let Some((len, mtime_ns)) = read_file_meta(source) else {
        return Ok(());
    };
    let hash = thumb_photo_hash(source, len, mtime_ns);
    let rec = super::db::ThumbCacheRecord {
        photo_hash: hash,
        source_path: source.to_string_lossy().into_owned(),
        thumb_path: thumb_path.to_string(),
        album_id: Some(album_id),
        user_id,
        size_bytes: len,
        mtime_ns,
    };
    db.upsert_thumb_caches(&[rec])
}

/// 确保单个照片的网格缩略图存在，返回缓存绝对路径
///
/// - 缓存名：`thumbs/grid/album_<id>_photo_<fingerprint>.jpg`
/// - 指纹命名：文件内容变化 → 指纹变化 → 自动换名（与封面缩略图一致）
/// - 与封面缩略图互不干扰：封面清理只扫 `thumbs/` 根目录，网格缩略图在 `grid/` 子目录
/// - 网格只加载 256px 缩略图，避免网格展示原图造成内存/IO 压力
/// - **FEAT-044 走表**：`db` 不为 None 时先查 `photo_thumb_cache` 表：
///     命中且 `thumb_path` 仍存在 → 直接返回（0 IO）。
///     未命中 / 表中 thumb_path 已丢失 → 生成后写表供后续命中。
///   `db` 为 None 时退化为纯文件系统缓存（兼容遗留调用方）。
pub fn ensure_grid_thumb(
    album_id: i64,
    source: &Path,
    thumbs_dir: &Path,
    db: Option<&super::db::Database>,
    user_id: i64,
) -> Result<String, ThumbError> {
    // 1. 走表：photo_hash 命中且文件存在 → 0 IO 返回
    if let Some(db) = db {
        if let Some((len, mtime_ns)) = read_file_meta(source) {
            let hash = thumb_photo_hash(source, len, mtime_ns);
            if let Ok(Some(hit)) = db.lookup_thumb_cache_one(&hash) {
                if is_thumb_file_alive(&hit.thumb_path) {
                    return Ok(hit.thumb_path);
                }
            }
        }
    }
    // 2. 表未命中 / 走 db 走空 → 文件系统指纹检查
    let grid_dir = thumbs_dir.join(GRID_THUMBS_SUBDIR);
    let fingerprint = file_fingerprint(source);
    let cached_name = format!("album_{album_id}_photo_{fingerprint}.jpg");
    let thumb_path = grid_dir.join(&cached_name);

    if thumb_path.exists() {
        // 文件存在但表未记录：仅写表（让下次 0 IO 命中）
        if let Some(db) = db {
            let _ = record_thumb_cache(
                db,
                album_id,
                user_id,
                source,
                &thumb_path.to_string_lossy(),
            );
        }
        return Ok(thumb_path.to_string_lossy().into_owned());
    }

    // 3. 缓存未命中：生成（JPEG 走 DCT 降采样快速路径）
    save_thumbnail(source, &thumb_path)?;

    // 4. 写表（让后续调用 0 IO 命中）
    if let Some(db) = db {
        let _ = record_thumb_cache(
            db,
            album_id,
            user_id,
            source,
            &thumb_path.to_string_lossy(),
        );
    }
    Ok(thumb_path.to_string_lossy().into_owned())
}

/// FEAT-044：带表查询的批量确保（拆为查 + 生成两个独立步骤以避开线程问题）
///
/// `db_lookup` 由调用方提供：“传一批 `photo_hash` → 返回 `HashMap<hash, thumb_path>` 已命中且
/// 存活项的映射”，调用方负责在独立的锁粒度内查表（避免 `MutexGuard<Database>` 跨 `spawn_blocking`）。
/// - 本函数是同步、纯文件系统 / 生成的；
/// - 生成后的记录交由 `on_generated` 回调统一写表（同样避免 Mutex 生命周期问题）。
pub fn ensure_grid_thumbs_with_lookup<FOnGen>(
    album_id: i64,
    sources: &[String],
    thumbs_dir: &Path,
    db_lookup: &dyn Fn(&[String]) -> std::collections::HashMap<String, String>,
    on_generated: FOnGen,
) -> Vec<(String, String)>
where
    FOnGen: Fn(&[(String, String, u64, u128)]) + Send + Sync,
{
    // 1. 算 photo_hash + 反查表
    let mut source_hashes: Vec<(String, String)> = Vec::with_capacity(sources.len());
    for s in sources {
        let path = std::path::Path::new(s);
        if let Some((len, mtime)) = read_file_meta(path) {
            source_hashes.push((s.clone(), thumb_photo_hash(path, len, mtime)));
        }
    }
    let hashes: Vec<String> = source_hashes.iter().map(|(_, h)| h.clone()).collect();
    let hit_map = db_lookup(&hashes); // hash → thumb_path（已验证 alive）

    // 2. 生成未命中项 —— **本函数一律串行**（BUG-2026-1023-009）。
    //
    // 这里原本是「misses 够多就 thread::scope 按核数开 min(核数,8) 个线程」。
    // 单请求看是加速，多请求叠加时是灾难：时间线页一次点开就并发了 129 个
    // 本函数调用（每个相册一路），129 × 8 ≈ 1000 个线程同时解码大 JPEG，
    // 线程数实测冲到 732、内存 1.2GB，最后整进程以 0xC00000FD 退出。
    //
    // 并行度改由**进程级**闸门（`save_thumbnail` 里的 `acquire_gen_permit`）
    // 提供：多路请求天然并行，但真正同时解码的线程数恒定 ≤ gen_max()。
    // 代价是单个请求变慢，收益是「请求再多也不会把进程打死」——这里选后者。
    let misses: Vec<(String, String)> = source_hashes
        .iter()
        .filter(|(_, hash)| !hit_map.contains_key(hash))
        .cloned()
        .collect();
    let mut gen_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut generated: Vec<(String, String, u64, u128)> = Vec::new(); // (source, thumb, len, mtime_ns)
    for (source, _hash) in &misses {
        let path = std::path::Path::new(source.as_str());
        let (len, mtime_ns) = match read_file_meta(path) {
            Some(m) => m,
            None => continue,
        };
        match ensure_grid_thumb(album_id, path, thumbs_dir, None, 0) {
            Ok(thumb) => {
                gen_map.insert(source.clone(), thumb.clone());
                generated.push((source.clone(), thumb, len, mtime_ns));
            }
            Err(_) => {
                // 单张失败不影响其余：跳过
            }
        }
    }
    // 3. 按请求顺序组装结果（表命中项在前，新生成项按原顺序插入）
    let mut out: Vec<(String, String)> = Vec::with_capacity(sources.len());
    for (source, hash) in &source_hashes {
        if let Some(thumb) = hit_map.get(hash) {
            out.push((source.clone(), thumb.clone()));
        } else if let Some(thumb) = gen_map.get(source) {
            out.push((source.clone(), thumb.clone()));
        }
    }
    // 4. 回调写表（让调用方决定怎么写、怎么加锁）
    on_generated(&generated);
    out
}

/// 对「已知缺失」的网格缩略图并行生成（语义腿 `scan_album_embeddings` 专用），带进度回调。
///
/// 与 `ensure_grid_thumbs_with_lookup` 的分工：
/// - 带 `_with_lookup` 的那版负责「算 hash → 查表 → 生成」三步，适合调用方尚未算过指纹的场景；
/// - 本函数假设调用方**已算好 `photo_hash` 并查过表**，只对确定缺失的那批做并行生成，
///   不重复 stat、不重复查表。
///
/// 为什么需要它（P19）：语义腿原本写成 `missing.iter().filter_map(...)` 单线程串行，
/// 实测 412 张耗时 **169.8s（412ms/张）**，占该相册总耗时 203s 的 **83%**，且全程无日志 ——
/// 它是三腿并发里的关键路径：AI 腿 41.6s 就跑完，随后空等 154s。
/// 并行后预估降到 20~30s（8 线程）。
///
/// 返回 `(source, photo_hash, thumb_path)`，**顺序与输入一致**（按下标落位，
/// 不依赖线程完成顺序）；生成失败项直接丢弃，与旧串行版 `filter_map` 行为一致。
///
/// - 线程数 = `available_parallelism().min(8)`；样本 < 2×线程数时退化为串行（省线程开销）
/// - `on_progress(done, total)` 每完成一张回调一次，由调用方决定上报频率
///   （日志写入在 `logger` 内部有全局 Mutex 串行，多线程回调安全）
pub fn ensure_grid_thumbs_missing_parallel(
    album_id: i64,
    missing: &[(String, String)],
    thumbs_dir: &Path,
    user_id: i64,
    on_progress: Option<&(dyn Fn(usize, usize) + Sync)>,
) -> Vec<(String, String, String)> {
    let total = missing.len();
    if total == 0 {
        return Vec::new();
    }

    // 结果按下标落位：线程完成顺序不影响输出顺序
    let out: std::sync::Mutex<Vec<Option<(String, String, String)>>> =
        std::sync::Mutex::new((0..total).map(|_| None).collect());
    let done = std::sync::atomic::AtomicUsize::new(0);

    let workers = std::thread::available_parallelism()
        .map(|w| w.get())
        .unwrap_or(4)
        .min(8)
        .max(1);

    if total >= workers * 2 && workers > 1 {
        let chunk = total.div_ceil(workers);
        std::thread::scope(|scope| {
            for (wi, part) in missing.chunks(chunk).enumerate() {
                let out = &out;
                let done = &done;
                let on_progress = on_progress;
                scope.spawn(move || {
                    generate_missing_range(
                        album_id,
                        thumbs_dir,
                        user_id,
                        part,
                        wi * chunk,
                        out,
                        done,
                        total,
                        on_progress,
                    );
                });
            }
        });
    } else {
        generate_missing_range(
            album_id,
            thumbs_dir,
            user_id,
            missing,
            0,
            &out,
            &done,
            total,
            on_progress,
        );
    }

    out.into_inner()
        .unwrap()
        .into_iter()
        .flatten()
        .collect()
}

/// 生成一段缺失项并按下标写入 `out`（并行块与串行退化共用同一段逻辑）。
///
/// 私有：只服务于 `ensure_grid_thumbs_missing_parallel`，避免闭包跨线程捕获带来的 `Sync` 约束。
fn generate_missing_range(
    album_id: i64,
    thumbs_dir: &Path,
    user_id: i64,
    items: &[(String, String)],
    start_idx: usize,
    out: &std::sync::Mutex<Vec<Option<(String, String, String)>>>,
    done: &std::sync::atomic::AtomicUsize,
    total: usize,
    on_progress: Option<&(dyn Fn(usize, usize) + Sync)>,
) {
    for (i, (src, hash)) in items.iter().enumerate() {
        // 与旧串行版语义一致：单张失败不影响其余，丢弃该项继续
        if let Ok(tp) = ensure_grid_thumb(
            album_id,
            std::path::Path::new(src),
            thumbs_dir,
            None,
            user_id,
        ) {
            if let Ok(mut g) = out.lock() {
                g[start_idx + i] = Some((src.clone(), hash.clone(), tp));
            }
        }
        let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if let Some(cb) = on_progress {
            cb(d, total);
        }
    }
}

/// 删除相册的全部网格缩略图（删除相册记录时调用，避免缓存磁盘持续增长）
pub fn cleanup_album_grid_thumbs(album_id: i64, thumbs_dir: &Path) {
    let grid_dir = thumbs_dir.join(GRID_THUMBS_SUBDIR);
    let prefix = format!("album_{album_id}_photo_");
    if let Ok(entries) = std::fs::read_dir(&grid_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// 计算单张原图的网格缩略图缓存文件名（P1 WebP + 分目录）
pub fn grid_thumb_cache_name(album_id: i64, source: &Path) -> String {
    let fp = file_fingerprint(source);
    let subdir = webp_subdir(&fp);
    format!("album_{album_id}_photo_{subdir}/{fp}.webp")
}

/// 删除指定缓存文件名列表对应的网格缩略图（照片删除后级联清理）
pub fn remove_grid_thumb_files(names: &[String], thumbs_dir: &Path) {
    let grid_dir = thumbs_dir.join(GRID_THUMBS_SUBDIR);
    for n in names {
        let _ = std::fs::remove_file(grid_dir.join(n));
    }
}

/// 递归收集相册文件夹内所有图片的绝对路径（无需扫描/入库即可展示照片）
///
/// 用于照片网格浏览：轻量 walkdir 遍历（只取路径不读图），过滤隐藏目录与图片扩展名。
/// 返回按路径字典序稳定排序，便于前端分页/对比。
pub fn list_album_images(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        let Ok(e) = entry else { continue };
        if !entry_is_file(&e) {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if is_image_file(&name) {
            out.push(e.into_path().to_string_lossy().into_owned());
        }
    }
    out.sort();
    out
}

/// Unix 秒 → (年, 月, 日)（Howard Hinnant civil_from_days 算法，无需日期库依赖）
fn unix_secs_to_date(secs: i64) -> (i32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// 读取文件修改时间（mtime）作为拍摄时间参考，返回 "YYYY-MM-DD"
///
/// 很多图片（微信导出/截图/后期处理）没有 EXIF 拍摄时间，但文件修改时间
/// 通常接近实际拍摄时刻，作为兜底能补齐日期定位。
fn read_file_mtime(path: &Path) -> Option<String> {
    let meta = path.metadata().ok()?;
    let modified = meta.modified().ok()?;
    let dur = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    let (y, m, d) = unix_secs_to_date(dur.as_secs() as i64);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// 读取图片的拍摄时间，返回 "YYYY-MM-DD"
///
/// 优先级（三级 EXIF 兜底 + mtime）：
/// 1. EXIF DateTimeOriginal（相机记录的准确拍摄时间）
/// 2. EXIF DateTime（扫描图/后期处理图兜底）
/// 3. GPS UTC 时间 +8h（相机时间漂移兜底）
/// 4. 文件修改时间 mtime 兜底（无 EXIF 时，修改时间接近拍摄时刻）
pub fn read_shoot_time(path: &Path) -> Option<String> {
    // 优先 EXIF DateTimeOriginal
    if let Some(exif_time) = read_exif_shoot_time(path) {
        return Some(exif_time);
    }
    // EXIF 缺失/读取失败 → 回退文件修改时间（修复：无 EXIF 图片日期定位缺失）
    read_file_mtime(path)
}

/// 读取 EXIF 拍摄时间，返回 "YYYY-MM-DD"；缺失/失败返回 None
///
/// 三级 EXIF 兜底
///   1. DateTimeOriginal（0x9003，相机快门时刻）
///   2. DateTime（0x0132，扫描图/后期处理图常有 Original 缺失但保留 DateTime）
///   3. GPS 时间（GPSDateStamp+GPSTimeStamp，UTC +8 换算本地日期；相机时间漂移时的兜底）
fn read_exif_shoot_time(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bufreader = std::io::BufReader::new(&file);
    let exif_reader = exif::Reader::new();
    let exif = exif_reader.read_from_container(&mut bufreader).ok()?;

    // 1) DateTimeOriginal（"YYYY:MM:DD HH:MM:SS"）
    if let Some(field) = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY) {
        if let Some(date) = exif_date_to_iso(&field.display_value().to_string()) {
            return Some(date);
        }
    }
    // 2) DateTime 兜底（相机 vs 数字化时间；DateTimeOriginal 缺失时用）
    if let Some(field) = exif.get_field(exif::Tag::DateTime, exif::In::PRIMARY) {
        if let Some(date) = exif_date_to_iso(&field.display_value().to_string()) {
            return Some(date);
        }
    }
    // 3) GPS 时间兜底（UTC）→ +8 小时换算本地日期（东八区假设，面向国内相册）
    if let Some(date) = read_gps_utc_date(&exif) {
        return Some(date);
    }
    None
}

/// "2023:01:15 10:30:00" → "2023-01-15"（取前 10 字符，冒号换横线；容错引号）
fn exif_date_to_iso(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_matches('"');
    let digits: Vec<char> = raw.chars().take(10).collect();
    if digits.len() < 10 {
        return None;
    }
    let d = [
        digits[0], digits[1], digits[2], digits[3],
        digits[5], digits[6], digits[8], digits[9],
    ];
    // 仅接受数字日期（防非法 ASCII 值）
    if !d.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!(
        "{}{}{}{}-{}{}-{}{}",
        d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    ))
}

/// GPS 时间（UTC）→ +8 小时 → 本地日期 "YYYY-MM-DD"
///
/// GPSDateStamp 为 ASCII "YYYY:MM:DD"，GPSTimeStamp 为 Rational 时分秒。
/// GPS 时间属 UTC，东八区直接 +8h（可能跨天进位，用 unix 秒换算保证正确）。
fn read_gps_utc_date(exif: &exif::Exif) -> Option<String> {
    let ds = exif.get_field(exif::Tag::GPSDateStamp, exif::In::PRIMARY)?;
    let date = match &ds.value {
        exif::Value::Ascii(v) => v
            .iter()
            .map(|b| String::from_utf8_lossy(b).trim_matches('\0').to_string())
            .collect::<Vec<_>>()
            .join(""),
        _ => return None,
    };
    // "YYYY:MM:DD" → 三个数字
    let parts: Vec<&str> = date.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i64 = parts[0].parse().ok()?;
    let mo: i64 = parts[1].parse().ok()?;
    let d: i64 = parts[2].parse().ok()?;

    let ts = exif.get_field(exif::Tag::GPSTimeStamp, exif::In::PRIMARY)?;
    let mut t = [0i64; 3];
    if let exif::Value::Rational(v) = &ts.value {
        if v.len() < 3 {
            return None;
        }
        for i in 0..3 {
            let den = v[i].denom;
            t[i] = if den != 0 { (v[i].num / den) as i64 } else { 0 };
        }
    } else {
        return None;
    }
    // 年月日时分秒 → unix 秒（UTC）→ +8h
    let secs = date_to_unix_secs(y, mo, d, t[0], t[1], t[2])? + 8 * 3600;
    let (yy, mm, dd) = unix_secs_to_date(secs);
    Some(format!("{yy:04}-{mm:02}-{dd:02}"))
}

/// (年,月,日,时,分,秒) → unix 秒（Howard Hinnant days_from_civil 算法）
fn date_to_unix_secs(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(0..=23).contains(&h)
        || !(0..=59).contains(&mi) || !(0..=59).contains(&s) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;                                  // [0, 399]
    let mp = (m + 9) % 12;                                    // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1;                     // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;          // [0, 146096]
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + s)
}

/// 缩略图结果信息
#[derive(Debug, Serialize, Deserialize)]
pub struct ThumbResult {
    /// 缩略图绝对路径（位于缓存目录）
    pub thumb_path: String,
    /// 原图路径（第一张图片）
    pub source_path: String,
}

/// 计算文件内容指纹（用于缩略图缓存命名）
///
/// 组合：文件长度 + 最后修改时间 + 文件头 8KB 的哈希。
/// 用户替换/修改照片后指纹必然变化，缓存自动失效并换名（旧文件随后被清理），
/// 修复旧版"同名文件缓存不失效"的问题。
/// FNV-1a 64 位哈希（确定性：跨编译/跨平台结果恒定，适合持久化缓存命名）
///
/// 注意：不能使用 `std::hash::DefaultHasher`——其种子在每次编译时随机生成，
/// 会导致重新编译应用后同一文件的指纹全变、缩略图缓存全部失效（曾导致
/// 每次启动都重新生成全部缩略图，列表加载卡死约 140 秒）。
fn fnv1a64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// 计算文件内容指纹（用于缩略图缓存命名）
///
/// 组合：文件长度 + 最后修改时间 + 文件头 8KB 的确定性哈希。
/// 用户替换/修改照片后指纹必然变化，缓存自动失效并换名（旧文件随后被清理）。
fn file_fingerprint(path: &Path) -> String {
    let mut fp = String::new();
    // 同 THUMB_RENDER_VERSION 的理由：指纹决定**磁盘文件名**，转正后像素变了，
    // 旧名对应的躺倒 WebP 若继续命中，同样是「改了没变化」。
    fp.push_str(&format!("v{THUMB_RENDER_VERSION}_"));
    if let Ok(meta) = path.metadata() {
        fp.push_str(&meta.len().to_string());
        if let Ok(m) = meta.modified() {
            if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
                fp.push('_');
                fp.push_str(&d.as_secs().to_string());
            }
        }
    }
    // 文件头 8KB 哈希，覆盖"拷贝保留 mtime"的场景
    let mut head = [0u8; 8192];
    let mut n = 0usize;
    if let Ok(mut f) = std::fs::File::open(path) {
        use std::io::Read;
        n = f.read(&mut head).unwrap_or(0);
    }
    fp.push('_');
    fp.push_str(&fnv1a64(&head[..n]).to_string());
    fp
}

/// 供迁移复用：暴露文件名指纹（`file_fingerprint` 是私有的）
///
/// 命名规则必须与 `ensure_grid_thumb` 内部一致，否则迁移改出来的名字
/// 跟懒加载算出来的名字对不上，白忙一场。
pub fn thumbnail_fingerprint(source: &Path) -> String {
    file_fingerprint(source)
}

/// P1 WebP 优化：按指纹前 2 位分目录（避免单目录文件过多）
fn webp_subdir(fingerprint: &str) -> String {
    fingerprint.chars().take(2).collect()
}

/// FEAT-050：某照片在网格缩略图目录下的全部可能缓存文件名
///
/// flat JPG（ensure_grid_thumb 旧命名）与 WebP 嵌套命名两套都返回，
/// 供删除时级联清理。指纹依赖原图可读，调用需在原图被删前进行。
pub fn grid_thumb_cache_names_all(album_id: i64, source: &Path) -> Vec<String> {
    let fp = file_fingerprint(source);
    let subdir = webp_subdir(&fp);
    vec![
        format!("album_{album_id}_photo_{fp}.jpg"),
        format!("album_{album_id}_photo_{subdir}/{fp}.webp"),
    ]
}

/// P1 WebP 兼容：尝试复用旧版 .jpg 缓存（命中即整字节复制，**不做转码** ——
/// 复用的目的就是为了避免重新解码大图）。
///
/// 覆盖三代旧命名：
/// 1. `grid/album_<id>_photo_<fp>.jpg` —— 上一代（网格缩略图 + 指纹命名）
/// 2. `album_<id>_auto_<fp>.jpg` —— 上一代（封面 + 指纹命名）
/// 3. `album_<id>_<safe_stem>.jpg` —— 最早一代（按源图**文件名**）：
///    `ensure_thumbnail_from_source` 的文档承诺的就是这一代。WebP 改造时
///    只保留了指纹命名的两代、漏掉了它，导致从更老版本直升的用户旧缩略图
///    不再复用、首次打开相册全量重新解码生成。
fn reuse_legacy_thumb(
    album_id: i64,
    source: &Path,
    thumbs_dir: &Path,
    thumb_path: &Path,
) -> bool {
    let fp = file_fingerprint(source);
    let mut candidates: Vec<PathBuf> = vec![
        // 上一代：网格缩略图 grid/album_<id>_photo_<fp>.jpg
        thumbs_dir
            .join(GRID_THUMBS_SUBDIR)
            .join(format!("album_{album_id}_photo_{fp}.jpg")),
        // 上一代：封面 album_<id>_auto_<fp>.jpg
        thumbs_dir.join(format!("album_{album_id}_auto_{fp}.jpg")),
    ];
    // 最早一代：album_<id>_<safe_stem>.jpg（按源图文件名）
    let safe_stem = safe_stem_of(source);
    if !safe_stem.is_empty() {
        candidates.push(thumbs_dir.join(format!("album_{album_id}_{safe_stem}.jpg")));
    }
    for legacy in candidates {
        if legacy.is_file() {
            if let Some(parent) = thumb_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            return std::fs::copy(&legacy, thumb_path).is_ok();
        }
    }
    false
}

/// 源图文件名的安全化形式（最早一代缩略图命名 `album_<id>_<safe_stem>.jpg` 用）：
/// 只保留字母数字与 `-` `_`，最长 40 字符（与基线时代实现逐字一致，否则旧缓存名字对不上）
fn safe_stem_of(source: &Path) -> String {
    source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(40)
        .collect()
}

pub fn cleanup_album_auto_thumbs(album_id: i64, thumbs_dir: &Path) {
    let prefix = format!("album_{album_id}_auto");
    if let Ok(entries) = std::fs::read_dir(thumbs_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(&prefix) {
                let _ = if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    std::fs::remove_dir_all(entry.path())
                } else {
                    std::fs::remove_file(entry.path())
                };
            }
        }
    }
}

/// 删除相册的手动封面缓存文件（P1 WebP：支持分目录）
pub fn cleanup_album_manual_thumbs(album_id: i64, thumbs_dir: &Path) {
    let prefix = format!("album_{album_id}_manual");
    if let Ok(entries) = std::fs::read_dir(thumbs_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(&prefix) {
                let _ = if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    std::fs::remove_dir_all(entry.path())
                } else {
                    std::fs::remove_file(entry.path())
                };
            }
        }
    }
}

/// 删除相册的全部缩略图缓存文件（自动 + 手动封面 + 网格缩略图）
pub fn cleanup_all_album_thumbs(album_id: i64, thumbs_dir: &Path) {
    let prefix = format!("album_{album_id}_");
    if let Ok(entries) = std::fs::read_dir(thumbs_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(&prefix) {
                let _ = if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    std::fs::remove_dir_all(entry.path())
                } else {
                    std::fs::remove_file(entry.path())
                };
            }
        }
    }
    // 网格缩略图在 grid/ 子目录，需单独清理
    cleanup_album_grid_thumbs(album_id, thumbs_dir);
}

/// 按**内容**解码（`image::open` 按扩展名猜格式，会把 PNG 内容叫 .jpg 的文件判成 JPEG）
fn open_any_image(source: &Path) -> Result<image::DynamicImage, ThumbError> {
    let bytes = std::fs::read(source).map_err(ThumbError::Io)?;
    image::load_from_memory(&bytes).map_err(ThumbError::Image)
}

/// 判断文件名是否为 JPEG（.jpg / .jpeg）
fn is_jpeg_path(file_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    lower.ends_with(".jpg") || lower.ends_with(".jpeg")
}

// =====================================================================
// 全局生成闸门（BUG-2026-1023-009 进程级崩溃的根治措施之一）
// =====================================================================
//
// 事故现象：点开时间线后进程直接消失，终端只有
//   `thread '<unknown>' (23492) has overflowed its stack`
//   `exit code: 0xc00000fd (STATUS_STACK_OVERFLOW)`
// 而 app.log 停在最后一条 get_photo_thumbs 上，没有任何 panic 与堆栈。
//
// 链式原因（实测数据，见 bug-notes BUG-2026-1023-009）：
// 1. FEAT-1021 把缩略图渲染版本升到 v2（EXIF 转正），两个缓存键
//    （`thumb_photo_hash` 的表主键 + `file_fingerprint` 的磁盘文件名）
//    一起变 → 全库 2.5 万张缩略图**一次性全部失效**（v0.4.2 是热缓存，
//    所以那个版本不炸）；
// 2. 时间线页按 album 分组后 `Promise.all`，一次点开就发出 129 个
//    `get_photo_thumbs`，单个相册最多 2876 条路径；
// 3. 后端每个请求内部再 `thread::scope` 开 min(核数,8) 个线程现场解码 →
//    进程线程数实测冲到 **732**、私有内存 **1.2GB**，冷缓存时还要乘上
//    每张 2000 万像素 JPEG 的解码峰值；
// 4. 线程栈在内存/提交量被打穿时无法继续提交，Windows 把这种失败报成
//    0xC00000FD（= STATUS_STACK_OVERFLOW）→ 进程 abort，正在写盘的缩略图
//    被截成 0 字节（崩溃现场实测留下 20 个 0 字节文件）。
//
// 因此「谁想生成」与「同时允许几个人真去解码」必须解耦：
// 本闸门让全进程同时解码/编码的线程数恒定 ≤ [`gen_max`]，与请求路数无关。
// 前端另有限流（`src/utils/thumbQueue.ts`）、并去掉单请求内部的二次开线程
// （见 `ensure_grid_thumbs_with_lookup`），三层一起保证不再出现线程风暴。

/// 当前正在生成的数量（受 [`GEN_GATE_CV`] 与 [`gen_max`] 约束）
static GEN_GATE: Mutex<usize> = Mutex::new(0);
/// 闸门通知：有线程释放名额时唤醒等待者
static GEN_GATE_CV: Condvar = Condvar::new();
/// 并发上限（首次使用时按 CPU 核数定，恒定不再变）
static GEN_MAX: OnceLock<usize> = OnceLock::new();

/// 同时生成的缩略图张数上限。
///
/// 取 `min(核数, 4)`：解码+WebP 编码本身是 CPU 密集型，超过核数只有
/// 线程切换开销；4 已经能让单张相册的首次填充跑满常见 4~8 核的机器，
/// 又不会在请求叠加时把内存打穿。
fn gen_max() -> usize {
    *GEN_MAX.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 4)
    })
}

/// 生成名额的 RAII 句柄：drop 即归还并唤醒一个等待者
struct GenPermit;

impl Drop for GenPermit {
    fn drop(&mut self) {
        // 锁中毒也要归还名额（否则一次 panic 会永久堵死闸门）
        let mut cur = GEN_GATE.lock().unwrap_or_else(|e| e.into_inner());
        *cur = cur.saturating_sub(1);
        GEN_GATE_CV.notify_one();
    }
}

/// 申请一个生成名额（超出上限时阻塞等待，不是丢弃任务）
fn acquire_gen_permit() -> GenPermit {
    let max = gen_max();
    let mut cur = GEN_GATE.lock().unwrap_or_else(|e| e.into_inner());
    while *cur >= max {
        cur = GEN_GATE_CV.wait(cur).unwrap_or_else(|e| e.into_inner());
    }
    *cur += 1;
    GenPermit
}

/// 缩略图临时文件名：`<原名>.tmp<pid>_<seq>`（与目标同目录，保证 rename 同卷）
fn temp_thumb_path(thumb_path: &Path) -> PathBuf {
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = thumb_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push_str(&format!(".tmp{}_{}", std::process::id(), seq));
    thumb_path.with_file_name(name)
}

/// JPEG DCT 降采样解码（只解所需 DCT 块，比全尺寸解码快 16~64 倍）
///
/// 修复：`image::open` 全尺寸解码 6000x4000 大图需 5~14 秒（debug 构建），
/// 前端更换封面/生成缩略图卡顿。jpeg-decoder 0.3.1+ 的 `scale` API 直接
/// 请求目标尺寸（内部自动选择 IDCT 降采样），输出接近目标后由调用方
/// `thumbnail` 收尾到精确尺寸。
fn decode_jpeg_scaled(source: &Path, target_px: u32) -> Result<image::RgbImage, ThumbError> {
    use std::io::BufReader;
    let file = std::fs::File::open(source)?;
    let mut decoder = jpeg_decoder::Decoder::new(BufReader::new(file));
    // 直接请求目标尺寸：jpeg-decoder 按需只解码对应的 DCT 块，输出接近 target_px
    let _ = decoder.scale(target_px as u16, target_px as u16)?;
    let pixels = decoder.decode()?;
    let info = decoder.info().ok_or(ThumbError::Decode)?;
    image::RgbImage::from_raw(info.width as u32, info.height as u32, pixels)
        .ok_or(ThumbError::Decode)
}

/// P1 WebP 优化：256px 缩略图统一存为 WebP（质量 85，比 JPEG 小 30~50%）
/// - JPEG 源图走 DCT 降采样快速路径；其他格式全尺寸解码
fn save_thumbnail(source: &Path, thumb_path: &Path) -> Result<(), ThumbError> {
    // BUG-2026-1014-002：缩略图必须按 EXIF Orientation 转正，否则网格里横拍照片
    // 躺着显示（WebP 不携带 EXIF，浏览器无从补救）。
    //
    // 顺序要点：**先转正、再缩放**。`thumbnail()` 按「当前长边」等比缩放，
    // 若先缩放，转置类方向（6/8）会把躺倒的那条长边当成基准，得到的构图与
    // 正置图不同。旋转不改变长边长度，故在降采样图上转是等价且省 CPU 的。
    let orientation = crate::exif_orient::read_orientation(source);
    let oriented = |img: image::DynamicImage| -> image::DynamicImage {
        crate::exif_orient::apply_orientation(img, orientation)
    };

    // 全局闸门：从这里到函数返回，本线程占一个生成名额。
    // 放置点选在解码之前、参数准备之后，保证「排在闸门外的请求」不占 CPU 与内存。
    let _permit = acquire_gen_permit();

    let thumb = if is_jpeg_path(
        source
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
            .as_str(),
    ) {
        match decode_jpeg_scaled(source, THUMB_SIZE) {
            Ok(img) => oriented(image::DynamicImage::ImageRgb8(img))
                .thumbnail(THUMB_SIZE, THUMB_SIZE),
            // 降采样失败（异常 JPEG / 名不副实的 PNG）降级：按**内容**解码
            Err(_) => match open_any_image(source) {
                Ok(img) => oriented(img).thumbnail(THUMB_SIZE, THUMB_SIZE),
                Err(e) => return Err(e),
            },
        }
    } else {
        // 非 .jpg 扩展名 / 内容不是 JPEG：按魔数解码（不信扩展名）
        oriented(open_any_image(source)?).thumbnail(THUMB_SIZE, THUMB_SIZE)
    };
    if let Some(parent) = thumb_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 原子落盘：先写同目录临时文件，再改名到目标。
    //
    // 为什么必须（BUG-2026-1023-009）：同一张照片会被多个入口同时请求
    // （时间线 / 回忆 / 相册详情各自按 album 分组取），直接写目标文件时
    // 一个线程 `File::create` 刚截断、另一个线程还在写 → 崩溃或断电就会
    // 留下半截/0 字节残骸；现在目标文件要么不存在，要么完整。
    // P1: WebP 编码，质量 85（压缩率远优于 JPEG）
    let tmp_path = temp_thumb_path(thumb_path);
    thumb.save_with_format(&tmp_path, ImageFormat::WebP)?;
    // Windows 的 rename 不允许覆盖已存在文件：先删目标再改名
    if std::fs::rename(&tmp_path, thumb_path).is_err() {
        let _ = std::fs::remove_file(thumb_path);
        if let Err(e) = std::fs::rename(&tmp_path, thumb_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(ThumbError::Io(e));
        }
    }
    Ok(())
}

/// 基于已知原图路径生成缩略图（若无缓存则生成），返回缓存路径
///
/// - P1 WebP + 分目录：`album_<id>_auto_<subdir>/<fingerprint>.webp`
/// - 缓存命中则直接返回，避免重复生成
/// - 生成前清理该相册旧的自动缩略图，避免指纹变更后留下孤儿文件
pub fn ensure_thumbnail_from_source(
    album_id: i64,
    source: &Path,
    thumbs_dir: &Path,
) -> Result<ThumbResult, ThumbError> {
    let fingerprint = file_fingerprint(source);
    let subdir = webp_subdir(&fingerprint);
    let cached_name = format!("album_{album_id}_auto_{subdir}/{fingerprint}.webp");
    let thumb_path = thumbs_dir.join(&cached_name);

    // 若缓存已存在则直接复用
    if thumb_path.exists() {
        return Ok(ThumbResult {
            thumb_path: thumb_path.to_string_lossy().into_owned(),
            source_path: source.to_string_lossy().into_owned(),
        });
    }

    // 指纹缓存未命中：尝试复用旧版命名缩略图（基线 era 的 `album_{id}_{safe_stem}.jpg`，
    // 基于源图文件名）。老用户升级后所有旧缩略图立即复用，避免首次全量重新生成
    // 导致列表加载卡死（修复：每次重编译后指纹全变 → 缓存全失效 → 全量重生成的卡死 bug）
    if reuse_legacy_thumb(album_id, source, thumbs_dir, &thumb_path) {
        return Ok(ThumbResult {
            thumb_path: thumb_path.to_string_lossy().into_owned(),
            source_path: source.to_string_lossy().into_owned(),
        });
    }

    // 清理旧指纹缓存（文件名变更才会走到这里）
    cleanup_album_auto_thumbs(album_id, thumbs_dir);

    // 生成缩略图（JPEG 走 DCT 降采样快速路径）
    save_thumbnail(source, &thumb_path)?;

    Ok(ThumbResult {
        thumb_path: thumb_path.to_string_lossy().into_owned(),
        source_path: source.to_string_lossy().into_owned(),
    })
}

/// 为用户手动指定的封面图片生成缩略图
///
/// 将任意图片路径生成 256px 缩略图写入缓存目录，
/// 并返回缓存路径（用于写入 Album.cover_path）。
/// 这样手动封面与自动缩略图都统一存放在 `thumbs/` 下，
/// 便于 asset 协议统一授权、加载一致且更快。
pub fn generate_cover(
    album_id: i64,
    source: &Path,
    thumbs_dir: &Path,
) -> Result<String, ThumbError> {
    // P1 WebP + 分目录
    let fingerprint = file_fingerprint(source);
    let subdir = webp_subdir(&fingerprint);
    let cached_name = format!("album_{album_id}_manual_{subdir}/{fingerprint}.webp");
    let thumb_path = thumbs_dir.join(&cached_name);

    if thumb_path.exists() {
        return Ok(thumb_path.to_string_lossy().into_owned());
    }

    // 清理旧封面缓存（换图才会走到这里）
    cleanup_album_manual_thumbs(album_id, thumbs_dir);

    // 生成缩略图（JPEG 走 DCT 降采样快速路径）
    save_thumbnail(source, &thumb_path)?;

    Ok(thumb_path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 网格缩略图：生成 + 缓存命中 + 与封面缩略图互不干扰（独立 grid/ 子目录）
    #[test]
    fn grid_thumb_generate_and_cache() {
        let tmp = std::env::temp_dir().join(format!("pm_grid_thumb_{}", std::process::id()));
        let img_dir = tmp.join("photos");
        std::fs::create_dir_all(&img_dir).unwrap();

        let img_path = img_dir.join("a.jpg");
        let img = image::RgbImage::new(100, 80);
        img.save(&img_path).unwrap();
        let src = img_path.to_string_lossy().into_owned();

        let thumbs = tmp.join("thumbs");
        // 首次：生成（落在 thumbs/grid/ 子目录）
        let pairs = ensure_grid_thumbs_with_lookup(
            7,
            &[src.clone()],
            &thumbs,
            &|_hashes| std::collections::HashMap::new(), // 测试中不走表
            |_gen| {},
        );
        assert_eq!(pairs.len(), 1);
        let (p, t) = &pairs[0];
        assert_eq!(p, &src);
        assert!(t.contains("grid"));
        assert!(t.ends_with(".jpg"));
        assert!(Path::new(t).exists());

        // 二次：缓存命中（目录内文件数不增长）
        let before = std::fs::read_dir(thumbs.join("grid")).unwrap().count();
        ensure_grid_thumbs_with_lookup(
            7,
            &[src.clone()],
            &thumbs,
            &|_hashes| std::collections::HashMap::new(),
            |_gen| {},
        );
        let after = std::fs::read_dir(thumbs.join("grid")).unwrap().count();
        assert_eq!(before, after);

        // 封面清理不影响网格缩略图（auto 前缀只扫根目录）
        let res = ensure_thumbnail_from_source(7, Path::new(&src), &thumbs).unwrap();
        cleanup_album_auto_thumbs(7, &thumbs);
        assert!(!Path::new(&res.thumb_path).exists());
        assert!(Path::new(t).exists());

        // 删除相册：网格缩略图一并清理
        cleanup_all_album_thumbs(7, &thumbs);
        assert!(!Path::new(t).exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 并行生成路径：批量（≥2×核数）未命中项走 scoped threads，全部生成且
    /// 二次调用全命中缓存（文件数不增长 = 不重复生成）
    #[test]
    fn parallel_generation_bulk_and_cache_hit() {
        let tmp = std::env::temp_dir().join(format!("pm_par_thumb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let img_dir = tmp.join("photos");
        std::fs::create_dir_all(&img_dir).unwrap();

        let mut sources: Vec<String> = Vec::new();
        for i in 0..24 {
            let p = img_dir.join(format!("p{i:02}.jpg"));
            image::RgbImage::new(120, 90).save(&p).unwrap();
            sources.push(p.to_string_lossy().into_owned());
        }
        let thumbs = tmp.join("thumbs");

        // 首次：并行批量生成，24 张全部产出
        let t0 = std::time::Instant::now();
        let pairs = ensure_grid_thumbs_with_lookup(
            11,
            &sources,
            &thumbs,
            &|_hashes| std::collections::HashMap::new(),
            |_gen| {},
        );
        eprintln!("并行生成 24 张耗时: {}ms", t0.elapsed().as_millis());
        assert_eq!(pairs.len(), 24, "全部应生成成功");
        for (_, thumb) in &pairs {
            assert!(Path::new(thumb).exists());
        }

        // 二次：磁盘缓存命中（指纹命名不变），目录文件数不增长 = 不重复生成
        let before = std::fs::read_dir(thumbs.join("grid")).unwrap().count();
        let pairs2 = ensure_grid_thumbs_with_lookup(
            11,
            &sources,
            &thumbs,
            &|_hashes| std::collections::HashMap::new(),
            |_gen| {},
        );
        let after = std::fs::read_dir(thumbs.join("grid")).unwrap().count();
        assert_eq!(pairs2.len(), 24);
        assert_eq!(before, after, "二次调用不得新增缓存文件（防重复生成）");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// P19：语义腿的并行缩略图生成 —— 输出顺序不依赖线程完成顺序、
    /// 进度回调每张恰好一次、小样本退化串行仍正确。
    ///
    /// 回归背景：旧版是单线程串行且零埋点，412 张耗时 169.8s 成为三腿并发的关键路径
    /// （BUG-2026-0929-010）。这里同时锁住「换成并行后语义不变」。
    #[test]
    fn missing_parallel_preserves_order_and_reports_progress() {
        let tmp = std::env::temp_dir().join(format!("pm_missing_par_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let img_dir = tmp.join("photos");
        std::fs::create_dir_all(&img_dir).unwrap();

        // hash 用可预测值：用来断言「输出与输入同序」而不只是「个数对」
        let mut missing: Vec<(String, String)> = Vec::new();
        for i in 0..24 {
            let p = img_dir.join(format!("p{i:02}.jpg"));
            image::RgbImage::new(120, 90).save(&p).unwrap();
            missing.push((p.to_string_lossy().into_owned(), format!("h{i:02}")));
        }
        let thumbs = tmp.join("thumbs");

        let progress = std::sync::atomic::AtomicUsize::new(0);
        let last_total = std::sync::atomic::AtomicUsize::new(0);
        let out = ensure_grid_thumbs_missing_parallel(
            77,
            &missing,
            &thumbs,
            9,
            Some(&|_done, total| {
                progress.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                last_total.store(total, std::sync::atomic::Ordering::Relaxed);
            }),
        );

        assert_eq!(out.len(), 24, "全部应生成成功");
        for (i, (src, hash, thumb)) in out.iter().enumerate() {
            assert_eq!(hash, &format!("h{i:02}"), "输出必须与输入同序（不依赖线程完成顺序）");
            assert_eq!(src, &missing[i].0);
            assert!(Path::new(thumb).exists(), "缩略图文件应存在");
        }
        assert_eq!(
            progress.load(std::sync::atomic::Ordering::Relaxed),
            24,
            "进度回调每张恰好一次"
        );
        assert_eq!(last_total.load(std::sync::atomic::Ordering::Relaxed), 24);

        // 空输入：不生成、不回调
        let empty = ensure_grid_thumbs_missing_parallel(77, &[], &thumbs, 9, None);
        assert!(empty.is_empty(), "空输入应返回空");

        // 小样本（< 2×线程数）走串行退化分支，结果必须一致
        let small: Vec<(String, String)> = missing.iter().take(3).cloned().collect();
        let thumbs2 = tmp.join("thumbs2");
        let out2 = ensure_grid_thumbs_missing_parallel(77, &small, &thumbs2, 9, None);
        assert_eq!(out2.len(), 3, "串行退化分支应同样产出 3 张");
        for (i, (_, hash, _)) in out2.iter().enumerate() {
            assert_eq!(hash, &format!("h{i:02}"), "串行分支也要保序");
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// P19 实测基准：语义腿缩略图生成 串行（旧） vs 并行（新）。
    ///
    /// 需要**真实照片**（合成 120×90 小图测不出差异，解码成本不在一个量级）：
    /// ```text
    /// PM_THUMB_BENCH_DIR="<相册目录>" [PM_THUMB_BENCH_N=96] \\
    ///   cargo test --lib thumb_bench_serial_vs_parallel -- --ignored --nocapture
    /// ```
    #[ignore = "真实照片基准：设 PM_THUMB_BENCH_DIR 后手动运行"]
    #[test]
    fn thumb_bench_serial_vs_parallel() {
        let dir = std::env::var("PM_THUMB_BENCH_DIR")
            .expect("请设置 PM_THUMB_BENCH_DIR=<相册目录>");
        let n_take: usize = std::env::var("PM_THUMB_BENCH_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(96);

        let all = list_album_images(Path::new(&dir));
        assert!(!all.is_empty(), "目录无图片: {dir}");
        let sources: Vec<String> = all.into_iter().take(n_take).collect();
        let n = sources.len();

        // 预热页缓存：两边都读同一批源文件，先把磁盘读延迟从计时里摘掉
        for s in sources.iter().take(8) {
            let _ = std::fs::read(s);
        }

        // 两个独立 thumbs 目录，保证两边都真生成（不命中对方的磁盘缓存）
        let base = std::env::temp_dir().join(format!("thumb_bench_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir_serial = base.join("serial");
        let dir_parallel = base.join("parallel");

        // —— 串行（旧实现：语义腿原来就是这段 filter_map）——
        let t0 = std::time::Instant::now();
        let serial: Vec<String> = sources
            .iter()
            .filter_map(|src| ensure_grid_thumb(77, Path::new(src), &dir_serial, None, 0).ok())
            .collect();
        let ms_serial = t0.elapsed().as_millis();

        // —— 并行（P19 新实现）——
        let missing: Vec<(String, String)> = sources
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), format!("h{i:04}")))
            .collect();
        let t1 = std::time::Instant::now();
        let parallel = ensure_grid_thumbs_missing_parallel(77, &missing, &dir_parallel, 0, None);
        let ms_parallel = t1.elapsed().as_millis();

        assert_eq!(serial.len(), n, "串行应全部成功");
        assert_eq!(parallel.len(), n, "并行应全部成功");
        // 顺序一致性：并行版产出的 photo_hash 必须与输入同序
        for (i, (_, hash, _)) in parallel.iter().enumerate() {
            assert_eq!(hash, &format!("h{i:04}"), "并行输出必须保序");
        }

        let speedup = if ms_parallel > 0 {
            ms_serial as f64 / ms_parallel as f64
        } else {
            f64::INFINITY
        };
        eprintln!(
            "[thumb_bench] 样本 {n} 张 | 串行(旧) {ms_serial}ms ({:.0}ms/张) | 并行(新) {ms_parallel}ms ({:.0}ms/张) | 加速 {speedup:.2}x",
            ms_serial as f64 / n as f64,
            ms_parallel as f64 / n as f64,
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 指纹确定性：同一文件多次计算必须完全一致（回归 DefaultHasher 随机种子 bug：
    /// 种子每次编译时随机 → 重编译后缓存全失效 → 每次启动全量重生成缩略图卡死）
    #[test]
    fn fingerprint_deterministic() {
        let tmp = std::env::temp_dir().join(format!("fp_det_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let img_path = tmp.join("photo.jpg");
        let img = image::RgbImage::new(64, 64);
        img.save(&img_path).unwrap();
        let fp1 = file_fingerprint(&img_path);
        let fp2 = file_fingerprint(&img_path);
        let fp3 = file_fingerprint(&img_path);
        assert_eq!(fp1, fp2, "同进程内指纹必须稳定");
        assert_eq!(fp2, fp3);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 旧命名缩略图复用：三代旧缓存都应被整字节复制为新的指纹文件，
    /// 老用户升级后无需重新解码大图（否则首次打开相册会全量重生成）。
    #[test]
    fn legacy_thumb_reuse() {
        let tmp = std::env::temp_dir().join(format!("legacy_reuse_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);

        // 每代旧缓存用独立的 thumbs 目录 + 相册 id，避免上一轮生成的 .webp 命中缓存
        // 而直接 return（那就测不到复用分支了）
        let make_source = |dir: &Path| -> PathBuf {
            let img_dir = dir.join("album_dir");
            std::fs::create_dir_all(&img_dir).unwrap();
            let p = img_dir.join("DSC_0001.jpg");
            image::RgbImage::new(80, 60).save(&p).unwrap();
            p
        };
        let make_legacy = |path: &Path| {
            image::RgbImage::new(80, 60).save(path).unwrap();
        };

        // ① 最早一代：thumbs/album_<id>_<safe_stem>.jpg（按源图文件名）
        {
            let dir = tmp.join("gen1");
            let img = make_source(&dir);
            let thumbs = dir.join("thumbs");
            std::fs::create_dir_all(&thumbs).unwrap();
            let legacy = thumbs.join("album_99_DSC_0001.jpg");
            make_legacy(&legacy);
            let res = ensure_thumbnail_from_source(99, &img, &thumbs).unwrap();
            assert!(Path::new(&res.thumb_path).exists());
            assert!(res.thumb_path.ends_with(".webp"), "新缓存应为 .webp：{}", res.thumb_path);
            assert_eq!(
                std::fs::read(&legacy).unwrap(),
                std::fs::read(&res.thumb_path).unwrap(),
                "按文件名命名的基线旧缩略图应被复用"
            );
            // 再次调用应命中新缓存（幂等）
            let res2 = ensure_thumbnail_from_source(99, &img, &thumbs).unwrap();
            assert_eq!(res.thumb_path, res2.thumb_path);
        }

        // ② 上一代：thumbs/grid/album_<id>_photo_<指纹>.jpg
        {
            let dir = tmp.join("gen2");
            let img = make_source(&dir);
            let thumbs = dir.join("thumbs");
            let grid = thumbs.join(GRID_THUMBS_SUBDIR);
            std::fs::create_dir_all(&grid).unwrap();
            let fp = file_fingerprint(&img);
            let legacy = grid.join(format!("album_98_photo_{fp}.jpg"));
            make_legacy(&legacy);
            let res = ensure_thumbnail_from_source(98, &img, &thumbs).unwrap();
            assert_eq!(
                std::fs::read(&legacy).unwrap(),
                std::fs::read(&res.thumb_path).unwrap(),
                "上一代指纹命名的 grid 旧缩略图应被复用"
            );
        }

        // ③ 上一代封面：thumbs/album_<id>_auto_<指纹>.jpg
        {
            let dir = tmp.join("gen3");
            let img = make_source(&dir);
            let thumbs = dir.join("thumbs");
            std::fs::create_dir_all(&thumbs).unwrap();
            let fp = file_fingerprint(&img);
            let legacy = thumbs.join(format!("album_97_auto_{fp}.jpg"));
            make_legacy(&legacy);
            let res = ensure_thumbnail_from_source(97, &img, &thumbs).unwrap();
            assert_eq!(
                std::fs::read(&legacy).unwrap(),
                std::fs::read(&res.thumb_path).unwrap(),
                "上一代指纹命名的封面旧缩略图应被复用"
            );
        }

        // ④ 无旧缓存时正常生成（不能因为“加了复用”就把生成路径弄坏）
        {
            let dir = tmp.join("gen4");
            let img = make_source(&dir);
            let thumbs = dir.join("thumbs");
            std::fs::create_dir_all(&thumbs).unwrap();
            let res = ensure_thumbnail_from_source(96, &img, &thumbs).unwrap();
            assert!(Path::new(&res.thumb_path).exists());
            assert!(res.thumb_path.ends_with(".webp"));
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 递归文件计数：含子目录、跳过隐藏目录、不读内容
    #[test]
    fn count_files_recursive_ok() {
        let tmp = std::env::temp_dir().join(format!("count_files_test_{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("sub/.hidden")).unwrap();
        std::fs::create_dir_all(tmp.join("sub2")).unwrap();
        for f in ["a.jpg", "b.png", "c.txt"] {
            std::fs::write(tmp.join(f), b"x").unwrap();
        }
        std::fs::write(tmp.join("sub/d.jpg"), b"x").unwrap();
        std::fs::write(tmp.join("sub/.hidden/e.jpg"), b"x").unwrap();
        std::fs::write(tmp.join("sub2/f.webp"), b"x").unwrap();
        // 3 顶层 + 1 子目录 + 1 子目录2 = 5（隐藏目录内不计）
        assert_eq!(count_files_recursive(&tmp), 5);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// mtime 兜底：无 EXIF 的图片，read_shoot_time 应回退到文件修改时间
    #[test]
    fn shoot_time_mtime_fallback() {
        let tmp = std::env::temp_dir().join(format!("mtime_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let img_path = tmp.join("no_exif.jpg");
        // image crate 保存的 JPEG 不含 EXIF DateTimeOriginal
        let img = image::RgbImage::new(64, 48);
        img.save(&img_path).unwrap();
        // EXIF 读取应失败 → 走 mtime 兜底
        let t = read_shoot_time(&img_path);
        assert!(t.is_some(), "无 EXIF 时应回退 mtime 返回日期");
        let date = t.unwrap();
        // 格式 YYYY-MM-DD 且年份合理
        assert_eq!(date.len(), 10);
        let year: i32 = date[0..4].parse().unwrap();
        assert!((2000..2100).contains(&year), "年份异常: {date}");
        // 与文件 mtime 的日期一致
        let meta = std::fs::metadata(&img_path).unwrap();
        let modified = meta.modified().unwrap();
        let secs = modified.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let (y, m, d) = unix_secs_to_date(secs);
        assert_eq!(date, format!("{y:04}-{m:02}-{d:02}"), "日期应与 mtime 一致");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 公历转换正确性（已知日期验证）
    #[test]
    fn unix_date_known_values() {
        // 1970-01-01
        assert_eq!(unix_secs_to_date(0), (1970, 1, 1));
        // 2000-01-01
        assert_eq!(unix_secs_to_date(946_684_800), (2000, 1, 1));
        // 2024-02-29（闰年）
        assert_eq!(unix_secs_to_date(1_709_164_800), (2024, 2, 29));
        // 2025-12-31
        assert_eq!(unix_secs_to_date(1_767_139_200), (2025, 12, 31));
        // 2026-01-01
        assert_eq!(unix_secs_to_date(1_767_225_600), (2026, 1, 1));
    }

    // -----------------------------------------------------------------
    // BUG-2026-1023-009：键空间约定（表主键与扫描侧同源，版本只进文件名）
    // -----------------------------------------------------------------

    /// 表主键必须与 `content::photo_hash` **逐字同源**。
    ///
    /// 事故：曾经把渲染版本掺进表主键 → `photo_thumb_cache` 与
    /// `photo_content_scan` 的 join（`count_scanned_without_thumb`）永远对不上，
    /// 系统认为「全都有缩略图」而不预热，懒加载侧又全 miss。
    #[test]
    fn table_key_matches_content_photo_hash() {
        let path = Path::new("D:/photos/IMG_0001.jpg");
        let (len, mtime) = (1_234_567u64, 1_700_000_000_123_456_789u128);
        assert_eq!(
            thumb_photo_hash(path, len, mtime),
            crate::content::photo_hash(&path.to_string_lossy(), len, mtime),
            "表主键必须与扫描侧 photo_hash 完全一致（不得掺渲染版本）"
        );
    }

    /// 渲染版本必须只影响**文件名**，不影响表主键。
    /// bump 版本时这个测试会提醒：表行可以原地复用，只是文件名换了。
    #[test]
    fn render_version_only_in_file_name() {
        let tmp = std::env::temp_dir().join(format!("ver_split_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let img_path = tmp.join("a.jpg");
        image::RgbImage::new(32, 32).save(&img_path).unwrap();

        let (len, mtime) = read_file_meta(&img_path).unwrap();
        let key = thumb_photo_hash(&img_path, len, mtime);
        assert_eq!(key, crate::content::photo_hash(&img_path.to_string_lossy(), len, mtime));

        let fp = file_fingerprint(&img_path);
        assert!(
            fp.starts_with(&format!("v{THUMB_RENDER_VERSION}_")),
            "文件名指纹应带渲染版本前缀，实际: {fp}"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 旧键函数：能算出 v2 时代的键（迁移靠它把老行认回来）
    #[test]
    fn legacy_hash_differs_from_current_hash() {
        let path = Path::new("D:/photos/IMG_0002.jpg");
        let (len, mtime) = (999u64, 42u128);
        let new_key = thumb_photo_hash(path, len, mtime);
        let old_key = thumb_photo_hash_v1(path, len, mtime, THUMB_RENDER_VERSION);
        assert_ne!(new_key, old_key, "旧键掺了版本号，必须与新键不同");
        assert_eq!(old_key.len(), 16);
    }

    /// 生成测试图片并验证扫描与缩略图
    #[test]
    fn find_and_thumb() {
        use image::{Rgb, RgbImage};

        // 构造临时相册目录（先清理，避免上轮残留影响断言）
        let tmp = std::env::temp_dir().join("pm_thumb_test");
        let _ = std::fs::remove_dir_all(&tmp);
        let img_dir = tmp.join("photos");
        std::fs::create_dir_all(&img_dir).unwrap();

        // 生成一张测试图
        let img_path = img_dir.join("first.png");
        let mut img = RgbImage::new(100, 50);
        for px in img.pixels_mut() {
            *px = Rgb([200, 100, 50]);
        }
        img.save(&img_path).unwrap();

        // 单次遍历扫描应找到首图、统计数量与大小
        let scan = scan_album_dir(&img_dir);
        assert_eq!(scan.photo_count, 1);
        assert!(scan.size_bytes > 0);
        let first = scan.first_image.unwrap();
        assert_eq!(first.file_name().unwrap(), "first.png");

        // 基于源图生成缩略图
        let thumbs = tmp.join("thumbs");
        let res = ensure_thumbnail_from_source(1, &first, &thumbs).unwrap();
        assert!(Path::new(&res.thumb_path).exists());
        assert!(res.thumb_path.ends_with(".webp"), "缩略图已改 WebP：{}", res.thumb_path);

        // 二次调用应命中缓存
        let res2 = ensure_thumbnail_from_source(1, &first, &thumbs).unwrap();
        assert_eq!(res2.thumb_path, res.thumb_path);

        // 同一路径文件内容变化 → 指纹变化 → 缓存文件名变更（旧缓存被清理）
        let mut img2 = RgbImage::new(100, 50);
        for px in img2.pixels_mut() {
            *px = Rgb([10, 200, 90]);
        }
        img2.save(&img_path).unwrap();
        let first2 = scan_album_dir(&img_dir).first_image.unwrap();
        let res3 = ensure_thumbnail_from_source(1, &first2, &thumbs).unwrap();
        assert_ne!(res3.thumb_path, res.thumb_path, "内容变化后缓存文件名应变化");
        // 旧指纹缓存文件应已被清理，目录中只保留一个自动缩略图
        let auto_files: Vec<_> = std::fs::read_dir(&thumbs)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("album_1_auto_"))
            .collect();
        assert_eq!(auto_files.len(), 1, "应只保留一个自动缩略图");

        // 清理全部缓存
        cleanup_all_album_thumbs(1, &thumbs);
        let remaining: Vec<_> = std::fs::read_dir(&thumbs)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("album_1_"))
            .collect();
        assert_eq!(remaining.len(), 0, "删除相册后缓存应全部清理");
    }

    // -----------------------------------------------------------------
    // BUG-2026-1023-009 回归：缩略图线程风暴导致进程 0xC00000FD 退出
    // -----------------------------------------------------------------

    /// 闸门必须真的把「同时生成数」卡在 `gen_max()` 以内。
    ///
    /// 事故里 129 路请求各自开 8 线程 → 同时解码几十张 2000 万像素图，
    /// 线程数 732 / 私有内存 1.2GB，最后整进程以栈提交失败退出。
    #[test]
    fn gen_gate_caps_concurrent_generation() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CUR: AtomicUsize = AtomicUsize::new(0);
        static PEAK: AtomicUsize = AtomicUsize::new(0);

        let handles: Vec<_> = (0..16)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..4 {
                        let _permit = acquire_gen_permit();
                        let now = CUR.fetch_add(1, Ordering::SeqCst) + 1;
                        PEAK.fetch_max(now, Ordering::SeqCst);
                        // 持有名额一小会儿，给其它线程制造叠加机会
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        CUR.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let peak = PEAK.load(Ordering::SeqCst);
        assert!(peak <= gen_max(), "同时生成数 {peak} 超过闸门上限 {}", gen_max());
        assert_eq!(CUR.load(Ordering::SeqCst), 0, "名额必须全部归还");
    }

    /// 同一张照片被并发生成时必须原子落盘：
    /// 不得留下 0 字节残骸（崩溃现场有 20 个），也不得残留 `.tmp` 临时文件。
    #[test]
    fn concurrent_same_source_writes_atomically() {
        let tmp = std::env::temp_dir().join(format!("pm_atomic_thumb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let img_dir = tmp.join("photos");
        std::fs::create_dir_all(&img_dir).unwrap();
        let img_path = img_dir.join("same.jpg");
        image::RgbImage::new(160, 120).save(&img_path).unwrap();
        let src = img_path.to_string_lossy().into_owned();
        let thumbs = tmp.join("thumbs");

        // 8 个线程同时要同一张（时间线/回忆/相册详情会这样叠）
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let src = src.clone();
                let thumbs = thumbs.clone();
                scope.spawn(move || {
                    ensure_grid_thumbs_with_lookup(
                        3,
                        &[src],
                        &thumbs,
                        &|_hashes| std::collections::HashMap::new(),
                        |_gen| {},
                    )
                });
            }
        });

        let grid = thumbs.join(GRID_THUMBS_SUBDIR);
        let entries: Vec<_> = std::fs::read_dir(&grid).unwrap().flatten().collect();
        let mut thumbs_count = 0;
        for e in &entries {
            let name = e.file_name().to_string_lossy().into_owned();
            assert!(!name.contains(".tmp"), "不得残留临时文件：{name}");
            let len = e.metadata().unwrap().len();
            assert!(len > 0, "不得留下 0 字节缩略图：{name}");
            thumbs_count += 1;
        }
        assert_eq!(thumbs_count, 1, "同一指纹只应产出一个文件");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
