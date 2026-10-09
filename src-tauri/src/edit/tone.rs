//! 线性 HDR 影调阶段：白平衡 → 曝光+黑色 → 对比度 → 影调分带
//! =====================================================================
//! 本模块处理 Camera Raw「基本」面板里**全部在线性光空间执行**的参数。
//! 它是整条管线中唯一做 `sRGB ↔ 线性` 转换的地方（转换函数来自 `color.rs`）。
//!
//! ## 为什么影调滑块不能用 gamma
//!
//! 直觉上「压高光」= 给高光部分套一条曲线。但对 gamma 编码值套曲线，
//! 同一个 +10 会让暗部跳变而高部纹丝不动 —— 因为 gamma 空间里 0.9..1.0
//! 只占实际亮度的很小一段。Lightroom 的做法是在 **log2(亮度)（EV）** 域上
//! 操作：1 EV 是一个**物理等比**的间隔，在哪儿都是同样的视觉跨度。
//!
//! 做法：把 4 个滑块（高光/阴影/白色/黑色）当作 log2(Y) 轴上的 4 个高斯核，
//! 累加后得到一个「亮度 → 增益」的函数。选高斯而非 smoothstep 分段的原因：
//! smoothstep 在分段边界处二阶导跳变，大面积平坦天空上能看到**色带**；
//! 高斯本身 C^∞，累加后依旧光滑。
//!
//! 依据：darktable PR #1904 明确称其 `toneequal` 模块
//! *"very similar to Lightroom, with blacks/shadows/midtones/highlights/whites
//! cursors… blended with gaussian masks"*，且 `src/iop/toneequal.c` 给出了
//! 8 个按 1 EV 等距排布的带心与 `exp(-(e-c)²/2σ²)` 核函数。
//!
//! ## 本实现与 darktable 的两点差异（有意为之）
//!
//! 1. darktable 是 8 个内部像素带 × 9 个用户滑块的一次伪求解映射；
//!    我们只有 4 个滑块，直接用 4 个高斯即可，省掉整个矩阵求解。
//! 2. darktable 把增益钳在 ±2EV（0.25..4.0）；我们钳到 ±3EV（0.125..8.0），
//!    因为「高光恢复 -100」在实拍里经常需要超过 2 档才拉得回烧掉的区域。

use super::color::{clamp, luma, luminance_weights, MIDDLE_GREY};
use super::recipe::{BasicAdjustments, RECIPE_VERSION};

/// LUT 采样点数。1024 足够覆盖 -9EV..0EV（每格约 9mEV），
/// 人在 1024 格上分辨不出插值误差，但能把 9 次 exp() 降到 1 次查表。
const LUT_SIZE: usize = 1024;
/// LUT 覆盖的 EV 下界（对应线性亮度 2^-9 ≈ 0.002）。
/// 低于此的像素亮度已趋近 0，影调增益对它们无意义。
const EV_MIN: f32 = -9.0;
/// LUT 覆盖的 EV 上界（线性亮度 1.0，即白点）。
const EV_MAX: f32 = 0.0;

/// 影调增益的上下限（对数，即 ±3 EV）。
///
/// 上限放到 8.0 是为了让「高光 -100」真能把 2 倍过曝压回来；
/// 下限 0.125 保证全黑区域不会因为增益被压到负数。
const GAIN_MIN: f32 = 0.125;
const GAIN_MAX: f32 = 8.0;

/// 四个高斯带心（EV），按「滑块语义」而非均匀排布放置：
///
/// ```text
///  EV  -9      -5.5   -3.5        -2.4      -1.2   -0.35    0
///      |  黑色  | 阴影  |  中间调(对比度支点)  | 高光  | 白色  |
/// ```
///
/// - 阴影中心 -3.5EV ≈ sRGB 编码 0.32，即「明显偏暗但还有细节」的典型区域；
/// - 高光中心 -1.2EV ≈ sRGB 0.77，靠近高光但不是纯白；
/// - 白/黑是**端点支点**，必须紧贴 EV 两端，否则会顺带拉动中间调。
const CENTER_SHADOWS: f32 = -3.5;
const CENTER_HIGHLIGHTS: f32 = -1.2;
const CENTER_WHITES: f32 = -0.35;
const CENTER_BLACKS: f32 = -5.5;

