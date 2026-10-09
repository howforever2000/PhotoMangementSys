//! 非破坏图像编辑引擎
//! ======================================================================
//! 目标：给「创意工坊」提供一套 Lightroom 基准调色能力
//! （曝光 / 对比度 / 高光 / 阴影 / 白色 / 黑色 / 色温 / 色调 / 曲线 /
//!   裁剪拉直 / 饱和度 / 自然饱和度 / 锐化），且：
//!
//! - **非破坏**：原图永不改动，全部状态在 `recipe::AdjustmentRecipe` 里，
//!   可随时重算、可存 sidecar、可撤销、可导入导出；
//! - **纯 Rust**：不依赖 OpenCV / Python 微服务 / 任何 C 库。
//!
//! ## 为什么整套放在 Rust 端而不是 Python 端
//!
//! 本项目原本的工坊算子（均衡化 / 模糊）走 Python 微服务
//! （`python-studio/server.py` + `studio.rs` 拉起）。调色引擎**故意不沿用**：
//!
//! | 维度 | Rust（本模块） | Python 微服务 |
//! |---|---|---|
//! | 拖动滑块的往返 | 进程内调用，~10ms | HTTP + multipart 上传 + cv2 转换，100~300ms |
//! | 冷启动 | 0（常驻） | python + fastapi + cv2，1~2s |
//! | 依赖体积 | 0 新增 | cv2 约 60MB + 独立进程生命周期管理 |
//! | 结果确定性 | 逐像素 f32，可单测 | numpy 隐式 dtype 提升，易出 NaN |
//!
//! 基准调色的核心是「几十次逐像素浮点运算 + 查表」，Python 在这里没有任何优势；
//! 而它对**响应延迟**的要求（每帧都要重算）恰好是微服务最不擅长的。
//! 微服务保留给「算子形态」的滤镜（均衡化、模糊）——那类操作参数少、
//! 用户不追求实时预览。
//!
//! ## 模块地图
//!
//! ```text
//!   recipe.rs    配方结构（serde 持久化 / 导入导出）  ← 唯一真相源
//!   schema.rs    参数表（滑块定义，前端据此渲染面板）
//!   color.rs     色彩空间基础数学（传输函数 / 亮度 / 白平衡增益）
//!   tone.rs      线性 HDR 影调（白平衡 / 曝光 / 对比度 / 影调分带）
//!   curve.rs     色调曲线（Catmull-Rom → LUT → 保色相应用）
//!   saturation.rs 饱和度 / 自然饱和度 / 保色相裁剪
//!   sharpen.rs   带边缘蒙版的 USM 锐化
//!   geometry.rs  裁剪与拉直（纯坐标数学）
//!   pipeline.rs  阶段编排 + 工作区（缓冲复用）
//!   source.rs    解码与预览分级加载
//!   store.rs     sidecar 原子存盘 / 导入导出
//!   session.rs   编辑会话（跨命令共享源图与工作区）
//!   commands.rs  Tauri 命令薄壳
//! ```

pub mod color;
pub mod commands;
pub mod curve;
pub mod geometry;
pub mod pipeline;
pub mod recipe;
pub mod saturation;
pub mod schema;
pub mod session;
pub mod sharpen;
pub mod source;
pub mod store;
pub mod tone;

pub use recipe::AdjustmentRecipe;
pub use session::{EditSession, SessionStore};

/// 便捷重导出：渲染一帧到 JPEG 字节（供 `commands.rs` 与单测共用）。
///
/// # Errors
/// 编码失败时返回错误信息（`image` 的 JPEG 编码器对合法尺寸不会失败，
/// 但保持 `Result` 以免未来编码路径变化时悄悄产出空图）。
pub fn encode_jpeg(rgb: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, String> {
    let img = image::RgbImage::from_raw(width, height, rgb.to_vec())
        .ok_or_else(|| "构建输出图像失败（尺寸不匹配）".to_string())?;
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode_image(&img)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;
    Ok(out)
}

/// 便捷重导出：渲染一帧到 PNG 字节（无损导出）。
///
/// # Errors
/// 编码失败时返回错误信息。
pub fn encode_png(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let img = image::RgbImage::from_raw(width, height, rgb.to_vec())
        .ok_or_else(|| "构建输出图像失败（尺寸不匹配）".to_string())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败: {e}"))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> (Vec<u8>, u32, u32) {
        let mut v = vec![0u8; (w * h * 3) as usize];
        for (i, px) in v.chunks_exact_mut(3).enumerate() {
            px.copy_from_slice(&[(i % 256) as u8, 128, 200]);
        }
        (v, w, h)
    }

    #[test]
    fn jpeg_encode_produces_valid_jpeg() {
        let (px, w, h) = img(64, 48);
        let out = encode_jpeg(&px, w, h, 90).unwrap();
        assert!(out.len() > 100, "JPEG 字节数 {} 异常", out.len());
        // JPEG SOI 标记
        assert_eq!(&out[0..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn png_encode_produces_valid_png() {
        let (px, w, h) = img(32, 32);
        let out = encode_png(&px, w, h).unwrap();
        assert_eq!(
            &out[0..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );
    }

    #[test]
    fn png_roundtrip_is_lossless() {
        let (px, w, h) = img(16, 16);
        let png = encode_png(&px, w, h).unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .to_rgb8();
        assert_eq!(decoded.as_raw(), &px[..], "PNG 应无损");
    }

    #[test]
    fn encode_rejects_size_mismatch() {
        let px = vec![0u8; 10];
        assert!(encode_jpeg(&px, 100, 100, 90).is_err());
        assert!(encode_png(&px, 100, 100).is_err());
    }

    #[test]
    fn jpeg_quality_affects_size() {
        let (px, w, h) = img(256, 256);
        let low = encode_jpeg(&px, w, h, 30).unwrap().len();
        let high = encode_jpeg(&px, w, h, 95).unwrap().len();
        assert!(high > low, "高质量 {high} 应大于低质量 {low}");
    }
}
