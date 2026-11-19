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
    // 下限 64 而不是 512：工坊胶片条/网格要的是 384px 缩略图，把请求值悄悄抬到
    // 512 会让「缓存文件名里的长边」与调用方以为的不一致，且多解一倍像素。
    let max_edge = max_edge.clamp(64, 8192);
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

    // 全局生成名额：与缩略图共用同一道闸（同一种「解码 + 缩放 + 编码」资源）。
    // 放在解码之前，保证排队的请求不占 CPU 与内存。
    let _permit = crate::thumbnail::acquire_gen_permit();

    let thumb = decode_limited(path, max_edge)?;

    std::fs::create_dir_all(&sub).map_err(|e| format!("创建预览目录失败: {e}"))?;
    let tmp = sub.join(format!("{key}_{max_edge}.tmp{}.jpg", std::process::id()));
    let mut out = std::fs::File::create(&tmp).map_err(|e| format!("写入预览失败: {e}"))?;
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, PREVIEW_QUALITY);
    enc.encode_image(&thumb)
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

/// 解码并缩到长边 `max_edge`，按源格式选最快的路径。
///
/// - **JPEG**：先按魔数嗅探（不信扩展名，项目里存在「.jpg 实为 PNG」的图库），
///   是 JPEG 就走 `jpeg_decoder` 的 DCT 域降采样（只解所需 DCT 块），
///   实测 24MP 全解码 146ms vs DCT 降采样 ~40ms；再按 EXIF 转正后收尾缩放。
/// - 其余格式：走 `imgio::open_any`（含 HEIC 系统解码）。
///
/// 顺序要点与 `thumbnail::save_thumbnail` 一致：**先转正、再缩放**，
/// 否则转置类方向（6/8）会把躺倒的那条长边当成缩放基准，构图与正置不同。
fn decode_limited(path: &Path, max_edge: u32) -> Result<image::RgbImage, String> {
    let orientation = crate::exif_orient::read_orientation(path);
    let oriented = |img: image::DynamicImage| -> image::DynamicImage {
        crate::exif_orient::apply_orientation(img, orientation)
    };

    if is_jpeg_by_magic(path) {
        if let Ok(small) = crate::thumbnail::decode_jpeg_scaled_pub(path, max_edge) {
            // 降采样后的图长边已接近 max_edge，这里只做精确收尾（转正放在缩放前）
            let img = oriented(image::DynamicImage::ImageRgb8(small));
            if img.width().max(img.height()) <= max_edge {
                return Ok(img.to_rgb8());
            }
            return Ok(img.thumbnail(max_edge, max_edge).to_rgb8());
        }
        // DCT 降采样失败（异常 JPEG）→ 回落全解码，行为与旧实现一致
    }

    // HEIC 的旋转在容器属性（irot/imir）里，系统解码器已烘焙；
    // 其余格式才需要用 EXIF Orientation 转正（WebP/JPEG 预览不带 EXIF）。
    let img = crate::imgio::open_any(path)?;
    let img = if crate::heic::is_heif_path(path) {
        img
    } else {
        oriented(img)
    };
    Ok(img.thumbnail(max_edge, max_edge).to_rgb8())
}

/// 按**魔数**判断是否 JPEG（不信扩展名）。
///
/// 只读文件头 4 字节：项目里有一批「`.jpg` 实际是 PNG」的图（微信/截图导出），
/// 反向也存在。扩展名判断会让这类图白白走全尺寸解码或直接解码失败。
fn is_jpeg_by_magic(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 3];
    f.read_exact(&mut head).is_ok() && head == [0xFF, 0xD8, 0xFF]
}

pub mod commands {
    use super::*;

    /// 工作台胶片条 / 网格用的缩略图长边（与 `.fs-thumb` 96×64、网格 148px 匹配，
    /// 取 2× 覆盖高 DPI 屏；再大就失去「便宜」的意义）。
    const WORKBENCH_THUMB_EDGE: u32 = 384;

    /// 确保某张照片有可被 WebView 渲染的预览图，返回预览文件路径
    ///
    /// 只对 HEIC/HEIF 有意义（其余格式前端直连原文件）；对非 HEIC 调用会
    /// 直接回原路径，让前端不必分叉。
    ///
    /// **必须是 async + spawn_blocking**：这是同步命令时它会跑在 Tauri 主线程
    /// （`devtools.rs` 有同款纪律注释），而它内部是「整文件读 + 全量解码 + 缩放 +
    /// JPEG 编码」。导入 200 张 HEIC 时前端会一次性并发 invoke，同步版会把
    /// 所有解码排到主线程，UI 与 IPC 一起冻死。
    #[tauri::command]
    pub async fn ensure_photo_preview(
        path: String,
        max_edge: Option<u32>,
        app: tauri::AppHandle,
    ) -> Result<String, String> {
        if !needs_preview(Path::new(&path)) {
            return Ok(path);
        }
        let edge = max_edge.unwrap_or(DEFAULT_MAX_EDGE);
        let t = log_call!("ensure_photo_preview", &path);
        let res = tauri::async_runtime::spawn_blocking(move || {
            let p = Path::new(&path);
            ensure(p, edge, &app)
        })
        .await
        .map_err(|e| format!("预览任务异常: {e}"))?;
        match &res {
            Ok(_) => crate::logger::log_call_end_with("ensure_photo_preview", t, "OK"),
            Err(e) => crate::logger::log_call_end_with("ensure_photo_preview", t, &format!("ERR {e}")),
        }
        res
    }