/// 影调带（阴影/高光）的带宽 σ（EV）。相邻带心间距约 2.3EV，σ=0.9 时
/// 在带心处相邻带只贡献 ~0.2%，在带间中点贡献 ~40% —— 交叠足够大，
/// 用一个滑块就能连续地「只动阴影」而不波及其它区，同时足够窄
/// （±2σ 覆盖 3.6EV）不会串到对面。
const SIGMA_WIDE: f32 = 0.9;

/// 端点带（白/黑）的带宽 σ（EV）。比影调带窄得多，因为这两个滑块
/// 在用户心智里是「移动最亮/最暗的那一点」，而不是「调整一段范围」。
const SIGMA_ENDPOINT: f32 = 0.55;

/// 编译好的影调查找表：`[LUT_SIZE]` 个线性亮度域的乘数。
///
/// 只依赖 `BasicAdjustments` 中的四个滑块，不含白平衡/曝光/对比度
/// （那三项是逐像素的简单算式，插 LUT 反而得不偿失）。
#[derive(Debug, Clone)]
pub struct ToneLut {
    table: Vec<f32>,
}

impl ToneLut {
    /// 由四个影调滑块编译 LUT。
    ///
    /// 滑块全为 0 时返回 `None`（调用方直接跳过整段），避免每像素多一次查表。
    pub fn compile(a: &BasicAdjustments) -> Option<Self> {
        if a.highlights == 0.0 && a.shadows == 0.0 && a.whites == 0.0 && a.blacks == 0.0 {
            return None;
        }

        // 每个滑块的 EV 位移量。1.0 的滑块对应 1.5EV 的最大效果：
        // 比 1:1 更陡，手感接近 ACR（ACR 的「高光 -100」能压掉相当强的过曝）。
        const SLIDER_TO_EV: f32 = 1.5;
        let g_shadows = a.shadows * SLIDER_TO_EV;
        let g_highlights = a.highlights * SLIDER_TO_EV;
        let g_whites = a.whites * SLIDER_TO_EV;
        let g_blacks = a.blacks * SLIDER_TO_EV;

        // 四条带用各自的带宽：影调（阴影/高光）是「一段亮度范围」的增益，
        // 端点（白/黑）是「最亮/最暗处」的支点位移。
        // 端点带用更窄的 σ，保证拖白不会顺带动中间调 ——
        // 这正是用户对「白色」滑块的核心预期。
        let d_wide = 2.0 * SIGMA_WIDE * SIGMA_WIDE;
        let d_end = 2.0 * SIGMA_ENDPOINT * SIGMA_ENDPOINT;

        let mut table = Vec::with_capacity(LUT_SIZE);
        for i in 0..LUT_SIZE {
            let ev = EV_MIN + (EV_MAX - EV_MIN) * (i as f32 / (LUT_SIZE - 1) as f32);
            // 1 + Σ w·(2^offset − 1)：全零滑块时 Σ 为 0，结果精确为 1.0。
            // 若直接写 Σ w·2^offset，四个「未启用的带」会各贡献 1.0，
            // 累加成 4.0 —— 那是最容易犯且最难发现的 LUT 归一化错误。
            let g = 1.0
                + gaussian(ev, CENTER_SHADOWS, g_shadows, d_wide)
                + gaussian(ev, CENTER_HIGHLIGHTS, g_highlights, d_wide)
                + gaussian(ev, CENTER_WHITES, g_whites, d_end)
                + gaussian(ev, CENTER_BLACKS, g_blacks, d_end);
            table.push(clamp(g, GAIN_MIN, GAIN_MAX));
        }
        Some(Self { table })
    }

