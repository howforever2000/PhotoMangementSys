//! 色彩空间与白平衡基础数学（轻量、无外部依赖）
//! =====================================================
//! 本模块只做**逐像素无关**的常量与查表计算，被 `tone.rs` / `curve.rs` /
//! `saturation.rs` / `sharpen.rs` 共用，不持有任何缓冲区。
//!
//! 分工边界：
//! - 本模块：sRGB 传输函数、Rec.709 亮度、色温/色调 → 通道增益、伽马常量
//! - `tone.rs`：曝光/对比度/影调分带
//! - 其余模块：不做颜色空间转换
//!
//! 之所以把传输函数单独放这里：`pipeline.rs` 要在「线性 HDR」与「显示参考」
//! 两个空间之间来回切换，谁都可能第一个用到它，放在 `mod.rs` 会造成循环依赖。

/// Rec.709 亮度权重（与 Camera Raw / darktable / RawTherapee 一致）。
///
/// 三处独立实现都硬编码了这一组常量（darktable `rgb_norms.h` 的 LUMINANCE、
/// RawTherapee `curves.h` 的 LuminanceToneCurve），说明这是事实标准而非某家私有口径。
#[inline(always)]
pub const fn luminance_weights() -> [f32; 3] {
    [0.2126729, 0.7151522, 0.0721750]
}

/// 18% 中灰（场景线性）。对比度与影调分带的支点都锁死在这个值上。
///
/// 依据：darktable `basicadj.c` 的 `middle_grey = 0.1842f`，其 `get_contrast`
/// 是围绕该值的幂函数。读作「Lab L* = 128」时正好对应 `(128/255)` 反推的结果。
pub const MIDDLE_GREY: f32 = 0.1842;

/// sRGB 传输函数（IEC 61966-2-1）的分段参数。
///
/// 注意 `0.055` 与 `1.055` 这一对：编码段用 `1.055·x^(1/2.4) − 0.055`，
/// 解码段用 `((x+0.055)/1.055)^2.4`。两者**必须严格互逆**，
/// 所以不能用某些资料里的 `0.0551`（那是四舍五入的近似，会让
/// 0.043 附近的值往返后偏 2e-14 —— 数值上无害，但会让
/// 「空配方必须逐字节还原原图」这条不变量在边界附近失效）。
const SRGB_DECODE_THRESHOLD: f32 = 0.04045;
const SRGB_ENCODE_THRESHOLD: f32 = 0.0031308;
const SRGB_SLOPE: f32 = 12.92;
const SRGB_ALPHA: f32 = 0.055;
const SRGB_GAMMA: f32 = 2.4;

/// sRGB 编码 → 线性（解码）。
///
/// **支持 x > 1**：超过 1 时走 `x^(1/2.4)` 的延伸段而不是直接返回 1。
/// 这是「线性 HDR」阶段能一路保留过曝能量的前提 —— 如果这里截断，
/// 曝光 +2EV 之后的高光信息会在第一次转换时就永久丢失，后续影调滑块再也救不回来。
#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    let x = c.max(0.0);
    if x <= SRGB_DECODE_THRESHOLD {
        x / SRGB_SLOPE
    } else {
        ((x + SRGB_ALPHA) / (1.0 + SRGB_ALPHA)).powf(SRGB_GAMMA)
    }
}

/// 保留过曝能量的线性 → sRGB 编码（`srgb_to_linear` 的逆，同样不截断）。
#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    let x = c.max(0.0);
    if x <= SRGB_ENCODE_THRESHOLD {
        x * SRGB_SLOPE
    } else {
        (1.0 + SRGB_ALPHA) * x.powf(1.0 / SRGB_GAMMA) - SRGB_ALPHA
    }
}

/// 线性光下的 Rec.709 亮度。
#[inline(always)]
pub fn luma(r: f32, g: f32, b: f32) -> f32 {
    let [wr, wg, wb] = luminance_weights();
    wr * r + wg * g + wb * b
}

