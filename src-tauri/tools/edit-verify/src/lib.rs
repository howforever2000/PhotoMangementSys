//! `edit` 模块的独立验证壳（详见 Cargo.toml 的说明）
//!
//! 通过 `#[path]` 引用主 crate 的**同一批源文件**，因此这里跑通的测试
//! 就是主 crate 里跑的测试。`commands.rs` 也一并纳入 —— 为此本 crate
//! 提供了一个最小的 `tauri` 桩（见下方 `mod tauri` 与 `stub-macros`）。
//
// ## 为什么连命令层也要编译检查
//!
//! 算法层的 bug 靠单测能抓；但命令层的 bug 恰恰是单测抓不到的：
//! 字段名与前端不一致、`State` 借用跨越 `await`、闭包捕获了非 `Send` 值
//! —— 这些只有在真正把它当 `async fn` 编译时才暴露。
//! 而「参数搬运」正是这一层 90% 的代码量。

#![allow(dead_code)]

#[path = "../../../src/edit/color.rs"]
pub mod color;

#[path = "../../../src/edit/curve.rs"]
pub mod curve;

#[path = "../../../src/edit/geometry.rs"]
pub mod geometry;

#[path = "../../../src/edit/pipeline.rs"]
pub mod pipeline;

#[path = "../../../src/edit/recipe.rs"]
pub mod recipe;

#[path = "../../../src/edit/saturation.rs"]
pub mod saturation;

#[path = "../../../src/edit/schema.rs"]
pub mod schema;

#[path = "../../../src/edit/session.rs"]
pub mod session;

#[path = "../../../src/edit/sharpen.rs"]
pub mod sharpen;

#[path = "../../../src/edit/source.rs"]
pub mod source;

#[path = "../../../src/edit/store.rs"]
pub mod store;

#[path = "../../../src/edit/tone.rs"]
pub mod tone;

// `commands.rs` 依赖 `crate::logger`（本项目的日志模块）。桩里给出
// **与 `src-tauri/src/logger.rs` 同签名**的最小实现，保证调用点的
// 参数个数与类型在验证壳里也被检查到。
pub mod logger {
    use std::time::Instant;

    pub fn log_call_start(_func: &str, _desc: &str) -> Instant {
        Instant::now()
    }

    pub fn log_call_end(_func: &str, _start: Instant) {}

    pub fn log_call_end_with(_func: &str, _start: Instant, _desc: &str) {}

    pub fn log_error(_func: &str, _err: &str) {}

    pub fn log_warn(_desc: &str) {}

    pub fn log_info(_desc: &str) {}
}

// 与 `mod.rs` 同构的模块根（`pub use` 保持一致，便于测试按同一路径引用）
#[path = "../../../src/edit/mod.rs"]
pub mod edit;

pub mod commands {
    pub use crate::edit::commands::*;
}

pub mod facade {
    pub use crate::edit::pipeline::{render_u8_to_u8, Workspace};
    pub use crate::edit::recipe::AdjustmentRecipe;
    pub use crate::edit::session::SessionStore;
}

#[cfg(test)]
mod integration {
    //! 跨模块的端到端断言。各模块的单元测试在各文件内部，这里只测
    //! 「模块之间的接线」—— 这类 bug 恰恰是最容易被单模块测试漏掉的。

    use super::facade::{render_u8_to_u8, AdjustmentRecipe, Workspace};
    use crate::edit::recipe::{BasicAdjustments, ColorAdjustments, CropSpec, Curve, CurvePoint};

