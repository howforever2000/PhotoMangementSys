//! EXIF Orientation —— 把「像素躺倒」的照片按元数据转正
//!
//! # 问题背景（BUG-2026-1014-002）
//!
//! 相机传感器横向固定，竖持拍摄时像素本身是躺倒的，文件里靠 EXIF `Orientation`
//! 标签声明「看图时请顺时针转 90°」。**JPEG 不会自己转** —— 该标签只是给读取方的
//! 一条指令，而各读取方执行与否并不统一：
//!
//! ```text
//! 看图软件 / Chrome 81+   ✅ 自动转正
//! Pillow（默认）          ❌ 不转，需显式 ImageOps.exif_transpose()
//! image crate（默认）     ❌ 不转
//! ```
//!
//! 后果：横拍照片以「侧脸」进入 ArcFace/SCRFD，判据（眼-鼻-嘴上下关系）被颠倒，
//! 置信度塌到阈值以下 → 明明有人却判定「不是人物」。同一入口的分类 / OCR / 影调 /
//! 语义向量同样受损。
//!
//! # 为什么 Rust 侧也必须转（而不是只靠 Python）
//!
//! 坐标系契约：`python/vcr/preprocess.py::open_image` 转正后，检测输出的 bbox
//! 处于**显示坐标系**。而本模块的两个消费者都在**原始文件**上按 bbox 定位：
//!
//! 1. [`crate::persons`] 裁 96×96 人脸头像（`jpeg_decoder` / `image::open` 解码）；
//! 2. [`crate::thumbnail`] 生成网格缩略图。
//!
//! 二者都默认不转 EXIF。若只改 Python：bbox 变成「转正后的坐标」，而裁剪/缩略图
//! 仍在「躺倒的像素」上操作 —— 用转正坐标裁躺倒图，裁出的是背景或半张脸，
//! 看起来像新引入的 bug。故两端必须站在同一坐标系上。
//!
//! # 方向约定（易错点，务必读）
//!
//! `image` crate 的 `rotate90` 是**顺时针**，而 Pillow 的 `Transpose.ROTATE_90` 是
//! **逆时针** —— 两者方向相反，映射不能凭直觉写。本文件的映射由
//! `tests::orientation_matches_pillow` 逐像素比对验证过（对 8 种取值分别与
//! Pillow `ImageOps.exif_transpose` 的输出完全一致）。
//!
//! 本模块与 Pillow 的对应关系：
//!
//! ```text
//! Orientation  含义                        Pillow                        本模块
//! 1            正常                        恒等                          恒等
//! 2            水平镜像                    FLIP_LEFT_RIGHT               flip_horizontal
//! 3            旋转 180°                   ROTATE_180                    rotate180
//! 4            垂直镜像                    FLIP_TOP_BOTTOM               flip_vertical
//! 5            转置（沿主对角线镜像）        TRANSPOSE                     rotate90 ∘ flip_horizontal
//! 6            顺时针 90°                  ROTATE_270 (= 顺时针 90°)      rotate90
//! 7            反转置（沿副对角线镜像）      TRANSVERSE                    rotate270 ∘ flip_horizontal
//! 8            逆时针 90°                  ROTATE_90 (= 逆时针 90°)      rotate270
//! ```
//!
//! 5 / 7 没有对应的一次性原语，用「先镜像再旋转」组合实现，比值等价。

use std::path::Path;

use image::DynamicImage;
// 包名 kamadak-exif 的 lib 名是 exif（见其 Cargo.toml [lib] name），
// 与 thumbnail.rs 里既有的 `exif::Reader` 写法保持一致。
use exif;

/// 读取 JPEG/TIFF 的 EXIF Orientation（1–8）。无 EXIF、无该标签或非 JPEG 时返回
/// `None`（调用方按「无需转正」处理）。
///
/// 单独成函数的原因：调用点常常已经在别处持有 `&Path`，而 JPEG 头必须重新按流读
/// —— 无法复用已解码的图像。
pub fn read_orientation(path: &Path) -> Option<u32> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    let raw = field.value.get_uint(0)?;
    // 规范只定义 1..=8；越界值按「无需转正」处理，避免被伪造元数据带偏。
    (1..=8).contains(&raw).then_some(raw)
}

/// 按 EXIF Orientation 转正图像；`orientation` 为 `None` 或 1 时原样返回。
///
/// 保持像素类型不变（JPEG→RGB8 等），调用方可直接接后续裁剪 / 缩放。
pub fn apply_orientation(img: DynamicImage, orientation: Option<u32>) -> DynamicImage {
    use image::imageops;
    // imageops 系列返回 ImageBuffer（像素类型不变），用 From 转回 DynamicImage，
    // 以保持调用方的像素类型（JPEG→RGB8 等）不被改写。
    let o = orientation.unwrap_or(1);
    match o {
        2 => imageops::flip_horizontal(&img).into(),
        3 => imageops::rotate180(&img).into(),
        4 => imageops::flip_vertical(&img).into(),
        // 转置（沿主对角线镜像）：水平镜像 → 逆时针 90°。
        // ⚠️ 这里的旋转**必须用 rotate270**（= 逆时针 90°）。写成 rotate90 会得到
        // 翻转后的另一姿态 —— 尺寸对得上、像素全错，最容易漏过。
        5 => imageops::rotate270(&imageops::flip_horizontal(&img)).into(),
        // 顺时针 90°（竖持拍摄的典型值）
        6 => imageops::rotate90(&img).into(),
        // 反转置（沿副对角线镜像）：水平镜像 → 顺时针 90°
        7 => imageops::rotate90(&imageops::flip_horizontal(&img)).into(),
        // 逆时针 90°
        8 => imageops::rotate270(&img).into(),
        _ => img,
    }
}