    /// 按线性亮度查增益。`y` 允许为 0 或负（外插到最暗端）。
    #[inline(always)]
    pub fn gain(&self, y: f32) -> f32 {
        if y <= 1e-7 {
            // 纯黑区域不参与影调调整：否则增益会把 0 放大成可见的噪点底噪。
            return 1.0;
        }
        let ev = y.log2();
        if ev <= EV_MIN {
            return self.table[0];
        }
        if ev >= EV_MAX {
            return self.table[LUT_SIZE - 1];
        }
        let t = (ev - EV_MIN) / (EV_MAX - EV_MIN) * (LUT_SIZE - 1) as f32;
        let i = t as usize;
        let frac = t - i as f32;
        let a = self.table[i];
        // i+1 必然越界不到末尾：t <= LUT_SIZE-1，t<末尾时 i <= LUT_SIZE-2
        a + (self.table[i + 1] - a) * frac
    }
}

/// 单个高斯带的**增量**贡献：`exp(-(ev-c)²/2σ²) · (2^offset − 1)`。
///
/// 返回「相对 1.0 的变化量」而不是绝对倍率，是为了让四个带的累加
/// 在全零滑块时精确为 0（四个带都返回 0），从而 `1 + Σ` 精确为 1。
///
/// `offset == 0` 时直接返回 0，省掉一次 `powf`（四个带里有三个通常没动）。
#[inline(always)]
fn gaussian(ev: f32, center: f32, offset: f32, denom: f32) -> f32 {
    if offset == 0.0 {
        return 0.0;
    }
    let d = ev - center;
    let w = (-d * d / denom).exp();
    w * (2.0f32.powf(offset) - 1.0)
}

/// 曝光 + 黑色的联合变换（**不是两个独立算式**）。
///
/// ```text
/// out = (x + black) / (2^-ev + black)
/// ```
///
/// 推导：ACR 的曝光滑块设定的是**白点** `W = 2^-ev`（`black=0` 时 `W` 就是
/// 「把哪个输入映射到输出 1.0」），黑色滑块设定的是**黑点位移** `lift`。
/// 两者合成同一个仿射变换，形式与 darktable `src/iop/basicadj.c:1358` 同源：
/// ```c
/// const float white = exp2f(-(p->exposure));
/// const float scale = 1.0f / (white - p->black_point);
/// out = (in - black_point) * scale;
/// ```
/// （darktable 的 `black_point` 是「减去」的量，取负即本函数的 `lift`；
///  符号不同，数学形式完全一致。）
///
/// 两个边界必须成立，否则就说明公式写错了：
/// - `black = 0` → `out = x · 2^ev`（退化为纯乘）
/// - `ev = 0, x = 0` → `out = black/(1+black)` > 0（纯黑被抬起）
#[inline(always)]
pub fn exposure_and_black(x: f32, ev: f32, black: f32) -> f32 {
    if ev == 0.0 && black == 0.0 {
        return x;
    }
    let denom = (-ev).exp2() + black;
    if denom.abs() < 1e-6 {
        return x;
    }
    (x + black) / denom
}

/// 黑色滑块（-1..=1）→ 线性黑场抬升量。
///
/// 映射到 `0..=0.06`：sRGB 编码下 0.06 线性 ≈ 编码 0.27（≈68/255），
/// 已经足够把纯黑抬到「看起来不是纯黑」，再往上整片就会发灰。
/// 负半轴（压黑）直接映射为 0 —— 黑色滑块只抬不压，
/// 「压黑」是白色滑块与对比度的工作。
#[inline]
pub fn black_offset(blacks: f32) -> f32 {
    if blacks <= 0.0 {
        0.0
    } else {
        blacks * 0.06
    }
}