    /// 工作台缩略图：**任意格式**都返回一张 ≤384px 的缓存 JPEG 路径。
    ///
    /// 与 `ensure_photo_preview` 的区别有两点，都是为了工作台：
    ///   1. 不限于 HEIC —— 胶片条/网格过去直接 `convertFileSrc(原图)`，一屏
    ///      10~18 个格子就是 10~18 次 24MP 解码；换成 384px 后解码量降两个数量级。
    ///   2. 条目按 `(路径, 大小, mtime, 长边)` 缓存，重复打开零开销。
    ///
    /// 对比框的「原图」层**不要**用这个（那是要看清细节的），它继续用原图/大预览。
    #[tauri::command]
    pub async fn workbench_thumb(
        path: String,
        max_edge: Option<u32>,
        app: tauri::AppHandle,
    ) -> Result<String, String> {
        let edge = max_edge.unwrap_or(WORKBENCH_THUMB_EDGE).clamp(64, 1024);
        let t = log_call!("workbench_thumb", &path);
        let res = tauri::async_runtime::spawn_blocking(move || {
            ensure(Path::new(&path), edge, &app)
        })
        .await
        .map_err(|e| format!("缩略图任务异常: {e}"))?;
        match &res {
            Ok(_) => crate::logger::log_call_end_with("workbench_thumb", t, "OK"),
            Err(e) => crate::logger::log_call_end_with("workbench_thumb", t, &format!("ERR {e}")),
        }
        res
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

    /// 工坊缩略图：任意格式（这里是 JPEG）都能落到 ≤edge，且二次命中缓存。
    ///
    /// 这条守的是「胶片条/网格不再直接加载原图」那个改动的后端契约：
    /// 旧调用方拿的是原图路径，新调用方必须能拿到一张真实存在的小图。
    #[test]
    fn workbench_thumb_renders_any_format_and_caches() {
        let src_dir = std::env::temp_dir().join(format!("pms-wbthumb-src-{}", std::process::id()));
        let out_dir = std::env::temp_dir().join(format!("pms-wbthumb-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src_dir);
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&src_dir).unwrap();

        // 大图（2000×1500）以 JPEG 落盘，走 DCT 降采样路径
        let src = src_dir.join("big.jpg");
        let img = image::RgbImage::from_fn(2000, 1500, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        image::DynamicImage::ImageRgb8(img)
            .save(&src)
            .expect("测试图应能落盘");

        let first = ensure_into(&out_dir, &src, 384).expect("应生成缩略图");
        let p1 = std::path::PathBuf::from(&first);
        assert!(p1.is_file(), "缩略图文件应存在");
        let dims = image::image_dimensions(&p1).expect("缩略图应可解码");
        assert!(
            dims.0.max(dims.1) <= 384,
            "长边应 ≤384，实际 {dims:?}"
        );

        let m1 = std::fs::metadata(&p1).unwrap().modified().unwrap();
        let second = ensure_into(&out_dir, &src, 384).expect("二次应命中缓存");
        assert_eq!(first, second);
        let m2 = std::fs::metadata(&second).unwrap().modified().unwrap();
        assert_eq!(m1, m2, "缓存命中时不应重写文件");

        let _ = std::fs::remove_dir_all(&src_dir);
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    /// 不同 max_edge 必须落在**不同**缓存文件上（否则 384 的工坊缩略图会顶掉
    /// 2560 的大图预览，对比框里看到糊图）。
    #[test]
    fn different_max_edge_uses_different_cache_file() {
        let src_dir = std::env::temp_dir().join(format!("pms-wbedge-src-{}", std::process::id()));
        let out_dir = std::env::temp_dir().join(format!("pms-wbedge-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src_dir);
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&src_dir).unwrap();

        let src = src_dir.join("p.jpg");
        let img = image::RgbImage::from_fn(800, 600, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        image::DynamicImage::ImageRgb8(img).save(&src).unwrap();

        let small = ensure_into(&out_dir, &src, 384).unwrap();
        let large = ensure_into(&out_dir, &src, 512).unwrap();
        assert_ne!(small, large, "不同长边不得共用同一个缓存文件");

        let _ = std::fs::remove_dir_all(&src_dir);
        let _ = std::fs::remove_dir_all(&out_dir);
    }
}
