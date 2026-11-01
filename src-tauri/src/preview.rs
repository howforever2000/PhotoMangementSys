//! 大图预览缓存（FEAT-142）
//!
//! ## 为什么需要
//!
//! 前端大图是直接把**原文件路径**交给 WebView2（`convertFileSrc`）渲染的。
//! WebView2 是 Chromium，**不认识 HEIC/HEIF** —— 相册里有 iPhone 照片时，
//! 网格（走缩略图）正常，但点开大图就是一片空白。
//!
//! 所以这里按需把原图渲染成一张**缓存 JPEG**（默认长边 2560，质量 88），
//! 前端只对 heic/heif 走这条路径，其余格式仍直连原文件（零额外开销）。
//!
//! ## 缓存键
//!
//! `previews/<thumb_photo_hash 前 2 位>/<hash>_<max_edge>.jpg` ——
//! 与缩略图缓存同源的「路径 + 大小 + mtime」指纹，原图变了自动换新文件；
//! 粒度里带 `max_edge`，将来放大预览尺寸不会与旧缓存混用。
//!
//! 落盘同样是**临时文件 + rename 原子替换**（同 `thumbnail::save_thumbnail`），
//! 避免并发预览同一张时留下半截文件。

use std::path::{Path, PathBuf};

use tauri::Manager;

/// 预览缓存目录名（位于 app_data_dir 下）
const PREVIEW_DIR: &str = "previews";

/// 默认预览长边：大于常见 4K 屏可视区，又不至于让 WebView2 解码吃力
pub const DEFAULT_MAX_EDGE: u32 = 2560;

/// JPEG 质量（88 在「看不出块效应」与「体积可控」之间）
const PREVIEW_QUALITY: u8 = 88;

fn preview_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {e}"))?;
    Ok(data.join(PREVIEW_DIR))
}

/// 该原图是否必须走预览（WebView2 不认识的格式）
pub fn needs_preview(path: &Path) -> bool {
    crate::heic::is_heif_path(path)
}

/// 确保预览存在，返回可交给前端的本地路径
pub fn ensure(path: &Path, max_edge: u32, app: &tauri::AppHandle) -> Result<String, String> {
    ensure_into(&preview_dir(app)?, path, max_edge)
}

/// 在指定缓存目录下确保预览存在（与 Tauri 解耦，便于单测与复用）
pub fn ensure_into(dir: &Path, path: &Path, max_edge: u32) -> Result<String, String> {
    let max_edge = max_edge.clamp(512, 8192);
    if !path.is_file() {
        return Err(format!("照片不存在: {}", path.display()));
    }
    let md = std::fs::metadata(path).map_err(|e| format!("读取照片信息失败: {e}"))?;
    let mtime_ns = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let key = crate::thumbnail::thumb_photo_hash(path, md.len(), mtime_ns);
    let sub = dir.join(&key[..2.min(key.len())]);
    let dst = sub.join(format!("{key}_{max_edge}.jpg"));
    if dst.is_file() {
        return Ok(dst.to_string_lossy().into_owned());
    }

    // 解码（HEIC 由 imgio 分流给系统解码器）
    let img = crate::imgio::open_any(path)?;
    // HEIC 的旋转在容器属性（irot/imir）里，系统解码器已烘焙；
    // 其余格式才需要用 EXIF Orientation 转正（WebP/JPEG 预览不带 EXIF）。
    let img = if crate::heic::is_heif_path(path) {
        img
    } else {
        crate::exif_orient::apply_orientation(img, crate::exif_orient::read_orientation(path))
    };
    let thumb = img.thumbnail(max_edge, max_edge);

    std::fs::create_dir_all(&sub).map_err(|e| format!("创建预览目录失败: {e}"))?;
    let tmp = sub.join(format!("{key}_{max_edge}.tmp{}.jpg", std::process::id()));
    let mut out = std::fs::File::create(&tmp).map_err(|e| format!("写入预览失败: {e}"))?;
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, PREVIEW_QUALITY);
    enc.encode_image(&thumb.to_rgb8())
        .map_err(|e| format!("编码预览失败: {e}"))?;
    drop(out);
    // Windows 的 rename 不能覆盖：目标理论上不存在，失败时清一次再试
    if std::fs::rename(&tmp, &dst).is_err() {
        let _ = std::fs::remove_file(&dst);
        std::fs::rename(&tmp, &dst).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("预览落盘失败: {e}")
        })?;
    }
    crate::logger::log_info(&format!(
        "[preview] 生成 {} → {}（长边 {}）",
        path.display(),
        dst.display(),
        max_edge
    ));
    Ok(dst.to_string_lossy().into_owned())
}

pub mod commands {
    use super::*;

    /// 确保某张照片有可被 WebView 渲染的预览图，返回预览文件路径
    ///
    /// 只对 HEIC/HEIF 有意义（其余格式前端直连原文件）；对非 HEIC 调用会
    /// 直接回原路径，让前端不必分叉。
    #[tauri::command]
    pub fn ensure_photo_preview(
        path: String,
        max_edge: Option<u32>,
        app: tauri::AppHandle,
    ) -> Result<String, String> {
        let p = Path::new(&path);
        if !needs_preview(p) {
            return Ok(path);
        }
        let _t = log_call!("ensure_photo_preview", &path);
        let out = ensure(p, max_edge.unwrap_or(DEFAULT_MAX_EDGE), &app)?;
        crate::logger::log_call_end_with("ensure_photo_preview", _t, "OK");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_heic_needs_preview() {
        assert!(needs_preview(Path::new(r"D:\pics\IMG_0001.HEIC")));
        assert!(needs_preview(Path::new(r"D:\pics\a.heif")));
        assert!(!needs_preview(Path::new(r"D:\pics\a.jpg")));
        assert!(!needs_preview(Path::new(r"D:\pics\a.png")));
    }

    /// 真样本验证（需系统 HEIF 解码器）：
    /// `PMS_HEIC_SAMPLE=... cargo test --lib -- --ignored preview`
    /// 验的是「HEIC → 缓存 JPEG」整条链路，含二次命中缓存不重复解码
    #[test]
    #[ignore = "需要真实 HEIC 样本：设置 PMS_HEIC_SAMPLE"]
    fn renders_heic_preview_and_reuses_cache() {
        let Some(sample) = std::env::var_os("PMS_HEIC_SAMPLE") else {
            eprintln!("未设置 PMS_HEIC_SAMPLE，跳过");
            return;
        };
        let src = std::path::PathBuf::from(sample);
        let out_dir = std::env::temp_dir().join(format!("pms-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out_dir);

        let first = ensure_into(&out_dir, &src, 1024).expect("首先生成预览应成功");
        let p1 = std::path::PathBuf::from(&first);
        assert!(p1.is_file(), "预览文件应已落盘");
        let dims = image::image_dimensions(&p1).expect("预览应可被解码");
        assert!(dims.0 <= 1024 && dims.1 <= 1024, "长边应被限制到 1024: {dims:?}");
        let m1 = std::fs::metadata(&p1).unwrap().modified().unwrap();

        // 二次调用：命中缓存，不再重解码（文件 mtime 不变）
        let second = ensure_into(&out_dir, &src, 1024).expect("二次调用应命中缓存");
        assert_eq!(first, second);
        let m2 = std::fs::metadata(&second).unwrap().modified().unwrap();
        assert_eq!(m1, m2, "缓存命中时不应重新写文件");

        let _ = std::fs::remove_dir_all(&out_dir);
    }
}