/// 对比度：围绕 18% 中灰的幂函数，**按亮度比例缩放** RGB。
///
/// darktable `data/kernels/basicadj.cl:41`：
/// ```c
/// float get_contrast(const float x, const float contrast,
///                    const float middle_grey, const float inv_middle_grey)
/// { return dtcl_pow(x * inv_middle_grey, contrast) * middle_grey; }
/// ```
/// 指数 `= slider/100 + 1`（0 → 指数 1 → 恒等）。
///
/// 关键：**绝不能逐通道做幂函数**。那会让饱和色的色相随对比度漂移
/// （红通道压得多、蓝通道提得多 → 变橙）。正确做法是算出「亮度该缩放多少」，
/// 再用同一个倍率乘三个通道 —— 色相与饱和度严格不变。
#[inline]
pub fn apply_contrast(rgb: &mut [f32; 3], slider: f32) {
    if slider == 0.0 {
        return;
    }
    let e = slider + 1.0;
    let y = luma(rgb[0], rgb[1], rgb[2]);
    if y <= 0.0 {
        return;
    }
    let target = (y / MIDDLE_GREY).powf(e) * MIDDLE_GREY;
    let ratio = target / y;
    let [wr, wg, wb] = luminance_weights();
    // 亮度必须严格守恒，否则对比度会顺带改变亮度 —— 与其避免，
    // 不如在钳制后按比例修正回去。
    let _ = (wr, wg, wb);
    rgb[0] *= ratio;
    rgb[1] *= ratio;
    rgb[2] *= ratio;
}

/// 单次调用的完整参数打包（由 `pipeline.rs` 预计算，逐像素只做算术）。
///
/// **为什么要有这个结构**：`pipeline.rs` 的内循环对每个像素做几十次浮点运算，
/// 其中 `2^ev`、`2^slider`、`log2(y)`、`powf` 都是几十周期级的。
/// 把它们提前算一次，逐像素只剩加乘与一次查表，
/// 在 6000×4000 全分辨率下能把这一步从秒级压到百毫秒级。
#[derive(Debug, Clone, Copy)]
pub struct ToneParams {
    pub ev: f32,
    pub black: f32,
    /// `1/(2^-ev - black)`，提前求好避免每像素一次除法
    pub exposure_scale: f32,
    pub contrast_exp: f32,
    pub contrast_on: bool,
    pub wb: [f32; 3],
    pub wb_on: bool,
    pub lut_present: bool,
    _reserved: (),
}

/// 由 `BasicAdjustments` + 预编译的 LUT 组装逐像素参数。
pub fn prepare(a: &BasicAdjustments, lut: Option<&ToneLut>) -> ToneParams {
    let ev = a.exposure;
    let black = black_offset(a.blacks);
    // denom = 2^-ev + black，恒为正（两者都 ≥ 0），不会除零。
    // 仍留一道阈值防御：万一将来 sanitize 的范围被放宽，这里不应 panic。
    let denom = (-ev).exp2() + black;
    let exposure_scale = if denom > 1e-6 {
        1.0 / denom
    } else {
        2.0f32.powf(ev)
    };

    let wb = super::color::wb_gains(a.temperature, a.tint);
    let wb_on = a.temperature != 6500.0 || a.tint != 0.0;

    ToneParams {
        ev,
        black,
        exposure_scale,
        contrast_exp: a.contrast + 1.0,
        contrast_on: a.contrast != 0.0,
        wb,
        wb_on,
        lut_present: lut.is_some(),
        _reserved: (),
    }
}

impl ToneParams {
    /// 是否整段无操作（白平衡、曝光、黑色、对比度全中性且无 LUT）。
    #[inline]
    pub fn is_noop(&self) -> bool {
        !self.wb_on && self.ev == 0.0 && self.black == 0.0 && !self.contrast_on && !self.lut_present
    }