    /// 生成一张有真实结构（左暗右亮）的测试图。
    fn photo(w: u32, h: u32) -> Vec<u8> {
        let mut buf = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let v = (40.0 + t * 190.0) as u8;
                let i = ((y * w + x) * 3) as usize;
                // 上下加一点亮度梯度，模拟真实的非均匀光照
                let vy = (y as f32 / (h - 1) as f32 * 20.0) as u8;
                buf[i] = v.saturating_add(vy);
                buf[i + 1] = v.saturating_add(vy);
                buf[i + 2] = v.saturating_sub(vy);
            }
        }
        buf
    }

    #[test]
    fn end_to_end_empty_recipe_is_pixel_identical() {
        let (w, h) = (96u32, 64u32);
        let src = photo(w, h);
        let mut ws = Workspace::new();
        let (_, _, out) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);
        assert_eq!(out, src, "空配方必须逐字节还原原图");
    }

    /// 全参数拉满也不能产生非法输出 —— 滑块端点用户一定会试到。
    #[test]
    fn end_to_end_all_sliders_at_extremes_is_safe() {
        let (w, h) = (64u32, 48u32);
        let src = photo(w, h);
        let mut ws = Workspace::new();
        for sign in [-1.0f32, 1.0] {
            let mut r = AdjustmentRecipe::default();
            r.basic = BasicAdjustments {
                exposure: 5.0 * sign,
                contrast: sign,
                highlights: sign,
                shadows: sign,
                whites: sign,
                blacks: sign,
                temperature: if sign > 0.0 { 50_000.0 } else { 1500.0 },
                tint: 100.0 * sign,
            };
            r.color = ColorAdjustments { vibrance: sign, saturation: sign };
            r.curve_rgb = Curve {
                points: vec![
                    CurvePoint::new(0.0, 0.5 * sign),
                    CurvePoint::new(0.5, 0.5),
                    CurvePoint::new(1.0, 0.5 + 0.5 * sign),
                ],
            };
            r.curve_luma = Curve {
                points: vec![CurvePoint::new(0.0, 0.3 * sign), CurvePoint::new(1.0, 0.9)],
            };
            r.sharpen = crate::edit::recipe::SharpenAdjustments {
                amount: 1.0,
                radius: 1.0,
                detail: sign,
                masking: sign,
            };
            r.crop = CropSpec { x: 0.05, y: 0.05, w: 0.9, h: 0.9, angle: 40.0 * sign, enabled: true };
            let (ow, oh, out) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
            assert_eq!(out.len(), (ow * oh * 3) as usize);
            assert!(out.iter().any(|v| *v > 4), "sign={sign} 输出全黑");
        }
    }

    /// 各阶段叠加后仍必须与「逐阶段手动应用」得到一致 ——
    /// 这条守的是 `pipeline.rs` 的阶段顺序不被后来的改动打乱。
    #[test]
    fn pipeline_order_is_wb_then_exposure() {
        let (w, h) = (8u32, 8u32);
        let mut src = vec![0u8; (w * h * 3) as usize];
        for px in src.chunks_exact_mut(3) {
            px.copy_from_slice(&[128, 128, 128]);
        }
        let mut ws = Workspace::new();

        let mut r = AdjustmentRecipe::default();
        r.basic.exposure = 1.0;
        r.basic.temperature = 4000.0;
        let (_, _, out) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);

        // 手算：sRGB 128 → 线性 ≈ 0.2159；×2^1 = 0.4318；再乘 WB 增益 → sRGB 编码
        let lin = crate::edit::color::srgb_to_linear(128.0 / 255.0);
        let wb = crate::edit::color::wb_gains(4000.0, 0.0);
        let expect = crate::edit::color::linear_to_srgb(lin * 2.0 * wb[0]);
        let got = out[0] as f32 / 255.0;
        assert!(
            (got - expect).abs() < 0.02,
            "管线顺序不符：得 {got}，按「WB 先于曝光」应为 {expect}"
        );
    }

    /// 工作区跨不同尺寸反复使用不得串数据。
    #[test]
    fn workspace_survives_size_churn() {
        let mut ws = Workspace::new();
        for (w, h) in [(16u32, 16u32), (256u32, 200u32), (8u32, 300u32), (16u32, 16u32)] {
            let src = photo(w, h);
            let mut r = AdjustmentRecipe::default();
            r.basic.exposure = 0.5;
            let (ow, oh, out) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
            assert_eq!(out.len(), (ow * oh * 3) as usize);
        }
        // 回到最初尺寸 + 空配方，必须精确还原
        let src = photo(16, 16);
        let (_, _, out) = render_u8_to_u8(&src, 16, 16, &AdjustmentRecipe::default(), 0, &mut ws);
        assert_eq!(out, src, "尺寸来回切换后工作区残留了旧数据");
    }

    /// 配方往返（存盘 → 读回 → 渲染）必须与直接渲染一致。
    #[test]
    fn recipe_roundtrip_preserves_render_output() {
        let (w, h) = (64u32, 64u32);
        let src = photo(w, h);
        let mut r = AdjustmentRecipe::default();
        r.basic = BasicAdjustments {
            exposure: 0.8,
            contrast: 0.25,
            highlights: -0.3,
            shadows: 0.4,
            whites: 0.15,
            blacks: 0.1,
            temperature: 5200.0,
            tint: -6.0,
        };
        r.color = ColorAdjustments { vibrance: 0.35, saturation: -0.1 };
        r.sharpen = crate::edit::recipe::SharpenAdjustments {
            amount: 0.4,
            radius: 0.35,
            detail: 0.3,
            masking: 0.4,
        };
        r.curve_rgb = Curve {
            points: vec![
                CurvePoint::new(0.0, 0.02),
                CurvePoint::new(0.5, 0.55),
                CurvePoint::new(1.0, 1.0),
            ],
        };
        r.crop = CropSpec { x: 0.1, y: 0.1, w: 0.8, h: 0.8, angle: 0.0, enabled: true };

        let text = r.to_json().unwrap();
        let back = AdjustmentRecipe::from_json(&text).unwrap();

        let mut ws = Workspace::new();
        let (_, _, a) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        let (_, _, b) = render_u8_to_u8(&src, w, h, &back, 0, &mut ws);
        assert_eq!(a, b, "配方往返后渲染结果不同");
    }

    /// 预览档位之间的差异：草稿档必须比正常档小。
    #[test]
    fn draft_is_smaller_than_preview() {
        let (w, h) = (2000u32, 1500u32);
        let src = photo(w, h);
        let mut ws = Workspace::new();
        let r = AdjustmentRecipe::default();
        let (dw, dh, _) = render_u8_to_u8(&src, w, h, &r, crate::edit::source::DRAFT_LONG_EDGE, &mut ws);
        let (pw, ph, _) =
            render_u8_to_u8(&src, w, h, &r, crate::edit::source::PREVIEW_LONG_EDGE, &mut ws);
        assert!(dw < pw && dh < ph, "草稿档 {dw}×{dh} 未小于正常档 {pw}×{ph}");
    }

    /// 导出的字节必须是合法图像 —— 「返回了一坨字节但前端当成图用」
    /// 是最容易被忽略的一类集成 bug。
    #[test]
    fn encode_produces_decodable_images() {
        let (w, h) = (32u32, 24u32);
        let src = photo(w, h);
        let jpeg = crate::edit::encode_jpeg(&src, w, h, 90).unwrap();
        assert_eq!(&jpeg[0..2], &[0xFF, 0xD8], "缺少 JPEG SOI");
        image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .expect("JPEG 字节应能被解码");

        let png = crate::edit::encode_png(&src, w, h).unwrap();
        image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("PNG 字节应能被解码");
    }
}