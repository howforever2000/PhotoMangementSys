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
    NoImage,
    Io(std::io::Error),
    Image(image::ImageError),
}

impl std::fmt::Display for ThumbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThumbError::NoImage => write!(f, "文件夹中没有支持的图片"),
            ThumbError::Io(e) => write!(f, "IO 错误: {e}"),
            ThumbError::Image(e) => write!(f, "图片处理错误: {e}"),
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

/// 判断文件名是否是支持的图片
fn is_image_file(file_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    IMAGE_EXTS.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// 扫描目录，返回第一张图片的完整路径
///
/// 使用 `walkdir` 递归遍历（含子目录），按文件系统顺序取第一个匹配的图片。
/// 对应"默认以第一张图片为封面图"的需求。
pub fn find_first_image(dir: &Path) -> Result<PathBuf, ThumbError> {
    if !dir.is_dir() {
        return Err(ThumbError::NoImage);
    }
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // 跳过隐藏目录和缩略图缓存目录，避免无限递归或扫到缓存
            !e.file_name().to_string_lossy().starts_with('.')
        })
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_image_file(&name) {
                return Ok(entry.into_path());
            }
        }
    }
    Err(ThumbError::NoImage)
}

/// 统计文件夹中图片的总数量
///
/// 递归遍历目录（含子目录），统计所有支持的图片文件数。
pub fn count_images(dir: &Path) -> usize {
    if !dir.is_dir() {
        return 0;
    }
    let mut count = 0usize;
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        if let Ok(e) = entry {
            if e.file_type().is_file() {
                let name = e.file_name().to_string_lossy().to_string();
                if is_image_file(&name) {
                    count += 1;
                }
            }
        }
    }
    count
}

/// 统计文件夹总大小（字节）—— 真实占用空间
///
/// 递归遍历目录（含子目录），累加所有文件大小。
/// Windows 目录自身的元数据不包含内容大小，因此必须遍历才能获得真实占用空间。
pub fn folder_size(dir: &Path) -> u64 {
    if !dir.is_dir() {
        return 0;
    }
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        if let Ok(e) = entry {
            if e.file_type().is_file() {
                if let Ok(meta) = e.metadata() {
                    total += meta.len();
                }
            }
        }
    }
    total
}

/// 读取图片的拍摄时间（EXIF DateTimeOriginal），返回 "YYYY-MM-DD"
///
/// - 无 EXIF 或读取失败时返回 None
/// - 提取年月日，精确到日
pub fn read_shoot_time(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bufreader = std::io::BufReader::new(&file);
    let exif_reader = exif::Reader::new();
    let exif = exif_reader.read_from_container(&mut bufreader).ok()?;

    let field = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)?;
    // DateTimeOriginal 存储为 ASCII，格式 "YYYY:MM:DD HH:MM:SS"
    let raw = field.display_value().to_string();
    // 提取前 10 个字符，把冒号换成横线 -> "YYYY-MM-DD"
    let digits: Vec<char> = raw.chars().take(10).collect();
    if digits.len() < 10 {
        return None;
    }
    Some(format!(
        "{}{}{}{}-{}{}-{}{}",
        digits[0], digits[1], digits[2], digits[3],
        digits[5], digits[6], digits[8], digits[9]
    ))
}

/// 缩略图结果信息
#[derive(Debug, Serialize, Deserialize)]
pub struct ThumbResult {
    /// 缩略图绝对路径（位于缓存目录）
    pub thumb_path: String,
    /// 原图路径（第一张图片）
    pub source_path: String,
}

/// 为相册生成缩略图（若无缓存则生成），返回缓存路径
///
/// - 扫描 `album_path` 找第一张图片
/// - 生成 256px 缩略图写入 `thumbs_dir/<相册id>_<hash>.jpg`
/// - 缓存命中则直接返回，避免重复生成
pub fn ensure_thumbnail(
    album_id: i64,
    album_path: &Path,
    thumbs_dir: &Path,
) -> Result<ThumbResult, ThumbError> {
    let source = find_first_image(album_path)?;

    // 缓存文件命名：album_<id>_<原文件名去掉扩展名>.jpg
    let file_stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "cover".to_string());
    // 文件名可能含非法字符，做清理
    let safe_stem: String = file_stem
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(40)
        .collect();
    let cached_name = format!("album_{album_id}_{}.jpg", if safe_stem.is_empty() { "cover".to_string() } else { safe_stem });
    let thumb_path = thumbs_dir.join(&cached_name);

    // 若缓存已存在则直接复用
    if thumb_path.exists() {
        return Ok(ThumbResult {
            thumb_path: thumb_path.to_string_lossy().into_owned(),
            source_path: source.to_string_lossy().into_owned(),
        });
    }

    // 生成缩略图
    let img = image::open(&source)?;
    let thumb = img.thumbnail(THUMB_SIZE, THUMB_SIZE);

    // 确保缓存目录存在
    if let Some(parent) = thumb_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    thumb.save_with_format(&thumb_path, ImageFormat::Jpeg)?;

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
    let cached_name = format!("album_{album_id}_manual_cover.jpg");
    let thumb_path = thumbs_dir.join(&cached_name);

    if thumb_path.exists() {
        return Ok(thumb_path.to_string_lossy().into_owned());
    }

    let img = image::open(source)?;
    let thumb = img.thumbnail(THUMB_SIZE, THUMB_SIZE);

    if let Some(parent) = thumb_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    thumb.save_with_format(&thumb_path, ImageFormat::Jpeg)?;

    Ok(thumb_path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // 第一张图应能被找到
        let first = find_first_image(&img_dir).unwrap();
        assert_eq!(first.file_name().unwrap(), "first.png");

        // 生成缩略图
        let thumbs = tmp.join("thumbs");
        let res = ensure_thumbnail(1, &img_dir, &thumbs).unwrap();
        assert!(Path::new(&res.thumb_path).exists());
        assert!(res.thumb_path.ends_with(".jpg"));

        // 二次调用应命中缓存
        let res2 = ensure_thumbnail(1, &img_dir, &thumbs).unwrap();
        assert_eq!(res2.thumb_path, res.thumb_path);
    }
}