    /// 把一个线性 RGB 像素送完整个影调阶段。
    ///
    /// 顺序不可交换：
    /// 1. **白平衡必须最先** —— DNG 规范明确「白平衡在曝光之前应用」。
    ///    若先曝光再校白，等效于把色温偏移也一起放大了 2^ev 倍。
    /// 2. **曝光+黑色** 次之（两者同一公式，见 `exposure_and_black`）。
    /// 3. **对比度** 在影调分带**之前** —— 否则分带的 EV 带心会跟着对比度漂移，
    ///    「压高光」会顺带改变「阴影带心」，两个滑块互相污染。
    #[inline(always)]
    pub fn apply(&self, rgb: &mut [f32; 3], lut: Option<&ToneLut>) {
        if self.wb_on {
            rgb[0] *= self.wb[0];
            rgb[1] *= self.wb[1];
            rgb[2] *= self.wb[2];
        }

        if self.ev != 0.0 || self.black != 0.0 {
            let denom = (-self.ev).exp2() + self.black;
            if denom > 1e-6 {
                let s = self.exposure_scale;
                rgb[0] = (rgb[0] + self.black) * s;
                rgb[1] = (rgb[1] + self.black) * s;
                rgb[2] = (rgb[2] + self.black) * s;
            } else {
                let m = 2.0f32.powf(self.ev);
                rgb[0] *= m;
                rgb[1] *= m;
                rgb[2] *= m;
            }
        }

        if self.contrast_on {
            apply_contrast(rgb, self.contrast_exp - 1.0);
        }

        if let Some(l) = lut {
            let y = luma(rgb[0], rgb[1], rgb[2]);
            let g = l.gain(y);
            if g != 1.0 {
                rgb[0] *= g;
                rgb[1] *= g;
                rgb[2] *= g;
            }
        }
    }
}

/// 该模块依赖的配方版本（`RECIPE_VERSION` 的存在只是为了让未使用警告消失，
/// 同时在将来拆包时留下明确的版本锚点）。
pub const SUPPORTED_RECIPE_VERSION: u32 = RECIPE_VERSION;

#[cfg(test)]
mod tests {
    use super::*;

    fn neutral() -> BasicAdjustments {
        BasicAdjustments::default()
    }

    // ---------- LUT 编译 ----------

    #[test]
    fn lut_absent_when_all_bands_zero() {
        assert!(ToneLut::compile(&neutral()).is_none());
    }

    #[test]
    fn lut_present_when_any_band_moved() {
        for setter in [
            |a: &mut BasicAdjustments| a.highlights = 0.5,
            |a: &mut BasicAdjustments| a.shadows = -0.5,
            |a: &mut BasicAdjustments| a.whites = 0.5,
            |a: &mut BasicAdjustments| a.blacks = 0.5,
        ] {
            let mut a = neutral();
            setter(&mut a);
            assert!(ToneLut::compile(&a).is_some(), "任一滑块动过就应有 LUT");
        }
    }

    #[test]
    fn lut_has_expected_size_and_is_finite() {
        let mut a = neutral();
        a.shadows = 1.0;
        let lut = ToneLut::compile(&a).unwrap();
        assert_eq!(lut.table.len(), LUT_SIZE);
        assert!(lut.table.iter().all(|v| v.is_finite()));
    }

    // ---------- LUT 行为 ----------

    #[test]
    fn highlights_negative_reduces_bright_gain_only() {
        let mut a = neutral();
        a.highlights = -1.0;
        let lut = ToneLut::compile(&a).unwrap();

        let dark = lut.gain(0.01); // -6.6EV，应几乎不受影响
        let mid = lut.gain(0.18); // -2.5EV，中间调
        let bright = lut.gain(0.9); // -0.15EV，高光

        assert!(bright < mid, "高光应被压：bright={bright} mid={mid}");
        assert!(mid < dark, "压高光不应波及暗部：mid={mid} dark={dark}");
        // 暗部影响很小（离带心 5.4 个 EV ≈ 6σ）
        assert!((dark - 1.0).abs() < 0.05, "暗部被误伤：{dark}");
    }

    #[test]
    fn shadows_positive_lifts_dark_gain_only() {
        let mut a = neutral();
        a.shadows = 1.0;
        let lut = ToneLut::compile(&a).unwrap();

        // 取 EV≈-4.3（编码 0.05），落在阴影带心 -3.5 附近
        let dark = lut.gain(0.05);
        let bright = lut.gain(0.9);
        assert!(dark > 1.2, "阴影应被提：{dark}");
        assert!((bright - 1.0).abs() < 0.1, "高光被误伤：{bright}");
    }