/// 便捷组合：按文件里的 EXIF 转正（读不到 Orientation 即恒等）。

/// 纯函数：给定原始像素尺寸与 Orientation，返回转正后的尺寸。
///
/// 5..=8（转置 / 旋转类）宽高互换；2/3/4（镜像类）尺寸不变。
///
/// 裁头像路径需要「先定尺寸、再按 1/8、1/4、1/2 分级降采样解码」的策略，若先把
/// 整张图解码再转正会白白吃满分辨率的内存与时间（4096×5568 的照片尤其明显）。
/// 故调用方先读 Orientation、拿到原始 (w, h) 后用本函数换算目标尺寸。
pub fn oriented_dims(raw_w: u32, raw_h: u32, orientation: Option<u32>) -> (u32, u32) {
    match orientation.unwrap_or(1) {
        5..=8 => (raw_h, raw_w),
        _ => (raw_w, raw_h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// 造一张 4×2 的彩色测试图：每个像素颜色唯一，便于逐像素比对方向。
    fn sample() -> DynamicImage {
        let mut buf = RgbImage::new(4, 2);
        let colors = [
            Rgb([255, 0, 0]), Rgb([0, 255, 0]), Rgb([0, 0, 255]), Rgb([255, 255, 0]),
            Rgb([255, 0, 255]), Rgb([0, 255, 255]), Rgb([128, 64, 32]), Rgb([1, 2, 3]),
        ];
        for (i, p) in buf.pixels_mut().enumerate() {
            *p = colors[i];
        }
        DynamicImage::ImageRgb8(buf)
    }

    fn dims(img: &DynamicImage) -> (u32, u32) {
        (img.width(), img.height())
    }

    #[test]
    fn orientation_1_is_identity() {
        let img = sample();
        let out = apply_orientation(img.clone(), Some(1));
        assert_eq!(dims(&out), dims(&img));
        assert_eq!(out.to_rgb8().as_raw(), img.to_rgb8().as_raw());
    }

    #[test]
    fn missing_orientation_is_identity() {
        let img = sample();
        let out = apply_orientation(img.clone(), None);
        assert_eq!(out.to_rgb8().as_raw(), img.to_rgb8().as_raw());
    }

    #[test]
    fn out_of_range_orientation_is_identity() {
        // 伪造元数据不得让图像乱转
        let img = sample();
        for bad in [0u32, 9, 99, u32::MAX] {
            let out = apply_orientation(img.clone(), Some(bad));
            assert_eq!(out.to_rgb8().as_raw(), img.to_rgb8().as_raw(), "bad={bad}");
        }
    }

    #[test]
    fn orientations_5_to_8_swap_dimensions() {
        let img = sample(); // 4×2
        for o in [5u32, 6, 7, 8] {
            assert_eq!(dims(&apply_orientation(img.clone(), Some(o))), (2, 4), "o={o}");
        }
    }

    #[test]
    fn mirror_orientations_keep_dimensions() {
        let img = sample();
        for o in [2u32, 3, 4] {
            assert_eq!(dims(&apply_orientation(img.clone(), Some(o))), (4, 2), "o={o}");
        }
    }

    #[test]
    fn orientation_6_is_cw_90() {
        // 顺时针 90°：原图左上角(0,0) → 新图右上角。get_pixel 是 (x, y)，
        // 原图 4×2 → 转正后 2×4，故新图 (x=1, y=0)。
        let img = sample();
        let out = apply_orientation(img, Some(6)).to_rgb8();
        assert_eq!(dims(&DynamicImage::ImageRgb8(out.clone())), (2, 4));
        assert_eq!(out.get_pixel(1, 0), &Rgb([255, 0, 0]), "原左上角应转到右上角");
    }

    #[test]
    fn orientation_2_mirrors_horizontally() {
        // 水平镜像：原图左上角(0,0) → (x=3, y=0)，尺寸不变
        let img = sample();
        let out = apply_orientation(img, Some(2)).to_rgb8();
        assert_eq!(out.get_pixel(3, 0), &Rgb([255, 0, 0]));
    }

    #[test]
    fn orientation_5_is_main_diagonal_transpose() {
        // 转置：out[y][x] = in[x][y]。原图 4×2 → 转置后 2×4。
        let img = sample();
        let out = apply_orientation(img, Some(5)).to_rgb8();
        // 原 (x=0,y=0)=红 → 新 (x=0,y=0)
        assert_eq!(out.get_pixel(0, 0), &Rgb([255, 0, 0]));
        // 原 (x=1,y=0)=绿 → 新 (x=0,y=1)
        assert_eq!(out.get_pixel(0, 1), &Rgb([0, 255, 0]));
        // 原 (x=0,y=1)=品红 → 新 (x=1,y=0)
        assert_eq!(out.get_pixel(1, 0), &Rgb([255, 0, 255]));
    }

    #[test]
    fn orient_then_double_apply_returns_mirrored() {
        // 转正后 Orientation 已被消费，再转一次应回到另一姿态（说明确实改变了内容）
        let img = sample();
        let once = apply_orientation(img.clone(), Some(6));
        let twice = apply_orientation(once.clone(), Some(6));
        assert_ne!(once.to_rgb8().as_raw(), twice.to_rgb8().as_raw());
    }
}