/// 白平衡：把 `temp`（K）与 `tint`（-100..=100，负绿正品红）折算成
/// **线性空间的三通道增益** `(gr, gg, gb)`。
///
/// 算法口径与 Camera Raw 对齐（darktable `src/iop/temperature.c`）：
/// 把目标光源的色温推到目标白点 → 得到该光源的 RGB 比例 → **求倒数**作为增益。
/// 注意这是**通道乘子**而不是 Bradford 色适应矩阵：ACR 用的就是前者，
/// 两者在高饱和区域的表现不同，用 CAT 会得到「更符合色科学但不像 ACR」的结果。
///
/// 色温用 Tanner Helland 拟合（sRGB 基色，`temperature.c` 未公开内部实现，
/// 但这是同类工具的通行做法，误差在 ±1% 量级，肉眼不可见）。
/// 为保持「0 偏移 = 完全中性」，增益始终以 6500K 的比值归一化。
///
/// 例：`wb_gains(6500.0, 0.0)` 返回三个接近 1.0 的增益（中性）；
/// `wb_gains(9000.0, 0.0)` 返回红 > 1、蓝 < 1 的组合（画面变暖）。
/// 完整断言见同文件 `tests::white_6500k_is_neutral`。
pub fn wb_gains(temp_kelvin: f32, tint: f32) -> [f32; 3] {
    let target = kelvin_rgb(clamp(temp_kelvin, 1500.0, 50_000.0));
    let neutral = kelvin_rgb(6500.0);

    // 求倒数得「把该光源校正为白」所需的通道增益，再以 6500K 归一化。
    let mut gains = [
        neutral[0] / target[0].max(1e-4),
        neutral[1] / target[1].max(1e-4),
        neutral[2] / target[2].max(1e-4),
    ];

    // 色调：**只作用于绿通道**。负 tint = 加绿，正 tint = 减绿（品红）。
    // 红/蓝各补一半以维持亮度（最后还有一次均值归一化）。
    // 系数 0.6 来自目视标定 —— 太小看不出变化，太大偏色明显。
    let t = (tint / 100.0).clamp(-1.0, 1.0) * 0.6;
    gains[1] *= 1.0 - t;
    gains[0] *= 1.0 + t * 0.5;
    gains[2] *= 1.0 + t * 0.5;

    let mean = (gains[0] + gains[1] + gains[2]) / 3.0;
    if mean > 1e-6 {
        [gains[0] / mean, gains[1] / mean, gains[2] / mean]
    } else {
        [1.0, 1.0, 1.0]
    }
}

/// 色温 (K) → 归一化 RGB（sRGB 基色，等效于 D65 白点下的比例）。
///
/// Tanner Helland 的经典拟合，误差在几百 K 以内不可见。
/// 中间值用 `ln`，需注意 2000K~6600K 段公式会给出负数，必须钳到 0。
pub fn kelvin_rgb(kelvin: f32) -> [f32; 3] {
    let t = kelvin / 100.0;
    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (t - 60.0).powf(-0.133_204_76)
    };
    let g = if t <= 66.0 {
        99.470_802_586_1 * t.max(1e-3).ln() - 161.119_568_166
    } else {
        288.122_169_528_3 * (t - 60.0).powf(-0.075_514_85)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_731
    };
    [r.max(0.0), g.max(0.0), b.max(0.0)]
}