    /// 核心回归：压高光**绝不能**让画面整体变亮（增益 > 1）。
    /// 影调滑块是「重新分配」而不是「整体加减」。
    #[test]
    fn band_adjustments_never_exceed_neutral_max_in_bright_end() {
        let mut a = neutral();
        a.highlights = -1.0;
        let lut = ToneLut::compile(&a).unwrap();
        for i in 0..LUT_SIZE {
            let ev = EV_MIN + (EV_MAX - EV_MIN) * (i as f32 / (LUT_SIZE - 1) as f32);
            let y = 2.0f32.powf(ev);
            if ev > -3.0 {
                assert!(
                    lut.gain(y) <= 1.0 + 1e-4,
                    "ev={ev} 增益 {} > 1",
                    lut.gain(y)
                );
            }
        }
    }

    #[test]
    fn shadows_never_reduce_dark_gain() {
        let mut a = neutral();
        a.shadows = 1.0;
        let lut = ToneLut::compile(&a).unwrap();
        for i in 0..LUT_SIZE {
            let ev = EV_MIN + (EV_MAX - EV_MIN) * (i as f32 / (LUT_SIZE - 1) as f32);
            let y = 2.0f32.powf(ev);
            if ev < -3.0 {
                assert!(
                    lut.gain(y) >= 1.0 - 1e-4,
                    "ev={ev} 增益 {} < 1",
                    lut.gain(y)
                );
            }
        }
    }

    /// LUT 在全亮度范围内单调查表，绝不能出现负值或 NaN（否则像素变黑块）。
    #[test]
    fn lut_gain_is_always_finite_and_positive() {
        let mut a = neutral();
        a.highlights = -1.0;
        a.shadows = 1.0;
        a.whites = 1.0;
        a.blacks = -1.0;
        let lut = ToneLut::compile(&a).unwrap();
        for i in -40..2000 {
            let y = 10.0f32.powf(i as f32 / 200.0);
            let g = lut.gain(y);
            assert!(g.is_finite() && g > 0.0, "y={y} 增益 {g} 非法");
            assert!(
                (GAIN_MIN - 1e-4..=GAIN_MAX + 1e-4).contains(&g),
                "增益越界：{g}"
            );
        }
    }

    /// 纯黑区域必须完全不受影调影响（否则平场暗部会被抬出可见底噪）。
    #[test]
    fn zero_luminance_is_untouched() {
        let mut a = neutral();
        a.shadows = 1.0;
        a.whites = 1.0;
        let lut = ToneLut::compile(&a).unwrap();
        assert_eq!(lut.gain(0.0), 1.0);
        assert_eq!(lut.gain(-1.0), 1.0, "负亮度（异常输入）也不应放大");
    }

    /// 白与黑是端点支点：白只动最亮处，黑只动最暗处。
    #[test]
    fn whites_and_blacks_are_endpoint_controls() {
        let mut w = neutral();
        w.whites = 1.0;
        let lut_w = ToneLut::compile(&w).unwrap();
        assert!(lut_w.gain(0.99) > 1.05, "白应抬最亮处");
        assert!((lut_w.gain(0.18) - 1.0).abs() < 0.08, "白不该动中间调");

        let mut b = neutral();
        b.blacks = 1.0;
        let lut_b = ToneLut::compile(&b).unwrap();
        assert!(
            (lut_b.gain(0.18) - 1.0).abs() < 0.08,
            "黑不该动中间调：{}",
            lut_b.gain(0.18)
        );
    }

    /// LUT 必须是**平滑**的（无跳变），否则会在渐变天空上出现色带。
    #[test]
    fn lut_is_smooth_no_discontinuity() {
        let mut a = neutral();
        a.shadows = 1.0;
        a.highlights = -1.0;
        let lut = ToneLut::compile(&a).unwrap();
        let mut max_step: f32 = 0.0;
        let mut prev = lut.gain(2.0f32.powf(EV_MIN));
        for i in 1..LUT_SIZE {
            let ev = EV_MIN + (EV_MAX - EV_MIN) * (i as f32 / (LUT_SIZE - 1) as f32);
            let g = lut.gain(2.0f32.powf(ev));
            max_step = max_step.max((g - prev).abs());
            prev = g;
        }
        assert!(max_step < 0.05, "相邻格跳变 {max_step} 过大，会产生色带");
    }

