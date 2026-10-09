//! 饱和度与自然饱和度（Vibrance）
//! ======================================================================
//! 两个滑块看起来重复，实际解决的是不同问题：
//!
//! - **饱和度**：三个通道统一外扩 → **已经很艳的照片会「爆」**。
//!   蓝天变纯蓝、日落变纯橙，大面积色块尤其明显。
//! - **自然饱和度**：按像素**已有的艳度反比加权** —— 越接近中性灰
//!   的像素动得越多，已经很艳的像素几乎不动。人像肤色因此不会变成「橙子」。
//!
//! ## 精确公式
//!
//! 采用 darktable `data/kernels/basicadj.cl:154-161` 的实现：
//!
//! ```c
//! const float average = (pixel.x + pixel.y + pixel.z) / 3;
//! const float delta   = fast_length(pixel - average);   // 色度半径
//! const float P       = vibrance * (1 - pow(delta, |vibrance|));
//! pixel = average + (saturation + P) * (pixel - average);
//! ```
//!
//! 关键结构是 `avg + k·(pixel - avg)`：把像素沿「灰点 → 像素」这条射线
//! 缩放。k > 1 增艳、k < 1 减艳，且**k 为标量**保证色相不变。
//!
//! `src/iop/basicadj.c` 中的标定：`saturation = slider + 1`、
//! `vibrance = slider / 1.4`。除以 1.4 是为了让自然饱和度的可用范围
//! 比理论值更大 —— 1.4 的因子让 ±1 的滑块对应约 ±1.4 的色度倍率，
//! 实际使用中「+50」已经相当明显。
//!
//! ## 为什么不直接用 HSL 的 S 通道
//!
//! HSL 的 S = (max-min)/max，在 max→0 时发散；而且 HSL 旋转色相、
//! RGB↔HSL 是每像素三角函数与分支，纯开销。色度半径法无分支、连续、
//! 且天然与亮度解耦，在 6000×4000 全分辨率下比 HSL 快一个量级。

use super::color::{clamp, luma, luminance_weights};
use super::recipe::ColorAdjustments;

/// 自然饱和度的标定因子（darktable: `vibrance / 1.4`）。
const VIBRANCE_SCALE: f32 = 1.4 / 1.0;

/// 预计算好的饱和度参数（逐像素只做算术，无 pow）。
#[derive(Debug, Clone, Copy)]
pub struct SaturationParams {
    /// 饱和度滑块 `-1..=1` → 倍率 `0..=2`
    pub saturation_mul: f32,
    /// 自然饱和度 `-1..=1` → `vibrance` 标定值
    pub vibrance: f32,
    /// 两项都中性时可整段跳过
    pub active: bool,
}

impl SaturationParams {
    #[inline]
    pub fn from(a: &ColorAdjustments) -> Self {
        let active = a.vibrance != 0.0 || a.saturation != 0.0;
        Self {
            saturation_mul: a.saturation + 1.0,
            // 与 darktable `basicadj.cl:157` 的 `vibrance` 同口径（含 1.4 标定）。
            // 逐像素按 `v * (1 - delta^|v|)` 计算，**不能把符号提前吸收进 v** ——
            // 那样 `delta=0` 时 `delta^|v|` 恒为 0，公式会变成 `1 - 0 = 1`，
            // 中性灰就永远不动（「自然饱和度 +100 作用在灰调照片上完全无效」）。
            vibrance: a.vibrance / 1.4,
            active,
        }
    }

    #[inline(always)]
    pub fn is_noop(&self) -> bool {
        !self.active
    }

    /// 逐像素应用。输入输出都是**显示参考**（0..1 但可 >1）值。
    #[inline(always)]
    pub fn apply(&self, rgb: &mut [f32; 3]) {
        // 三个通道的平均值 = 灰点
        let avg = (rgb[0] + rgb[1] + rgb[2]) / 3.0;

        let mut k = self.saturation_mul;

        if self.vibrance != 0.0 {
            // 色度半径 ‖rgb - avg‖，L2 范数
            let dr = rgb[0] - avg;
            let dg = rgb[1] - avg;
            let db = rgb[2] - avg;
            let delta = (dr * dr + dg * dg + db * db).sqrt().clamp(0.0, 1.0);
            // 指数取 |vibrance|（恒正），保证 delta=0 时结果是 0 而不是 0^0=1。
            let p = self.vibrance * (1.0 - delta.powf(self.vibrance.abs()));
            k += p;
        }

        if k == 1.0 {
            return;
        }
        rgb[0] = avg + k * (rgb[0] - avg);
        rgb[1] = avg + k * (rgb[1] - avg);
        rgb[2] = avg + k * (rgb[2] - avg);
    }
}

