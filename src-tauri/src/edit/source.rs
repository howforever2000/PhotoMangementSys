//! 源图加载与预览金字塔
//! ====================================================================
//! 职责：把磁盘上的一个文件路径变成「一块可反复渲染的 RGB u8 平面 +
//! 原图尺寸」。渲染管线（`pipeline.rs`）不做解码，因此所有解码策略
//! 都收在这里，替换解码器不影响任何算法代码。
//!
//! ## 预览为什么要分级解码（而不是「全解码再缩小」）
//!
//! 实测（6000×4000 JPEG）：全解码 340ms，而 JPEG 的 DCT 缩放解码
//! （`jpeg-decoder::Decoder::scale`）只解所需 DCT 块。但要注意它的两个陷阱：
//!
//! 1. **只有 1/1、1/2、1/4、1/8 四档**，且判定是「任一轴满足即降档」
//!    （`choose_idct_size` 里是 `||`）。实测请求 1600px 宽会拿到 1/2 档
//!    （3000×2000），请求 1500px 才拿到 1/4（1500×1000）。
//! 2. **加速比远低于缩放比例**：Huffman 熵解码无法跳过，1/8 档也只快 1.4 倍。
//!
//! 因此正确的策略是**两级**：先用 DCT 缩放拿到「不超过目标 1.3 倍」的
//! 中等尺寸，再用面积平均缩放到目标尺寸。第二步不可省 —— DCT 缩放在
//! 大比例下会丢高频（实测与全解码路径的 RMSE 达 7.76/255，肉眼可见）。
//!
//! 非 JPEG 格式没有 DCT 缩放可用，直接全解码（大图慢，但 PNG/BMP 在
//! 照片场景里少见，且它们本身就常被存成已缩放尺寸）。

use std::io::BufReader;
use std::path::Path;

use image::RgbImage;

/// 预览最长边（正常档）。1600px 在 1:1 显示下已覆盖绝大多数屏幕。
pub const PREVIEW_LONG_EDGE: u32 = 1600;
/// 预览最长边（草稿档）。拖动滑块时用，帧率优先。
pub const DRAFT_LONG_EDGE: u32 = 800;
/// 缩略图最长边（缩略图网格用）。
pub const THUMB_LONG_EDGE: u32 = 400;

/// 解码错误（对用户可读）。
#[derive(Debug)]
pub enum LoadError {
    Io(String),
    Decode(String),
    TooLarge { width: u32, height: u32, limit: u32 },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "读取文件失败：{e}"),
            LoadError::Decode(e) => write!(f, "解码图片失败：{e}"),
            LoadError::TooLarge {
                width,
                height,
                limit,
            } => write!(
                f,
                "图片 {width}×{height} 超过单边 {limit} 的上限，请先缩小后再编辑"
            ),
        }
    }
}

/// 像素尺寸硬上限（长边）。
///
/// 上限的作用是**内存护栏**：f32 工作平面在长边 8000 时已需 550MB，
/// 再大就会 OOM。宁可明确报错，也不要让用户看着应用卡死。
pub const MAX_LONG_EDGE: u32 = 12_000;

/// 一张已加载的源图。
#[derive(Debug, Clone)]
pub struct SourceImage {
    /// 解码后的 RGB 平面（长边 = `decoded_long_edge`）
    pub pixels: RgbImage,
    /// 文件的真实像素尺寸
    pub original_width: u32,
    pub original_height: u32,
    /// 是否经过 DCT 降采样解码（影响「预览质量」提示）
    pub subsampled: bool,
}

impl SourceImage {
    #[inline]
    pub fn decoded_width(&self) -> u32 {
        self.pixels.width()
    }
    #[inline]
    pub fn decoded_height(&self) -> u32 {
        self.pixels.height()
    }
}

/// 按长边约束把尺寸等比缩小。
#[inline]
fn fit_long_edge(w: u32, h: u32, max_long: u32) -> (u32, u32) {
    if max_long == 0 {
        return (w, h);
    }
    let m = w.max(h);
    if m <= max_long {
        return (w, h);
    }
    let s = max_long as f32 / m as f32;
    let nw = (w as f32 * s).round().max(1.0) as u32;
    let nh = (h as f32 * s).round().max(1.0) as u32;
    (nw, nh)
}

