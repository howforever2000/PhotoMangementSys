//! 头像图片处理工具（FEAT-045）
//!
//! 中心方裁 + 缩放 + JPEG 落盘，供两类头像复用：
//! - 用户头像（`set_user_avatar`）：256×256
//! - 人物自选头像（`set_person_avatar_from_photo`）：96×96（与自动裁剪一致）
//!
//! JPEG 走 jpeg-decoder 分级降采样快速路径（与 persons.rs 代表脸裁剪同源思路），
//! 避免 6000×4000 大图全尺寸解码卡顿（BUG-2026-0815-003 更换封面卡顿的教训）。

use std::path::Path;

/// 中心方裁：读图 → 取中心正方形 → 缩放 `size×size` → JPEG 落盘到 `dst`
///
/// 输入支持项目已启用的格式（jpg/jpeg/png/webp/gif/bmp）；输出统一 JPEG。
pub fn crop_square(src: &Path, dst: &Path, size: u32) -> Result<(), String> {
    if !src.is_file() {
        return Err(format!("图片不存在: {}", src.display()));
    }
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let is_jpeg = name.ends_with(".jpg") || name.ends_with(".jpeg");

    let cropped = if is_jpeg {
        crop_square_jpeg(src, size)?
    } else {
        let img = image::open(src).map_err(|e| format!("图片解码失败: {e}"))?;
        let side = img.width().min(img.height()).max(1);
        let x = (img.width() - side) / 2;
        let y = (img.height() - side) / 2;
        img.crop_imm(x, y, side, side)
    };

    let out = cropped.resize_exact(size, size, image::imageops::FilterType::Triangle);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建头像目录失败: {e}"))?;
    }
    out.save_with_format(dst, image::ImageFormat::Jpeg)
        .map_err(|e| format!("保存头像失败: {e}"))
}

/// JPEG 快速路径：按目标 `size` 选最大可用降采样档（1/8、1/4、1/2、1/1），
/// 解码后中心方裁坐标在实际解码尺寸上直接计算（无需换算回原图）。
fn crop_square_jpeg(src: &Path, size: u32) -> Result<image::DynamicImage, String> {
    // 先读头拿原尺寸
    let probe = std::fs::File::open(src).map_err(|e| format!("打开图片失败: {e}"))?;
    let mut head = jpeg_decoder::Decoder::new(std::io::BufReader::new(probe));
    let _ = head.read_info();
    let info = head.info().ok_or("无法读取图片头信息")?;
    let (w0, h0) = (info.width as u32, info.height as u32);

    // 分级选档：降采样后短边仍 ≥ size 的最大档（jpeg-decoder 仅支持 2 的幂档位）
    let mut chosen = 1u32;
    for &d in &[8u32, 4, 2] {
        if (w0.min(h0)) as f64 / d as f64 >= size as f64 {
            chosen = d;
            break;
        }
    }
    let tw = ((w0 + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;
    let th = ((h0 + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;

    let file2 = std::fs::File::open(src).map_err(|e| format!("打开图片失败: {e}"))?;
    let mut dec = jpeg_decoder::Decoder::new(std::io::BufReader::new(file2));
    let _ = dec.scale(tw, th);
    let pixels = dec.decode().map_err(|e| format!("JPEG 解码失败: {e:?}"))?;
    let info2 = dec.info().ok_or("无法读取解码信息")?;
    let aw = (info2.width as u32).max(1);
    let ah = (info2.height as u32).max(1);
    let side = aw.min(ah);
    let x = (aw - side) / 2;
    let y = (ah - side) / 2;
    Ok(image::DynamicImage::ImageRgb8(
        image::RgbImage::from_raw(aw, ah, pixels).ok_or("像素数据长度不符")?,
    )
    .crop_imm(x, y, side, side))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 非 JPEG（PNG）中心方裁 + JPEG 快速路径均应产出 size×size JPEG
    #[test]
    fn crop_square_png_and_jpeg() {
        let tmp = std::env::temp_dir().join(format!("avatar_crop_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // PNG：非正方形 160×80 → 中心方裁 64×64
        let png = tmp.join("in.png");
        image::RgbImage::from_fn(160, 80, |x, _| {
            if x < 80 {
                image::Rgb([255u8, 0, 0])
            } else {
                image::Rgb([0, 0, 255u8])
            }
        })
        .save(&png)
        .unwrap();
        let dst_png = tmp.join("out.png.jpg");
        crop_square(&png, &dst_png, 64).unwrap();
        let img = image::open(&dst_png).unwrap();
        assert_eq!((img.width(), img.height()), (64, 64));

        // JPEG：1600×800 走快速路径 → 96×96
        let jpg = tmp.join("in.jpg");
        image::RgbImage::from_fn(1600, 800, |_, _| image::Rgb([0u8, 128, 0]))
            .save(&jpg)
            .unwrap();
        let dst_jpg = tmp.join("out.jpg");
        crop_square(&jpg, &dst_jpg, 96).unwrap();
        let img2 = image::open(&dst_jpg).unwrap();
        assert_eq!((img2.width(), img2.height()), (96, 96));

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 源文件不存在应报错而非 panic
    #[test]
    fn crop_square_missing_source_errors() {
        let r = crop_square(
            Path::new("/nonexistent/avatar_src.jpg"),
            Path::new("/tmp/avatar_dst.jpg"),
            96,
        );
        assert!(r.is_err());
    }
}
