//! 相册照片影调分析模块（测试功能，独立组件）
//!
//! 职责：遍历相册目录内所有图片，**先下采样到 256px** 再统计灰度直方图，
//! 按平均亮度法给出影调类型（低调 / 中间调 / 高调）。
//!
//! 性能关键：直方图是统计分布，256px（约 6.5 万像素）已足够精确；
//! JPEG 走 jpeg-decoder 的 DCT 降采样（解码时只解所需 DCT 块，大图从
//! 秒级降到几十毫秒），其他格式解码后缩放到 256px，避免全尺寸解码卡顿。
//!
//! 解耦原则：
//! - 不依赖 `db` / `thumbnail` / `folder` 模块（图片扩展名列表本地定义）
//! - 不修改数据库 schema，扫描结果不落库
//! - `lib.rs` 仅保留薄命令壳 `scan_album_tones`

use std::path::Path;

use serde::Serialize;

/// 下采样目标边长（px）
const SAMPLE_SIZE: u32 = 256;

/// 影调类型（平均亮度法判断）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToneType {
    /// 平均亮度 L̄ < 85
    LowKey,
    /// 85 ≤ L̄ ≤ 170
    MidKey,
    /// L̄ > 170
    HighKey,
}

/// 单张照片的影调分析结果
#[derive(Debug, Clone, Serialize)]
pub struct PhotoTone {
    /// 文件名（不含路径）
    pub file_name: String,
    /// 完整路径（前端 tooltip 显示用）
    pub path: String,
    /// 灰度直方图，256 个 bin（索引 = 灰度值 0..255）
    pub histogram: Vec<u32>,
    /// 加权平均亮度 L̄（0..255）；解码失败/无像素时为 None
    pub avg_luma: Option<f64>,
    /// 影调类型；无法统计时为 None
    pub tone_type: Option<ToneType>,
}

/// 扫描相册目录内所有图片的影调信息（递归子目录，跳过隐藏文件/目录）
///
/// - 目录不存在或不是文件夹 → 返回错误
/// - 单张图片解码失败不影响整体，该图片 histogram 为空、tone_type 为 None
pub fn scan_album_tones(dir: &str) -> Result<Vec<PhotoTone>, String> {
    let paths = crate::vision::walk_image_paths(dir)?;
    Ok(analyze_paths(&paths))
}

/// 对**已收集好的路径列表**做影调分析（避免重复遍历目录）。
///
/// 遍历规则由 `vision::walk_image_paths` 统一负责，本函数只做「逐张分析 + 排序」，
/// 与 `scan_album_tones` 的结果完全一致（后者现在也就是调它）。
pub fn analyze_paths(paths: &[std::path::PathBuf]) -> Vec<PhotoTone> {
    let mut tones: Vec<PhotoTone> = paths
        .iter()
        .map(|p| {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            analyze_photo(p, &name)
        })
        .collect();
    tones.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    tones
}

/// 多线程版影调分析（仿 `photo_scan::read_exif_paths_parallel`）。
///
/// 为什么值得并行：影调是**纯 CPU + 独立 IO** 的任务（下采样解码 → 直方图），
/// 单张之间零共享状态、零顺序依赖，天然易并行。
///
/// 与 `analyze_paths` 的**结果完全等价**：
/// - 同样先按 `file_name` 排序（多线程分片会打乱顺序，必须重排才能对齐单线程口径）
/// - 分片方式同 `read_exif_paths_parallel`：`chunk = ceil(n / threads)`，
///   各线程处理互不重叠的连续区间，最后按分片顺序拼接
///
/// `threads` 由调用方按需给出；传 0 或 1 时退化为单线程，
/// 避免小相册（几十张）为起线程付出调度开销。
pub fn analyze_paths_parallel(paths: &[std::path::PathBuf], threads: usize) -> Vec<PhotoTone> {
    let n = paths.len();
    if n == 0 {
        return Vec::new();
    }
    let threads = threads.max(1).min(n);
    if threads == 1 {
        return analyze_paths(paths);
    }

    let chunk = n.div_ceil(threads);
    let mut tones: Vec<PhotoTone> = std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|part| scope.spawn(move || analyze_paths(part)))
            .collect();
        let mut all: Vec<PhotoTone> = Vec::with_capacity(n);
        for h in handles {
            match h.join() {
                Ok(mut v) => all.append(&mut v),
                // 单张分析内部已经把「解码失败」收敛为 None 字段、不 panic，
                // 线程 join 失败只可能是 OOM 等极端情况：跳过该分片而非整册失败。
                Err(_) => {}
            }
        }
        all
    });
    tones.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    tones
}

/// 分析单张图片：下采样解码 → 灰度直方图 → 平均亮度 → 影调类型
fn analyze_photo(path: &Path, name: &str) -> PhotoTone {
    let mut tone = PhotoTone {
        file_name: name.to_string(),
        path: path.to_string_lossy().into_owned(),
        histogram: Vec::new(),
        avg_luma: None,
        tone_type: None,
    };

    let rgb = match decode_sampled(path) {
        Some(img) => img,
        None => return tone,
    };

    // 统计灰度直方图（BT.601 加权亮度：0.299R + 0.587G + 0.114B，整数定点运算）
    let mut hist = [0u64; 256];
    for p in rgb.pixels() {
        let luma = (299 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32) / 1000;
        hist[luma as usize] += 1;
    }

    let total: u64 = hist.iter().sum();
    if total == 0 {
        return tone;
    }

    // 平均亮度法：L̄ = Σ(k·h(k)) / Σ(h(k))
    let weighted: u64 = hist
        .iter()
        .enumerate()
        .map(|(k, &c)| k as u64 * c)
        .sum();
    let avg_luma = weighted as f64 / total as f64;

    tone.histogram = hist.iter().map(|&c| c as u32).collect();
    tone.avg_luma = Some(avg_luma);
    tone.tone_type = Some(classify(avg_luma));
    tone
}