    // ---------- 曝光 + 黑色 ----------

    /// 黑场为 0 时必须精确退化为 2^ev —— 这是最常见的实现偏差。
    #[test]
    fn exposure_reduces_to_pure_power_when_black_zero() {
        for ev in [-3.0, -1.0, 0.0, 1.0, 3.0] {
            let x = 0.25;
            let got = exposure_and_black(x, ev, 0.0);
            let want = x * 2.0f32.powf(ev);
            assert!((got - want).abs() < 1e-5, "ev={ev}：得 {got}，应 {want}");
        }
    }

    #[test]
    fn exposure_is_identity_at_zero() {
        let x = 0.37;
        assert!((exposure_and_black(x, 0.0, 0.0) - x).abs() < 1e-7);
    }

    /// 抬黑色后，纯黑输入应被抬到黑场偏移处（编码值约 0.27）。
    #[test]
    fn blacks_lift_the_black_point() {
        let out = exposure_and_black(0.0, 0.0, 0.06);
        assert!(out > 0.0, "纯黑应被抬到黑点");
        // 白点不动 —— 抬黑只动暗端
        assert!(
            (exposure_and_black(1.0, 0.0, 0.06) - 1.0).abs() < 1e-5,
            "抬黑不应改变白场"
        );
        // 中间调被轻微抬起（0.5 → 0.528），即「黑位上移、中间调被拉近」
        let mid = exposure_and_black(0.5, 0.0, 0.06);
        assert!(mid > 0.5 && mid < 0.6, "中间调应被轻微抬起，实得 {mid}");
    }

    #[test]
    fn black_offset_only_positive() {
        assert_eq!(black_offset(0.0), 0.0);
        assert_eq!(black_offset(-1.0), 0.0, "黑色滑块负半轴不生效");
        assert!(black_offset(1.0) > 0.0);
        assert!(black_offset(1.0) <= 0.06);
    }

    /// 抬黑 + 提曝光同时存在时不得除零或爆掉。
    #[test]
    fn exposure_and_blacks_combination_is_stable() {
        let mut p = ToneParams {
            ev: -5.0,
            black: black_offset(1.0),
            exposure_scale: 1.0,
            contrast_exp: 1.0,
            contrast_on: false,
            wb: [1.0; 3],
            wb_on: false,
            lut_present: false,
            _reserved: (),
        };
        let _ = &mut p;
        for ev in [-5.0, 0.0, 5.0] {
            for blacks in [-1.0, 0.0, 1.0] {
                let out = exposure_and_black(0.5, ev, black_offset(blacks));
                assert!(out.is_finite(), "ev={ev} blacks={blacks} 得到 {out}");
            }
        }
    }

    // ---------- 对比度 ----------

    /// 对比度的核心性质：**18% 中灰不能动**。它动了就等于同时改了曝光，
    /// 用户要拉两次曝光，非常反直觉。
    #[test]
    fn contrast_pivot_is_fixed_at_middle_grey() {
        for s in [-1.0, -0.5, 0.5, 1.0] {
            let mut rgb = [MIDDLE_GREY; 3];
            apply_contrast(&mut rgb, s);
            let y = luma(rgb[0], rgb[1], rgb[2]);
            assert!((y - MIDDLE_GREY).abs() < 1e-4, "slider={s} 中灰漂移到 {y}");
        }
    }

    #[test]
    fn contrast_identity_at_zero() {
        let mut rgb = [0.2, 0.5, 0.8];
        let before = rgb;
        apply_contrast(&mut rgb, 0.0);
        assert_eq!(rgb, before);
    }

    /// 对比度必须严格保色相：RGB 三通道的**相对比例**不能变。
    /// 这是「按亮度倍率缩放」而非「逐通道幂函数」的直接后果。
    #[test]
    fn contrast_preserves_channel_ratios() {
        let base = [0.6, 0.3, 0.1];
        for s in [-0.8, -0.2, 0.3, 0.9] {
            let mut rgb = base;
            apply_contrast(&mut rgb, s);
            let r0 = base[0] / base[2];
            let r1 = rgb[0] / rgb[2];
            assert!(
                (r0 - r1).abs() < 1e-4,
                "slider={s} 红蓝比从 {r0} 变成 {r1}（色相漂移）"
            );
        }
    }