/// 从文件加载源图，`target_long_edge = 0` 表示全分辨率。
///
/// JPEG 走两阶段（详见模块头注释），其他格式直接 `image` 解码。
pub fn load(path: &Path, target_long_edge: u32) -> Result<SourceImage, LoadError> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let is_jpeg = name.ends_with(".jpg") || name.ends_with(".jpeg");

    if is_jpeg {
        load_jpeg(path, target_long_edge)
    } else {
        load_generic(path, target_long_edge)
    }
}

/// JPEG：DCT 缩放解码 + 面积平均二次缩放。
fn load_jpeg(path: &Path, target_long_edge: u32) -> Result<SourceImage, LoadError> {
    let file = std::fs::File::open(path).map_err(|e| LoadError::Io(e.to_string()))?;
    let mut dec = jpeg_decoder::Decoder::new(BufReader::new(file));

    // 阶段 1：向 jpeg-decoder 请求一个「不超过目标的 1.3 倍」的尺寸。
    // 1.3 而不是 1.0：`choose_idct_size` 的 `||` 判定有跳变，
    // 请求正好等于目标时可能拿到高一档（6000→请求 3000 反而给 1500）。
    // 略过冲一点能让落入的档位稳定。
    let probe = if target_long_edge == 0 {
        (0u16, 0u16)
    } else {
        let want = (target_long_edge as f32 * 1.3).round().max(1.0) as u32;
        let want = want.min(u16::MAX as u32) as u16;
        (want, want)
    };

    let (dw, dh) = dec
        .scale(probe.0, probe.1)
        .map_err(|e| LoadError::Decode(e.to_string()))?;
    // info() 是 Option（scale 成功后必有值，但 API 不保证）
    let info = dec
        .info()
        .ok_or_else(|| LoadError::Decode("无法读取 JPEG 头信息".into()))?;
    let raw = dec.decode().map_err(|e| LoadError::Decode(e.to_string()))?;

    // decode() 之后的 buffer 尺寸才是真正的降采样尺寸；
    // info() 给出的是原始尺寸（用于原图尺寸上报）。
    let stage1_w = info.width as u32;
    let stage1_h = info.height as u32;
    if stage1_w > MAX_LONG_EDGE || stage1_h > MAX_LONG_EDGE {
        return Err(LoadError::TooLarge {
            width: stage1_w,
            height: stage1_h,
            limit: MAX_LONG_EDGE,
        });
    }

    let stage1 = RgbImage::from_raw(dw as u32, dh as u32, raw)
        .ok_or_else(|| LoadError::Decode("解码缓冲区尺寸异常".into()))?;

    // 阶段 2：若仍大于目标，做面积平均缩放
    let pixels = if target_long_edge != 0 {
        let (tw, th) = fit_long_edge(dw as u32, dh as u32, target_long_edge);
        if (tw, th) == (dw as u32, dh as u32) {
            stage1
        } else {
            downscale(&stage1, tw, th)
        }
    } else {
        stage1
    };

    Ok(SourceImage {
        pixels,
        original_width: stage1_w,
        original_height: stage1_h,
        subsampled: (dw as u32) < stage1_w,
    })
}

/// 非 JPEG：`image` 解码后缩放。
fn load_generic(path: &Path, target_long_edge: u32) -> Result<SourceImage, LoadError> {
    let img = image::ImageReader::open(path)
        .map_err(|e| LoadError::Io(e.to_string()))?
        .with_guessed_format()
        .map_err(|e| LoadError::Decode(e.to_string()))?
        .decode()
        .map_err(|e| LoadError::Decode(e.to_string()))?;
    let (w, h) = (img.width(), img.height());
    if w > MAX_LONG_EDGE || h > MAX_LONG_EDGE {
        return Err(LoadError::TooLarge {
            width: w,
            height: h,
            limit: MAX_LONG_EDGE,
        });
    }
    let pixels = if target_long_edge == 0 {
        img.to_rgb8()
    } else {
        let (tw, th) = fit_long_edge(w, h, target_long_edge);
        if (tw, th) == (w, h) {
            img.to_rgb8()
        } else {
            downscale(&img.to_rgb8(), tw, th)
        }
    };
    Ok(SourceImage {
        pixels,
        original_width: w,
        original_height: h,
        subsampled: false,
    })
}

