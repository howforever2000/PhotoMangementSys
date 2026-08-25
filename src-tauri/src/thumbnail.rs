//! 缩略图生成模块
//!
//! 职责：扫描相册文件夹，找到第一张图片，生成缩略图并缓存。
//! 缓存目录：`app_data_dir/thumbs/<相册id>_<时间戳>.jpg`
//!
//! 对应需求文档 §1 中"缩略图异步生成 (image-rs)"的简化版实现。
//! 当前为同步阻塞实现（列表/详情加载时调用），后续可升级为异步线程。

use std::path::{Path, PathBuf};

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
        if e.file_type().is_file() {
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
        if !e.file_type().is_file() {
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

/// 确保单个照片的网格缩略图存在，返回缓存绝对路径
///
/// - 缓存名：`thumbs/grid/album_<id>_photo_<fingerprint>.jpg`
/// - 指纹命名：文件内容变化 → 指纹变化 → 自动换名（与封面缩略图一致）
/// - 与封面缩略图互不干扰：封面清理只扫 `thumbs/` 根目录，网格缩略图在 `grid/` 子目录
/// - 网格只加载 256px 缩略图，避免网格展示原图造成内存/IO 压力
pub fn ensure_grid_thumb(
    album_id: i64,
    source: &Path,
    thumbs_dir: &Path,
) -> Result<String, ThumbError> {
    let grid_dir = thumbs_dir.join(GRID_THUMBS_SUBDIR);
    let fingerprint = file_fingerprint(source);
    let cached_name = format!("album_{album_id}_photo_{fingerprint}.jpg");
    let thumb_path = grid_dir.join(&cached_name);

    if thumb_path.exists() {
        return Ok(thumb_path.to_string_lossy().into_owned());
    }

    // 缓存未命中：生成（JPEG 走 DCT 降采样快速路径）
    save_thumbnail(source, &thumb_path)?;
    Ok(thumb_path.to_string_lossy().into_owned())
}

/// 批量确保网格缩略图存在（供前端分批懒加载），返回 `(原图路径, 缩略图路径)` 列表
///
/// 单张失败不影响其余：失败项跳过，前端可稍后重试。
pub fn ensure_grid_thumbs(
    album_id: i64,
    sources: &[String],
    thumbs_dir: &Path,
) -> Vec<(String, String)> {
    sources
        .iter()
        .filter_map(|s| {
            ensure_grid_thumb(album_id, Path::new(s), thumbs_dir)
                .ok()
                .map(|thumb| (s.clone(), thumb))
        })
        .collect()
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

/// 计算单张原图的网格缩略图缓存文件名（须在原图仍存在时调用：指纹依赖文件内容）
pub fn grid_thumb_cache_name(album_id: i64, source: &Path) -> String {
    format!("album_{album_id}_photo_{}.jpg", file_fingerprint(source))
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
        if !e.file_type().is_file() {
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

/// 删除相册的自动缩略图缓存文件（`album_{id}_auto_*.jpg`）
///
/// 在生成新缓存前调用，确保每个相册最多只有一个自动缩略图，
/// 避免图片变更后旧指纹文件成为孤儿占用磁盘。
pub fn cleanup_album_auto_thumbs(album_id: i64, thumbs_dir: &Path) {
    cleanup_album_prefix(album_id, "auto", thumbs_dir);
}

/// 删除相册的手动封面缓存文件（`album_{id}_manual_*.jpg`）
///
/// 用户更换封面图时调用，避免旧封面指纹文件成为孤儿。
pub fn cleanup_album_manual_thumbs(album_id: i64, thumbs_dir: &Path) {
    cleanup_album_prefix(album_id, "manual", thumbs_dir);
}

/// 按前缀清理相册的某类缩略图缓存（auto / manual）
fn cleanup_album_prefix(album_id: i64, kind: &str, thumbs_dir: &Path) {
    let prefix = format!("album_{album_id}_{kind}_");
    if let Ok(entries) = std::fs::read_dir(thumbs_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// 删除相册的全部缩略图缓存文件（自动 + 手动封面 + 网格缩略图）
///
/// 在删除相册记录成功后调用，清理对应缓存目录，避免磁盘持续增长。
pub fn cleanup_all_album_thumbs(album_id: i64, thumbs_dir: &Path) {
    let prefix = format!("album_{album_id}_");
    if let Ok(entries) = std::fs::read_dir(thumbs_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    // 网格缩略图在 grid/ 子目录，需单独清理
    cleanup_album_grid_thumbs(album_id, thumbs_dir);
}

/// 尝试复用旧版命名缩略图（基线 era 的 `album_{id}_{safe_stem}.jpg`，基于源图文件名）
///
/// 旧版缓存命名不含内容指纹，只要源图文件名不变即可命中。升级到指纹命名后，
/// 将旧文件直接复制为指纹文件名，老用户零成本迁移，无需重新解码大图生成。
fn reuse_legacy_thumb(
    album_id: i64,
    source: &Path,
    thumbs_dir: &Path,
    thumb_path: &Path,
) -> bool {
    let safe_stem: String = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(40)
        .collect();
    if safe_stem.is_empty() {
        return false;
    }
    let legacy = thumbs_dir.join(format!("album_{album_id}_{safe_stem}.jpg"));
    if legacy.is_file() {
        return std::fs::copy(&legacy, thumb_path).is_ok();
    }
    false
}

/// 判断文件名是否为 JPEG（.jpg / .jpeg）
fn is_jpeg_path(file_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    lower.ends_with(".jpg") || lower.ends_with(".jpeg")
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

/// 生成 256px 缩略图并保存为 JPEG（统一入口）
///
/// - JPEG 源图：走 DCT 降采样快速路径（快 16~64 倍）；若降采样失败自动降级全尺寸解码
/// - 其他格式（png/webp/gif/bmp）：保持 image::open 全尺寸解码 + thumbnail
fn save_thumbnail(source: &Path, thumb_path: &Path) -> Result<(), ThumbError> {
    let thumb = if is_jpeg_path(
        source
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
            .as_str(),
    ) {
        match decode_jpeg_scaled(source, THUMB_SIZE) {
            Ok(img) => image::DynamicImage::ImageRgb8(img).thumbnail(THUMB_SIZE, THUMB_SIZE),
            // 降采样失败（异常 JPEG）降级为全尺寸解码，保证可用性
            Err(_) => image::open(source)?.thumbnail(THUMB_SIZE, THUMB_SIZE),
        }
    } else {
        image::open(source)?.thumbnail(THUMB_SIZE, THUMB_SIZE)
    };
    if let Some(parent) = thumb_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    thumb.save_with_format(thumb_path, ImageFormat::Jpeg)?;
    Ok(())
}

/// 基于已知原图路径生成缩略图（若无缓存则生成），返回缓存路径
///
/// - 缓存文件名基于内容指纹：`album_<id>_auto_<fingerprint>.jpg`
/// - 缓存命中则直接返回，避免重复生成
/// - 生成前清理该相册旧的自动缩略图，避免指纹变更后留下孤儿文件
pub fn ensure_thumbnail_from_source(
    album_id: i64,
    source: &Path,
    thumbs_dir: &Path,
) -> Result<ThumbResult, ThumbError> {
    let fingerprint = file_fingerprint(source);
    let cached_name = format!("album_{album_id}_auto_{fingerprint}.jpg");
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
    // 缓存文件名基于内容指纹：用户更换封面图时生成新文件并清理旧文件，
    // 修复旧版"固定文件名导致换图后仍显示旧封面"的问题。
    let fingerprint = file_fingerprint(source);
    let cached_name = format!("album_{album_id}_manual_{fingerprint}.jpg");
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
        let pairs = ensure_grid_thumbs(7, &[src.clone()], &thumbs);
        assert_eq!(pairs.len(), 1);
        let (p, t) = &pairs[0];
        assert_eq!(p, &src);
        assert!(t.contains("grid"));
        assert!(t.ends_with(".jpg"));
        assert!(Path::new(t).exists());

        // 二次：缓存命中（目录内文件数不增长）
        let before = std::fs::read_dir(thumbs.join("grid")).unwrap().count();
        ensure_grid_thumbs(7, &[src.clone()], &thumbs);
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

    /// 旧命名缩略图复用：album_{id}_{safe_stem}.jpg（基线产物）应被复制为指纹文件，
    /// 老用户升级后无需重新解码大图生成缩略图
    #[test]
    fn legacy_thumb_reuse() {
        let tmp = std::env::temp_dir().join(format!("legacy_reuse_test_{}", std::process::id()));
        let thumbs = tmp.join("thumbs");
        let img_dir = tmp.join("album_dir");
        std::fs::create_dir_all(&thumbs).unwrap();
        std::fs::create_dir_all(&img_dir).unwrap();
        let img_path = img_dir.join("DSC_0001.jpg");
        let img = image::RgbImage::new(80, 60);
        img.save(&img_path).unwrap();
        // 构造基线时代的旧命名缩略图
        let legacy = thumbs.join("album_99_DSC_0001.jpg");
        let legacy_img = image::RgbImage::new(80, 60);
        legacy_img.save(&legacy).unwrap();
        let res = ensure_thumbnail_from_source(99, &img_path, &thumbs).unwrap();
        assert!(Path::new(&res.thumb_path).exists());
        // 指纹文件内容应与 legacy 完全一致（证明是复用而非重新生成）
        assert_eq!(
            std::fs::read(&legacy).unwrap(),
            std::fs::read(&res.thumb_path).unwrap(),
            "指纹文件应复用 legacy 缩略图内容"
        );
        // 再次调用应命中指纹缓存（幂等）
        let res2 = ensure_thumbnail_from_source(99, &img_path, &thumbs).unwrap();
        assert_eq!(res.thumb_path, res2.thumb_path);
        std::fs::remove_dir_all(&tmp).ok();
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

    /// 生成测试图片并验证扫描与缩略图
    #[test]
    fn find_and_thumb() {
        use image::{Rgb, RgbImage};

        // 构造临时相册目录
        let tmp = std::env::temp_dir().join("pm_thumb_test");
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
        assert!(res.thumb_path.ends_with(".jpg"));

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
}