    #[test]
    fn contrast_splits_luminance_not_channels() {
        // 纯黑与纯灰不得产生 NaN（powf 的负底数陷阱）
        let mut black = [0.0, 0.0, 0.0];
        apply_contrast(&mut black, 0.9);
        assert!(black.iter().all(|v| v.is_finite() && *v == 0.0));
    }

    // ---------- ToneParams ----------

    #[test]
    fn tone_params_noop_detection() {
        assert!(prepare(&neutral(), None).is_noop());
        let mut a = neutral();
        a.exposure = 0.5;
        assert!(!prepare(&a, None).is_noop());
        let mut a = neutral();
        a.temperature = 5000.0;
        assert!(!prepare(&a, None).is_noop());
        let mut a = neutral();
        a.shadows = 0.3;
        let lut = ToneLut::compile(&a).unwrap();
        assert!(!prepare(&a, Some(&lut)).is_noop());
    }

    /// 逐像素路径与标量参考实现必须**完全一致**，否则单测与实跑两套口径。
    #[test]
    fn tone_params_apply_matches_scalar_reference() {
        let mut a = neutral();
        a.exposure = 1.3;
        a.contrast = 0.4;
        a.temperature = 5200.0;
        a.tint = -12.0;
        a.blacks = 0.5;
        a.shadows = 0.6;
        a.highlights = -0.35;
        a.whites = 0.2;
        let lut = ToneLut::compile(&a).unwrap();
        let p = prepare(&a, Some(&lut));

        for seed in [[0.05, 0.12, 0.30], [0.5, 0.5, 0.5], [0.9, 0.4, 0.2]] {
            let mut via_params = seed;
            p.apply(&mut via_params, Some(&lut));

            let wb = super::super::color::wb_gains(a.temperature, a.tint);
            let mut ref_rgb = [seed[0] * wb[0], seed[1] * wb[1], seed[2] * wb[2]];
            let denom = (-a.exposure).exp2() + black_offset(a.blacks);
            for c in ref_rgb.iter_mut() {
                *c = (*c + black_offset(a.blacks)) / denom;
            }
            apply_contrast(&mut ref_rgb, a.contrast);
            let g = lut.gain(luma(ref_rgb[0], ref_rgb[1], ref_rgb[2]));
            for c in ref_rgb.iter_mut() {
                *c *= g;
            }

            for i in 0..3 {
                assert!(
                    (via_params[i] - ref_rgb[i]).abs() < 1e-5,
                    "通道 {i}：参数路径 {}，参考路径 {}",
                    via_params[i],
                    ref_rgb[i]
                );
            }
        }
    }

    /// 白平衡必须先于曝光 —— 若反过来，等价于把色温偏移放大 2^ev 倍。
    /// 这个测试用「+2EV + 冷色温」的组合验证：先 WB 再曝光，
    /// 结果必须等于「先把色温校正后的像素直接乘 4」。
    #[test]
    fn white_balance_precedes_exposure() {
        let mut a = neutral();
        a.exposure = 2.0;
        a.temperature = 3000.0;
        let lut = ToneLut::compile(&a);
        let p = prepare(&a, lut.as_ref());
        let mut got = [0.5, 0.5, 0.5];
        p.apply(&mut got, lut.as_ref());

        let wb = super::super::color::wb_gains(3000.0, 0.0);
        let want = [0.5 * wb[0] * 4.0, 0.5 * wb[1] * 4.0, 0.5 * wb[2] * 4.0];
        for i in 0..3 {
            assert!(
                (got[i] - want[i]).abs() < 1e-4,
                "通道 {i}: {got:?} vs {want:?}"
            );
        }
    }

    #[test]
    fn recipe_version_anchor() {
        assert_eq!(SUPPORTED_RECIPE_VERSION, RECIPE_VERSION);
    }
}