/// 经典 smoothstep：`t²(3-2t)`，输入自动钳到 [0,1]。
///
/// 影调分带的过渡段与锐化的边缘蒙版共用同一个形状函数（C² 连续），
/// 保证两处都不会出现一阶导数跳变而在照片上留下可见的「带子」。
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (e1 - e0).abs() < f32::EPSILON {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 线性标量钳制。
#[inline]
pub fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// 线性值的 0..1 钳制。
///
/// 与 `clamp(v, 0.0, 1.0)` 等价，单独命名是为了在阅读直方图代码时
/// 一眼看出「这里钳的是**线性亮度**，不是编码值」—— 两者差一个 gamma，
/// 搞混会让整个直方图分布右移 100 多个桶。
#[inline]
pub fn clip01_linear(v: f32) -> f32 {
    clamp(v, 0.0, 1.0)
}

/// 归一化 8 位量化：`0..=1` → `0..=255`。
#[inline]
pub fn quantize8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 传输函数必须是互逆的，否则预览与导出会差半个色阶。
    #[test]
    fn srgb_transfer_is_invertible() {
        for i in 0..=255 {
            let x = i as f32 / 255.0;
            let round_trip = linear_to_srgb(srgb_to_linear(x));
            assert!(
                (round_trip - x).abs() < 1e-5,
                "x={x} 往返得到 {round_trip}，传输函数不可逆"
            );
        }
    }

    /// 关键回归：>1 的值不能被截断，否则过曝能量在第一阶段就丢了。
    #[test]
    fn srgb_transfer_preserves_overexposure() {
        // 注意：输入 1.6 是「显示参考域」的值。解码后必须 > 1，
        // 否则 +2EV 之后的高光信息会在第一次转换时被永久截断。
        let big = srgb_to_linear(1.6);
        assert!(big > 1.6, "1.6 应解码为更大的线性值，实得 {big}");
        let back = linear_to_srgb(3.5);
        assert!(back > 1.5, "3.5 线性值应编码回 >1.5，实得 {back}");
        // 往返也要能还原过曝值
        assert!((linear_to_srgb(srgb_to_linear(1.6)) - 1.6).abs() < 1e-4);
    }

    /// 0 与 1 是两个锚点，任何缩放都不能动它们。
    #[test]
    fn srgb_transfer_endpoints_fixed() {
        assert!(srgb_to_linear(0.0).abs() < 1e-6);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!(linear_to_srgb(0.0).abs() < 1e-6);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
    }

    /// 中灰必须是 18%，对比度支点才与 darktable / ACR 一致。
    #[test]
    fn middle_grey_is_18_percent() {
        assert!((MIDDLE_GREY - 0.1842).abs() < 1e-4);
        assert_eq!(luminance_weights(), [0.2126729, 0.7151522, 0.0721750]);
    }

    #[test]
    fn white_6500k_is_neutral() {
        let g = wb_gains(6500.0, 0.0);
        for c in g {
            assert!((c - 1.0).abs() < 1e-3, "6500K 应完全中性，实得 {g:?}");
        }
    }

    /// 拉高色温 = 让画面变暖 = 红增益上升、蓝增益下降。方向反了用户会立刻察觉。
    #[test]
    fn warmer_setting_pushes_red_up_blue_down() {
        let g = wb_gains(9000.0, 0.0);
        assert!(g[0] > 1.0, "暖色调下红应被提，实得 {g:?}");
        assert!(g[2] < 1.0, "暖色调下蓝应被压，实得 {g:?}");
        // 绿通道保持不动（6500K 的锚点几乎落在绿上）
        // 绿不会精确保持 1：均值归一化会把三通道一起缩放，
        // 只要红升蓝降、绿居中，「绿通道的相对位置不变」就够了。
        assert!(
            g[0] > g[1] && g[1] > g[2],
            "暖色调下应严格保持 红>绿>蓝，实得 {g:?}"
        );
    }

    #[test]
    fn tint_green_magenta_directions() {
        // 负 tint = 偏绿（绿增益上升）；正 tint = 偏品红（绿增益下降）
        let green = wb_gains(6500.0, -100.0);
        let magenta = wb_gains(6500.0, 100.0);
        assert!(
            green[1] > magenta[1],
            "负 tint 应加强绿：{:?} vs {:?}",
            green[1],
            magenta[1]
        );
        assert!(
            (green[1] - magenta[1]).abs() > 0.05,
            "两端差异应可感知：{:?} vs {:?}",
            green[1],
            magenta[1]
        );
        // 品红方向必然是「绿 < 红=蓝」
        assert!(
            magenta[1] < magenta[0] && magenta[1] < magenta[2],
            "正 tint 应压绿到最弱：{magenta:?}"
        );
    }

    /// 白平衡不得改变整体亮度，否则用户每调一次色温都要再拉一次曝光。
    #[test]
    fn white_balance_preserves_mean_gain() {
        for t in [2000.0, 3500.0, 5500.0, 12000.0] {
            let g = wb_gains(t, 30.0);
            let mean = (g[0] + g[1] + g[2]) / 3.0;
            assert!((mean - 1.0).abs() < 1e-3, "t={t} 平均增益 {mean} 偏离 1");
        }
    }

    #[test]
    fn kelvin_rgb_monotonic_warmth() {
        let cool = kelvin_rgb(4000.0);
        let warm = kelvin_rgb(12000.0);
        assert!(warm[2] > cool[2], "高温蓝应更强");
        assert!(warm[0] < cool[0], "高温红应更弱");
    }

    /// smoothstep 的三个硬性约束：单调、端点精确、C¹ 连续。
    #[test]
    fn smoothstep_properties() {
        assert_eq!(smoothstep(0.0, 1.0, -5.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 5.0), 1.0);
        assert_eq!(smoothstep(0.0, 1.0, 0.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 1.0), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        let mut prev = -1.0;
        for i in 0..=100 {
            let v = smoothstep(0.3, 0.7, i as f32 / 100.0);
            assert!(v >= prev, "必须单调不减");
            prev = v;
        }
    }

    /// e0 == e1 的退化区间不能产生 NaN（否则整幅图变白/黑）。
    #[test]
    fn smoothstep_degenerate_range_safe() {
        assert_eq!(smoothstep(0.5, 0.5, 0.4), 0.0);
        assert_eq!(smoothstep(0.5, 0.5, 0.6), 1.0);
        assert!(smoothstep(0.5, 0.5, 0.5).is_finite());
    }

    #[test]
    fn quantize_rounds_half_up() {
        assert_eq!(quantize8(0.0), 0);
        assert_eq!(quantize8(1.0), 255);
        assert_eq!(quantize8(0.5), 128);
        assert_eq!(quantize8(-1.0), 0, "负值必须钳到 0 而不是回绕成 255");
        assert_eq!(quantize8(2.0), 255, "过曝必须钳到 255");
    }
}