/// 色度半径（用于 UI 显示「当前饱和度」，也便于单测）。
#[inline]
pub fn chroma_radius(rgb: [f32; 3]) -> f32 {
    let avg = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    let dr = rgb[0] - avg;
    let dg = rgb[1] - avg;
    let db = rgb[2] - avg;
    (dr * dr + dg * dg + db * db).sqrt()
}

/// 保色相的最终裁剪（Lightroom 非 HDR 的默认做法）。
///
/// ACES / OCIO 称之为 `maxRGB` 或 `AVG` 裁剪：若三通道最大值超过上限，
/// 用**同一个倍率**把三通道压回 —— 色相与三通道比例严格不变。
///
/// 对比「逐通道 clamp」：后者会独立截断每个通道，红 `(2.0, 0.5, 0.5)`
/// 逐通道得 `(1.0, 0.5, 0.5)`（变暗红），maxRGB 得 `(1.0, 0.25, 0.25)`
/// —— 后者才是用户期望的「只是过亮了」。
#[inline(always)]
pub fn clip_preserving_hue(rgb: &mut [f32; 3]) {
    let m = rgb[0].max(rgb[1]).max(rgb[2]);
    if m <= 1.0 {
        // 同时处理负值（欠饱和/曲线后可能出现负通道）。
        // 必须**先平移再缩放**：`v·k` 只缩不平时负值仍是负的，
        // 量化成 u8 时会变成黑洞。`(v - min)/(1 - min)` 把 min 映到 0、max 映到 1，
        // 且是仿射变换，三通道比例不变。
        let min = rgb[0].min(rgb[1]).min(rgb[2]);
        if min < 0.0 {
            let denom = 1.0 - min;
            rgb[0] = (rgb[0] - min) / denom;
            rgb[1] = (rgb[1] - min) / denom;
            rgb[2] = (rgb[2] - min) / denom;
        }
        return;
    }
    let k = 1.0 / m;
    rgb[0] *= k;
    rgb[1] *= k;
    rgb[2] *= k;
}