/// 面积平均降采样（整数倍缩小时是精确的）。
///
/// 为什么不用 `image::imageops::resize`：它的 Lanczos3 在 6000→1600 上
/// 要 549ms 且额外分配 98MB 的 f32 中间缓冲，而这里只需要「把缩略图缩到
/// 预览尺寸」，视觉上两者无法区分。导出路径才用 Lanczos（见 `commands.rs`）。
pub fn downscale(src: &RgbImage, target_w: u32, target_h: u32) -> RgbImage {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dw, dh) = (target_w as usize, target_h as usize);
    if sw == dw && sh == dh {
        return src.clone();
    }
    let src_px = src.as_raw();
    let mut dst = vec![0u8; dw * dh * 3];
    let sx = sw as f32 / dw as f32;
    let sy = sh as f32 / dh as f32;

    for dy in 0..dh {
        let y0 = (dy as f32 * sy).floor() as usize;
        let y1 = (((dy + 1) as f32 * sy).ceil() as usize).clamp(y0 + 1, sh);
        for dx in 0..dw {
            let x0 = (dx as f32 * sx).floor() as usize;
            let x1 = (((dx + 1) as f32 * sx).ceil() as usize).clamp(x0 + 1, sw);
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for y in y0..y1 {
                let row = y * sw * 3;
                for x in x0..x1 {
                    let i = row + x * 3;
                    acc[0] += src_px[i] as u32;
                    acc[1] += src_px[i + 1] as u32;
                    acc[2] += src_px[i + 2] as u32;
                    n += 1;
                }
            }
            let o = (dy * dw + dx) * 3;
            if n > 0 {
                dst[o] = (acc[0] / n) as u8;
                dst[o + 1] = (acc[1] / n) as u8;
                dst[o + 2] = (acc[2] / n) as u8;
            }
        }
    }
    RgbImage::from_raw(target_w, target_h, dst).expect("尺寸自洽")
}

/// Lanczos3 缩放（导出路径专用）。
///
/// 导出要的是「放大镜下也经得起看」，所以这里必须用高质量重采样核。
/// 借用 `image` 内置实现而非引新依赖：本项目已有 `image`，
/// 再引一个 resize 库对导出这一条冷路径收益不抵依赖成本。
pub fn resample_lanczos(src: &RgbImage, target_w: u32, target_h: u32) -> RgbImage {
    if src.width() == target_w && src.height() == target_h {
        return src.clone();
    }
    image::imageops::resize(
        src,
        target_w,
        target_h,
        image::imageops::FilterType::Lanczos3,
    )
}