/// 平均亮度法判断影调类型：L̄ < 85 → 低调；L̄ > 170 → 高调；否则 → 中间调
fn classify(avg_luma: f64) -> ToneType {
    if avg_luma < 85.0 {
        ToneType::LowKey
    } else if avg_luma > 170.0 {
        ToneType::HighKey
    } else {
        ToneType::MidKey
    }
}

/// 解码并下采样到 SAMPLE_SIZE
///
/// - JPEG：jpeg-decoder 的 DCT 降采样，解码时只解所需 DCT 块（快 16~64 倍）
/// - 其他格式：image 解码后缩放到 SAMPLE_SIZE
fn decode_sampled(path: &Path) -> Option<image::RgbImage> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let is_jpeg = name.ends_with(".jpg") || name.ends_with(".jpeg");
    if is_jpeg {
        let file = std::fs::File::open(path).ok()?;
        let mut decoder = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
        let _ = decoder.scale(SAMPLE_SIZE as u16, SAMPLE_SIZE as u16);
        let pixels = decoder.decode().ok()?;
        let info = decoder.info()?;
        return image::RgbImage::from_raw(info.width as u32, info.height as u32, pixels);
    }
    let img = image::open(path).ok()?.thumbnail(SAMPLE_SIZE, SAMPLE_SIZE);
    Some(img.to_rgb8())
}

pub mod commands {

// =====================================================================
// 以下命令自 lib.rs 迁入（lib.rs 瘦身）：色调扫描命令层（薄包装）
// =====================================================================

/// 扫描相册目录内所有图片的影调分析（灰度直方图 + 影调类型，测试功能，不落库）
/// 影调提取逻辑全部在独立模块 `tone` 中，此处仅保留薄命令壳（功能解耦）。
#[tauri::command]
pub fn scan_album_tones(path: String) -> Result<Vec<crate::tone::PhotoTone>, String> {
    let _t = log_call!("scan_album_tones", &format!("path={path}"));
    let r = crate::tone::scan_album_tones(&path);
    match &r {
        Ok(list) => crate::logger::log_call_end_with(
            "scan_album_tones",
            _t,
            &format!("OK | photos={}", list.len()),
        ),
        Err(e) => crate::logger::log_call_end_with("scan_album_tones", _t, &format!("ERR | {e}")),
    }
    r
}

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_thresholds() {
        assert_eq!(classify(10.0), ToneType::LowKey);
        assert_eq!(classify(84.9), ToneType::LowKey);
        assert_eq!(classify(85.0), ToneType::MidKey); // 边界：85 归中间调
        assert_eq!(classify(100.0), ToneType::MidKey);
        assert_eq!(classify(170.0), ToneType::MidKey); // 边界：170 归中间调
        assert_eq!(classify(170.1), ToneType::HighKey);
        assert_eq!(classify(240.0), ToneType::HighKey);
    }

    #[test]
    fn test_scan_fixture_tone_dir() {
        // 测试图片目录：项目根 test_fixture_photos（不存在则跳过）
        let dir = format!("{}/../test_fixture_photos", env!("CARGO_MANIFEST_DIR"));
        if !std::path::Path::new(&dir).is_dir() {
            eprintln!("跳过：无测试图片目录 {dir}");
            return;
        }
        let tones = scan_album_tones(&dir).expect("扫描应成功");
        // 深色照片 RGB(120,140,160) → L̄≈136 → 中间调
        let camera = tones.iter().find(|t| t.file_name == "IMG_0001_camera.jpg").unwrap();
        assert_eq!(camera.histogram.len(), 256);
        assert!(camera.histogram.iter().sum::<u32>() > 0);
        assert_eq!(camera.tone_type, Some(ToneType::MidKey));

        // 近黑照片 → 低调
        let dark = tones.iter().find(|t| t.file_name == "IMG_0005_dark.jpg").unwrap();
        assert_eq!(dark.tone_type, Some(ToneType::LowKey));
        assert!(dark.avg_luma.unwrap() < 30.0);

        // 近白照片 → 高调
        let bright = tones.iter().find(|t| t.file_name == "IMG_0006_bright.jpg").unwrap();
        assert_eq!(bright.tone_type, Some(ToneType::HighKey));
        assert!(bright.avg_luma.unwrap() > 210.0);

        // PNG（无 EXIF 不影响影调统计）
        let png = tones.iter().find(|t| t.file_name == "IMG_0003_png.png").unwrap();
        assert!(png.histogram.iter().sum::<u32>() > 0);
        assert!(png.tone_type.is_some());

        // 子目录递归 + 隐藏目录跳过
        let wide = tones.iter().find(|t| t.file_name == "IMG_0004_wide.jpg").unwrap();
        assert!(wide.histogram.iter().sum::<u32>() > 0);
        assert!(!tones.iter().any(|t| t.file_name.contains("hidden")));
    }
}