/// 供前端直方图使用的亮度量化（0..=255 桶）。
#[inline]
pub fn luma_bucket(r: f32, g: f32, b: f32) -> usize {
    let y = clamp(luma(r, g, b), 0.0, 1.0);
    let [wr, wg, wb] = luminance_weights();
    let _ = (wr, wg, wb);
    (y * 255.0 + 0.5) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neutral() -> ColorAdjustments {
        ColorAdjustments::default()
    }

    // ---------- 基本性质 ----------

    #[test]
    fn neutral_params_are_noop() {
        assert!(SaturationParams::from(&neutral()).is_noop());
    }

    #[test]
    fn neutral_params_leave_pixels_bit_identical() {
        let p = SaturationParams::from(&neutral());
        let mut rgb = [0.31f32, 0.62, 0.44];
        let before = rgb;
        p.apply(&mut rgb);
        assert_eq!(rgb, before);
    }

    // ---------- 饱和度 ----------

    #[test]
    fn saturation_plus_increases_chroma() {
        let p = SaturationParams::from(&ColorAdjustments {
            saturation: 0.5,
            vibrance: 0.0,
        });
        let before = chroma_radius([0.6, 0.45, 0.3]);
        let mut rgb = [0.6f32, 0.45, 0.3];
        p.apply(&mut rgb);
        assert!(chroma_radius(rgb) > before, "饱和度应增艳");
    }

    #[test]
    fn saturation_minus_desaturates() {
        let p = SaturationParams::from(&ColorAdjustments {
            saturation: -1.0,
            vibrance: 0.0,
        });
        let mut rgb = [0.8f32, 0.3, 0.2];
        p.apply(&mut rgb);
        assert!(
            rgb.iter().all(|v| (v - 0.4333).abs() < 0.02),
            "饱和度 -100 应完全去色，三通道 {rgb:?}"
        );
    }

    /// 饱和度 -100 的精确结果：三通道都等于灰点值。
    #[test]
    fn saturation_minus_one_lands_on_grey() {
        let p = SaturationParams::from(&ColorAdjustments {
            saturation: -1.0,
            vibrance: 0.0,
        });
        let mut rgb = [0.9f32, 0.1, 0.55];
        p.apply(&mut rgb);
        let avg = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
        for v in rgb {
            assert!((v - avg).abs() < 1e-5, "未完全去色：{rgb:?}");
        }
    }

    /// 任何饱和度设置都**不得改变色相**。
    ///
    /// 正确的判据不是「r/g 不变」—— 沿灰点共线缩放本来就会改变 r/g
    /// （灰点不是坐标原点）。判据是「三通道相对灰点的位移是同一个标量倍」，
    /// 即像素仍落在「灰点 → 原像素」这条射线上。
    #[test]
    fn saturation_preserves_hue() {
        for s in [-0.9, -0.3, 0.4, 1.0] {
            let p = SaturationParams::from(&ColorAdjustments {
                saturation: s,
                vibrance: 0.0,
            });
            let base = [0.75f32, 0.42, 0.18];
            let mut rgb = base;
            p.apply(&mut rgb);
            let avg0 = (base[0] + base[1] + base[2]) / 3.0;
            let avg1 = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
            let d0 = [base[0] - avg0, base[1] - avg0, base[2] - avg0];
            let d1 = [rgb[0] - avg1, rgb[1] - avg1, rgb[2] - avg1];
            // 判据是 d1[i] / d0[i] 三个比值相等（都等于倍率 k）
            let k = [d1[0] / d0[0], d1[1] / d0[1], d1[2] / d0[2]];
            assert!(
                (k[0] - k[1]).abs() < 1e-4 && (k[1] - k[2]).abs() < 1e-4,
                "s={s} 偏离灰点射线：{base:?} → {rgb:?}（倍率 {k:?}）"
            );
        }
    }

    // ---------- 自然饱和度 ----------

    /// 自然饱和度的核心承诺：**已艳的少动，接近中性灰的多动**。
    ///
    /// ⚠️ 判据必须是**倍率**，不是绝对增量。
    /// 艳色本来彩度就大，哪怕倍率低，绝对增量也可能更大
    /// （实测 1.129 倍 × 彩度 0.756 = +0.098，比 1.66 倍 × 0.028 = +0.019 还多）。
    /// 用户感知到的「动得多/少」对应的正是倍率，所以只能比倍率。
    #[test]
    fn vibrance_moves_muted_more_than_vivid() {
        let p = SaturationParams::from(&ColorAdjustments {
            saturation: 0.0,
            vibrance: 1.0,
        });

        let gain = |base: [f32; 3]| {
            let before = chroma_radius(base);
            let mut rgb = base;
            p.apply(&mut rgb);
            chroma_radius(rgb) / before
        };

        let muted = gain([0.50, 0.52, 0.48]);
        let vivid = gain([1.00, 0.10, 0.05]);
        assert!(
            muted > vivid * 1.4,
            "低彩度倍率 {muted:.3} 应显著高于艳色 {vivid:.3}"
        );
        assert!(
            muted > 1.4 && vivid < 1.2,
            "倍率应在两端之间收敛：muted={muted:.3} vivid={vivid:.3}"
        );
    }

    /// 彩度趋于 0 时，自然饱和度的倍率必须趋于其理论上限 `1 + v/1.4`。
    ///
    /// 这条测试守的是一个非常隐蔽的 bug：若把公式写成
    /// `1 - delta.powf(vibrance)`（**没有绝对值**），则在 `delta = 0`
    /// 且 `vibrance > 0` 时得到 `1 - 0 = 1`，倍率变成 0 —— 恰好相反，
    /// 「自然饱和度 +100」会把所有弱彩度像素**洗成纯灰**。
    /// 用正负两个方向的 slider 一起夹住，才不会只测到一半。
    #[test]
    fn vibrance_gain_limit_at_zero_chroma() {
        for slider in [1.0f32, -1.0] {
            let p = SaturationParams::from(&ColorAdjustments {
                saturation: 0.0,
                vibrance: slider,
            });
            let base = [0.50f32, 0.502, 0.498];
            let mut rgb = base;
            p.apply(&mut rgb);
            let gain = chroma_radius(rgb) / chroma_radius(base);
            let v = slider / 1.4;
            let want = 1.0 + v;
            assert!(
                (gain - want).abs() < 0.05,
                "slider={slider} 零彩度处倍率 {gain}，应趋近 {want}"
            );
        }
    }

    /// 纯中性灰**不可能**被加艳 —— 它没有色相可供放大。
    /// 这不是 bug，而是「灰点共线缩放」公式的定义结果
    /// （位移为零，任何倍率都不改变它）。把它固化为测试，
    /// 以免将来有人误以为这是缺陷而去「修」它。
    #[test]
    fn pure_grey_is_mathematically_unchanged() {
        let p = SaturationParams::from(&ColorAdjustments {
            saturation: 1.0,
            vibrance: 1.0,
        });
        let mut rgb = [0.5f32, 0.5, 0.5];
        p.apply(&mut rgb);
        assert_eq!(rgb, [0.5, 0.5, 0.5]);
    }

    #[test]
    fn negative_vibrance_desaturates() {
        let p = SaturationParams::from(&ColorAdjustments {
            saturation: 0.0,
            vibrance: -1.0,
        });
        let before = chroma_radius([0.8, 0.4, 0.2]);
        let mut rgb = [0.8f32, 0.4, 0.2];
        p.apply(&mut rgb);
        assert!(chroma_radius(rgb) < before, "自然饱和度 -100 应减艳");
    }

    #[test]
    fn vibrance_preserves_hue_too() {
        let p = SaturationParams::from(&ColorAdjustments {
            saturation: 0.0,
            vibrance: 0.8,
        });
        let base = [0.7f32, 0.35, 0.2];
        let mut rgb = base;
        p.apply(&mut rgb);
        let avg0 = (base[0] + base[1] + base[2]) / 3.0;
        let avg1 = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
        let ratio_before = (base[1] - avg0) / (base[0] - avg0);
        let ratio_after = (rgb[1] - avg1) / (rgb[0] - avg1);
        assert!(
            (ratio_before - ratio_after).abs() < 1e-3,
            "自然饱和度改变了色相 {ratio_before} → {ratio_after}"
        );
    }

    #[test]
    fn vibrance_never_produces_nan() {
        let p = SaturationParams::from(&ColorAdjustments {
            saturation: 1.0,
            vibrance: -1.0,
        });
        for seed in [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [2.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
        ] {
            let mut rgb = seed;
            p.apply(&mut rgb);
            assert!(rgb.iter().all(|v| v.is_finite()), "{seed:?} → {rgb:?}");
        }
    }

    /// 超界输入（过曝像素）不得让 `powf` 溢出。
    #[test]
    fn vibrance_handles_out_of_domain_chroma() {
        let p = SaturationParams::from(&ColorAdjustments {
            saturation: 0.0,
            vibrance: 1.0,
        });
        let mut rgb = [5.0f32, -2.0, 9.0];
        p.apply(&mut rgb);
        assert!(rgb.iter().all(|v| v.is_finite()), "{rgb:?}");
    }

    // ---------- 保色相裁剪 ----------

    /// 裁剪必须等比缩放 —— 逐通道截断是常见的「高光变脏」根因。
    #[test]
    fn hue_preserving_clip_scales_equally() {
        let mut rgb = [2.0f32, 0.5, 0.5];
        clip_preserving_hue(&mut rgb);
        let r0 = rgb[0] / rgb[1];
        let r1 = 2.0f32 / 0.5;
        assert!((r0 - r1).abs() < 1e-5, "裁剪破坏了通道比例：{r0} vs {r1}");
        assert!((rgb[0] - 1.0).abs() < 1e-5, "最大通道应恰好落到 1.0");
    }

    #[test]
    fn hue_preserving_clip_passes_through_valid_range() {
        let mut rgb = [0.5f32, 0.4, 0.3];
        let before = rgb;
        clip_preserving_hue(&mut rgb);
        assert_eq!(rgb, before);
    }

    /// 欠饱和区域可能出现负通道，裁剪必须一并处理（否则负值量化成 0 变黑洞）。
    #[test]
    fn hue_preserving_clip_handles_negative_channels() {
        let mut rgb = [0.5f32, -0.2, 0.1];
        clip_preserving_hue(&mut rgb);
        assert!(rgb.iter().all(|v| *v >= -1e-6), "负通道未被抬起：{rgb:?}");
        // 全负的像素也必须被完整抬进 [0,1]，不能只缩放
        let mut all_neg = [-1.0f32, -2.0, -3.0];
        clip_preserving_hue(&mut all_neg);
        assert!(
            all_neg.iter().all(|v| (-1e-6..=1.0 + 1e-6).contains(v)),
            "全负像素未被抬起：{all_neg:?}"
        );
    }

    #[test]
    fn clip_brings_all_pixels_into_unit_range() {
        for seed in [
            [3.0f32, 2.0, 1.0],
            [-1.0f32, -2.0, -3.0],
            [0.5f32, -0.9, 0.1],
            [1.0f32, 1.0, 1.0],
        ] {
            let mut rgb = seed;
            clip_preserving_hue(&mut rgb);
            assert!(
                rgb.iter().all(|v| (-1e-5..=1.0 + 1e-5).contains(v)),
                "{seed:?} → {rgb:?}"
            );
        }
    }

    #[test]
    fn luma_bucket_covers_full_range() {
        assert_eq!(luma_bucket(0.0, 0.0, 0.0), 0);
        assert_eq!(luma_bucket(1.0, 1.0, 1.0), 255);
        assert_eq!(luma_bucket(2.0, 2.0, 2.0), 255, "过曝应钳到 255");
        let mid = luma_bucket(0.5, 0.5, 0.5);
        assert!((125..=130).contains(&mid), "中灰桶 {mid} 偏离预期");
    }
}