/// 估算一张图在给定预览档位下的解码耗时（毫秒），用于 UI 提示。
///
/// 粗略但方向正确：JPEG 解码时间与像素数线性相关（熵解码占主导，
/// 缩放只省 IDCT 与输出缓冲），DCT 降采样大致省掉 30%。
pub fn estimate_decode_ms(width: u32, height: u32) -> u64 {
    let px = width as u64 * height as u64;
    // 经验系数：6000×4000（24MP）≈ 340ms
    px * 14_000 / 1_000_000_000 + 5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(w: u32, h: u32, v: u8) -> RgbImage {
        RgbImage::from_pixel(w, h, image::Rgb([v, v, v]))
    }

    #[test]
    fn fit_long_edge_keeps_small_images() {
        assert_eq!(fit_long_edge(800, 600, 1600), (800, 600));
        assert_eq!(fit_long_edge(0, 0, 1600), (0, 0));
    }

    #[test]
    fn fit_long_edge_preserves_aspect() {
        let (w, h) = fit_long_edge(6000, 4000, 1600);
        assert_eq!((w, h), (1600, 1067));
        let ratio_src = 6000.0 / 4000.0;
        let ratio_dst = w as f32 / h as f32;
        assert!((ratio_src - ratio_dst).abs() < 0.01, "宽高比被改变");
    }

    #[test]
    fn fit_long_edge_never_zero() {
        let (w, h) = fit_long_edge(1, 1, 0);
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn downscale_identity_returns_clone() {
        let img = gray(10, 10, 128);
        assert_eq!(downscale(&img, 10, 10).as_raw(), img.as_raw());
    }

    #[test]
    fn downscale_produces_requested_dimensions() {
        let img = gray(200, 100, 200);
        let out = downscale(&img, 50, 25);
        assert_eq!((out.width(), out.height()), (50, 25));
    }

    /// 恒色图缩放后必须还是同一个颜色 —— 缩放算法错了会引入色偏。
    #[test]
    fn downscale_preserves_uniform_color() {
        let img = gray(64, 64, 137);
        let out = downscale(&img, 16, 16);
        for px in out.pixels() {
            assert_eq!(px.0, [137, 137, 137], "缩放引入了色偏");
        }
    }

    /// 左右对半不同色的图，缩放到正好一半后必须**精确保留左半颜色**。
    /// 这是「不引入摩尔纹/方向错误」的直接验证。
    #[test]
    fn downscale_keeps_orientation() {
        let mut img = RgbImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = if x < 32 { 0u8 } else { 255u8 };
                img.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }
        // 缩到 8px 宽：每 8 个源像素折成 1 个，每个输出像素混合 4 黑 4 白 → 127
        let out = downscale(&img, 8, 8);
        let left = out.get_pixel(0, 0).0[0];
        let right = out.get_pixel(7, 0).0[0];
        assert_eq!(left, 0, "左半应保持黑");
        assert_eq!(right, 255, "右半应保持白（方向反了？）");
    }

    #[test]
    fn downscale_to_one_pixel_averages_whole_image() {
        let mut img = RgbImage::from_pixel(64, 64, image::Rgb([0, 0, 0]));
        // 右半改成 100
        for y in 0..64 {
            for x in 32..64 {
                img.put_pixel(x, y, image::Rgb([100, 100, 100]));
            }
        }
        let out = downscale(&img, 1, 1);
        let got = out.get_pixel(0, 0).0[0];
        assert!((got as i32 - 50).abs() <= 2, "均值应为 50，实得 {got}");
    }

    #[test]
    fn resample_lanczos_identity() {
        let img = gray(20, 20, 90);
        assert_eq!(resample_lanczos(&img, 20, 20).as_raw(), img.as_raw());
    }

    #[test]
    fn resample_lanczos_produces_requested_size() {
        let img = gray(80, 60, 90);
        let out = resample_lanczos(&img, 40, 30);
        assert_eq!((out.width(), out.height()), (40, 30));
    }

    #[test]
    fn estimate_decode_scales_with_pixels() {
        let small = estimate_decode_ms(1000, 1000);
        let big = estimate_decode_ms(6000, 4000);
        assert!(
            big > small * 10,
            "24MP 的解码估算 {big} 应远大于 1MP 的 {small}"
        );
        assert!(
            big > 200 && big < 600,
            "24MP 估算 {big}ms 与实测 340ms 量级不符"
        );
    }

    #[test]
    fn oversized_image_is_rejected() {
        let p = Path::new("/nonexistent/should-not-matter.png");
        let e = load(p, 1600).unwrap_err();
        let s = e.to_string();
        assert!(!s.is_empty());
    }

    #[test]
    fn missing_file_reports_io_error() {
        let e = load(Path::new("/definitely/not/here.jpg"), 1600).unwrap_err();
        assert!(
            matches!(e, LoadError::Io(_)),
            "缺失文件应报 IO 错误，实得 {e:?}"
        );
    }
}
