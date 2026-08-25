//! 单张照片信息读取模块（按需、不落库）
//!
//! 职责：读取单张照片的基础信息（分辨率 / 文件大小 / 格式）与 RGB 三通道
//! 像素分布直方图，供大图查看器的「详细信息」面板展示。
//!
//! 性能关键：
//! - 分辨率走 `image::ImageReader::into_dimensions()`（只读文件头，毫秒级，
//!   不做全尺寸解码——参见缺陷 BUG-2026-0815-003 的教训）
//! - 直方图复用 tone.rs 的下采样思路：先采样到 256px 再统计，
//!   JPEG 走 jpeg-decoder 的 DCT 降采样（只解所需 DCT 块）
//!
//! 解耦原则：
//! - 不依赖 `db` / `thumbnail` 模块
//! - 结果不落库：点击时实时读取单张（低频操作，无性能压力）

use std::path::Path;

use serde::Serialize;

/// 直方图统计的下采样目标边长（px）。256px ≈ 6.5 万像素，对分布统计足够精确。
const SAMPLE_SIZE: u32 = 256;

/// 单张照片信息 —— 对应前端 `PhotoInfo`
#[derive(Debug, Clone, Serialize)]
pub struct PhotoInfo {
    /// 完整路径
    pub path: String,
    /// 文件名（不含目录）
    pub file_name: String,
    /// 格式（小写扩展名，如 jpg / png）
    pub format: String,
    /// 原始宽度（px）
    pub width: u32,
    /// 原始高度（px）
    pub height: u32,
    /// 文件大小（字节）
    pub file_size: u64,
    /// R 通道直方图，256 个 bin；解码失败时为空数组
    pub hist_r: Vec<u32>,
    /// G 通道直方图，256 个 bin
    pub hist_g: Vec<u32>,
    /// B 通道直方图，256 个 bin
    pub hist_b: Vec<u32>,
}

/// 读取单张照片信息：分辨率 + 文件大小 + RGB 像素分布直方图
///
/// - 文件不存在 → Err
/// - 尺寸解析失败 → Err（基本信息都拿不到没有展示意义）
/// - 直方图统计失败不影响基本信息（直方图为空数组，前端隐藏分布图）
pub fn read_photo_info(path: &str) -> Result<PhotoInfo, String> {
    let p = Path::new(path);
    if !p.is_file() {
        return Err(format!("文件不存在: {path}"));
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("读取文件元数据失败: {e}"))?;
    let file_name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let format = p
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // 只读头的分辨率探测（支持 jpg/png/webp/gif/bmp/tiff 等 image 已启用格式）
    let (width, height) = image::ImageReader::open(p)
        .map_err(|e| format!("无法打开图片: {e}"))?
        .into_dimensions()
        .map_err(|e| format!("无法解析图片尺寸: {e}"))?;

    let (hist_r, hist_g, hist_b) = match sample_rgb(p) {
        Some(img) => count_histograms(&img),
        None => (Vec::new(), Vec::new(), Vec::new()),
    };

    Ok(PhotoInfo {
        path: path.to_string(),
        file_name,
        format,
        width,
        height,
        file_size: meta.len(),
        hist_r,
        hist_g,
        hist_b,
    })
}

/// 下采样解码到 SAMPLE_SIZE 后返回 RGB 图；失败返回 None
fn sample_rgb(path: &Path) -> Option<image::RgbImage> {
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

/// 统计 RGB 三通道直方图（256 bin）
fn count_histograms(img: &image::RgbImage) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let mut hr = [0u32; 256];
    let mut hg = [0u32; 256];
    let mut hb = [0u32; 256];
    for p in img.pixels() {
        hr[p[0] as usize] += 1;
        hg[p[1] as usize] += 1;
        hb[p[2] as usize] += 1;
    }
    (hr.to_vec(), hg.to_vec(), hb.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_photo_info_fixture() {
        // 测试图片目录：项目根 test_fixture_photos（不存在则跳过）
        let dir = format!("{}/../test_fixture_photos", env!("CARGO_MANIFEST_DIR"));
        if !std::path::Path::new(&dir).is_dir() {
            eprintln!("跳过：无测试图片目录 {dir}");
            return;
        }
        let path = format!("{dir}/IMG_0001_camera.jpg");
        let info = read_photo_info(&path).expect("读取应成功");
        assert_eq!(info.file_name, "IMG_0001_camera.jpg");
        assert_eq!(info.format, "jpg");
        assert!(info.width > 0 && info.height > 0);
        assert!(info.file_size > 0);
        // 直方图应有统计值且各通道 bin 数为 256
        assert_eq!(info.hist_r.len(), 256);
        assert_eq!(info.hist_g.len(), 256);
        assert_eq!(info.hist_b.len(), 256);
        assert!(info.hist_r.iter().sum::<u32>() > 0);

        // 不存在的文件应报错
        assert!(read_photo_info("/nonexistent/nope.jpg").is_err());
    }
}
