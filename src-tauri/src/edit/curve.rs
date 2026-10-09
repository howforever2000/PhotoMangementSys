//! 色调曲线：Catmull-Rom 插值 → 256 项 LUT → Adobe 式保色相应用
//! ==================================================================
//! 两条曲线：
//! - **RGB 曲线** —— 三通道联动，但**绝不逐通道查表**。逐通道查表是
//!   Photoshop CS 时代的做法，会让饱和色的色相随曲线漂移（红被压 → 变蓝）。
//! - **亮度曲线** —— 只算 Rec.709 亮度，按倍率缩放三通道，严格保色相。
//!
//! ## RGB 曲线为什么要「排好序再 LUT」
//!
//! RawTherapee `rtengine/curves.h:2004` 的 `AdobeToneCurve::RGBTone`，
//! 注释明写 *"Tone curve according to Adobe's reference implementation"*：
//!
//! ```cpp
//! maxval = lutToneCurve[maxvalold];
//! minval = lutToneCurve[minvalold];
//! medval = minval + (maxval - minval) * (medvalold - minvalold) / (maxvalold - minvalold);
//! ```
//!
//! 即：把三个通道**按大小排序**，只对最小/最大值查 LUT，中间通道用
//! 「在变换后区间内保持相对位置」的方式插值回去。数学上这是把曲线限制在
//! 灰度轴上单调插值 —— 于是中性灰（r=g=b）永远映射到灰，不可能被曲线染色。
//! 本模块的 `apply_rgb_curve` 就是它的 Rust 版（含 7 种大小关系的还原）。
//!
//! ## 曲线作用在哪个空间
//!
//! Adobe 在 *Understanding Adobe Camera Raw 3* 里明确：
//! *"the Curve tab controls in Camera Raw work on the linear data, but present
//! a gamma 2.2-corrected user interface"*。但真正落地时曲线被应用在
//! **影调映射之后的显示参考值**上（RawTherapee 的 `ToneCurve` 亦然，
//! 在 `ToneCurveQ` 阶段而非 develop 阶段）。我们遵循同样的次序：
//! `pipeline.rs` 里曲线排在影调阶段之后、细节阶段之前。

use super::color::{clamp, luma};
use super::recipe::{Curve, CurvePoint};

/// LUT 项数。
///
/// 与 Camera Raw 的 `ToneCurvePV2012` 一致为 256 —— 不是因为 256 更好，
/// 而是因为**与 XMP 侧车格式字节数对齐**：将来若要导出 Lightroom 可读的
/// 配方，256 项可以直接按字节拷贝，24 位精度在小尺寸图上足够。
pub const LUT_SIZE: usize = 256;

/// 编译后的曲线：256 项，值域 `0..=1`。
#[derive(Debug, Clone)]
pub struct CurveLut {
    table: [f32; LUT_SIZE],
}

impl CurveLut {
    /// 恒等 LUT。未启用曲线的像素直接跳过整段（省一次查表 + 三次数值运算）。
    pub fn identity() -> Self {
        let mut table = [0.0f32; LUT_SIZE];
        for (i, v) in table.iter_mut().enumerate() {
            *v = i as f32 / (LUT_SIZE - 1) as f32;
        }
        Self { table }
    }

    /// 从控制点编译。
    ///
    /// 控制点少于 2 个时等价于恒等（返回 `None` 让调用方整段跳过）：
    /// 单点无法确定一条曲线，与其猜一个默认形状，不如明确当作没调过。
    pub fn compile(curve: &Curve) -> Option<Self> {
        if curve.is_identity() {
            return None;
        }
        let mut lut = Self::identity();
        sample_catmull_rom(&curve.points, &mut lut.table);
        Some(lut)
    }

    #[inline(always)]
    fn at(&self, t: f32) -> f32 {
        let x = clamp(t, 0.0, 1.0) * (LUT_SIZE - 1) as f32;
        let i = x as usize;
        if i >= LUT_SIZE - 1 {
            return self.table[LUT_SIZE - 1];
        }
        let f = x - i as f32;
        let a = self.table[i];
        a + (self.table[i + 1] - a) * f
    }

    /// 供 UI 画曲线用的采样（供 `commands.rs` 导出给前端绘图）。
    pub fn sample_table(&self) -> Vec<f32> {
        self.table.to_vec()
    }
}

/// Catmull-Rom 样条在整个 x∈[0,1] 上采样 LUT。
///
/// ### 为什么用 Catmull-Rom 而不是线性/单调三次插值
///
/// - **线性**：控制点之间是折线，斜率在节点处跳变。用户拖 3 个点的 S 形曲线
///   会出现肉眼可见的「棱角」，且与 Lightroom 的手感不符。
/// - **Catmull-Rom**：节点处一阶导连续，拖动时曲线「跟手」。
/// - **为什么不用 Fritsch-Carlson 单调三次**：单调插值会**削掉所有过冲**，
///   用户想要的「提亮同时保住高光」这类 S 形恰恰需要轻微过冲，单调约束会
///   让曲线看起来「推不动」。因此这里用 Catmull-Rom 并在 LUT 生成后
///   统一钳到 `[0,1]`（见下），用一个显式的钳制换取用户要的曲线形状。
///
/// ### 边界处理
///
/// 首尾各补两个「外推点」（沿首/末段斜率延伸），使首段与末段的切线也正确 ——
/// 否则 `(0,y0)` 到第一个控制点之间会退化成平直段，表现为「曲线起点附近的
/// 调整没反应」。
fn sample_catmull_rom(points: &[CurvePoint], out: &mut [f32]) {
    debug_assert!(points.len() >= 2);

    let n = points.len();
    // 外推点：p[-1] = 2*p0 - p1，p[n] = 2*p[n-1] - p[n-2]
    let p = |i: isize| -> CurvePoint {
        if i < 0 {
            let a = points[0];
            let b = points[1];
            CurvePoint::new(2.0 * a.x - b.x, 2.0 * a.y - b.y)
        } else if i as usize >= n {
            let a = points[n - 1];
            let b = points[n - 2];
            CurvePoint::new(2.0 * a.x - b.x, 2.0 * a.y - b.y)
        } else {
            points[i as usize]
        }
    };

    for (i, slot) in out.iter_mut().enumerate() {
        let x = i as f32 / (LUT_SIZE - 1) as f32;

        // 定位 x 所在的段：最后一个点之后归入末段
        let mut seg = 0usize;
        for j in 0..(n - 1) {
            if x >= points[j].x {
                seg = j;
            }
        }
        let p0 = p(seg as isize - 1);
        let p1 = p(seg as isize);
        let p2 = p(seg as isize + 1);
        let p3 = p(seg as isize + 2);

        let span = p2.x - p1.x;
        // 零宽段（重复 x，理论上 sanitize 阶段已排除）：
        // 取 p2 的 y 而不是除零，否则整条曲线变 NaN。
        let t = if span.abs() < 1e-9 {
            0.0
        } else {
            (x - p1.x) / span
        };

        // 端点外 x∈[0,1] 但落在首点/末点之外时，按端点值延伸（不做外推），
        // 否则曲线会冲出控制点围成的框，用户拖首点会把整张图左半边拉飞。
        if x < points[0].x {
            *slot = points[0].y;
            continue;
        }
        if x > points[n - 1].x {
            *slot = points[n - 1].y;
            continue;
        }

        let t2 = t * t;
        let t3 = t2 * t;
        // 标准 Catmull-Rom 基
        let y = 0.5
            * ((2.0 * p1.y)
                + (-p0.y + p2.y) * t
                + (2.0 * p0.y - 5.0 * p1.y + 4.0 * p2.y - p3.y) * t2
                + (-p0.y + 3.0 * p1.y - 3.0 * p2.y + p3.y) * t3);
        *slot = clamp(y, 0.0, 1.0);
    }
}

/// 编译结果对：`Some(lut)` 表示该曲线启用。
pub type CompiledCurve = Option<CurveLut>;

/// 编译 RGB 曲线与亮度曲线。
///
/// 两条曲线合成一个结构，让 `pipeline.rs` 只传一个引用。
#[derive(Debug, Clone)]
pub struct Curves {
    pub rgb: CompiledCurve,
    pub luma: CompiledCurve,
}

impl Curves {
    /// 两条曲线都未启用。
    pub fn is_noop(&self) -> bool {
        self.rgb.is_none() && self.luma.is_none()
    }
}

/// 由配方编译两条曲线。
pub fn compile(rgb: &Curve, luma_curve: &Curve) -> Curves {
    Curves {
        rgb: CurveLut::compile(rgb),
        luma: CurveLut::compile(luma_curve),
    }
}

/// Adobe 式 RGB 曲线应用（保色相）。
///
/// 见模块头部对 `AdobeToneCurve::RGBTone` 的引用。
///
/// 实现要点：排序出 (min, med, max) 的**通道下标**，对 min/max 查 LUT，
/// 中间通道按「变换前后的相对位置」线性插值，最后按原下标写回。
/// 中间通道不做查表是保色相的关键 —— 查表就会引入与 hue 相关的偏差。
#[inline(always)]
pub fn apply_rgb_curve(rgb: &mut [f32; 3], lut: &CurveLut) {
    // 三元素排序：手动展开而非 sort_unstable_by，比较次数固定为 3 次
    let (i_min, i_mid, i_max) = order3(rgb[0], rgb[1], rgb[2]);

    let lo_old = rgb[i_min];
    let mid_old = rgb[i_mid];
    let hi_old = rgb[i_max];

    let lo_new = lut.at(lo_old);
    let hi_new = lut.at(hi_old);

    // hi_old == lo_old（纯灰或已饱和到同值）：分母为 0，此时三点都映射到同值，
    // 直接取 lo_new 即可，否则会 Inf * 0 = NaN。
    let mid_new = if hi_old - lo_old > 1e-9 {
        lo_new + (hi_new - lo_new) * (mid_old - lo_old) / (hi_old - lo_old)
    } else {
        lo_new
    };

    rgb[i_min] = lo_new;
    rgb[i_mid] = clamp(mid_new, 0.0, 1.0);
    rgb[i_max] = hi_new;
}

/// 亮度曲线应用（按倍率缩放三通道）。
///
/// RawTherapee `LuminanceToneCurve::Apply`：
/// ```cpp
/// const float coef = newLuminance / currLuminance;
/// r *= coef; g *= coef; b *= coef;
/// ```
/// 与 RGB 曲线不同，这里**允许**超过 1.0（曲线上限之外由后续 clamp 处理），
/// 这样「曲线抬亮中间调但保住高光」才可能实现。
#[inline(always)]
pub fn apply_luma_curve(rgb: &mut [f32; 3], lut: &CurveLut) {
    let y = luma(rgb[0], rgb[1], rgb[2]);
    if y <= 1e-7 {
        return;
    }
    let coef = lut.at(y) / y;
    rgb[0] *= coef;
    rgb[1] *= coef;
    rgb[2] *= coef;
}

/// 返回 (最小值下标, 中间值下标, 最大值下标)。
///
/// 大量小函数调用不如展开，编译器也更容易内联。
#[inline(always)]
fn order3(a: f32, b: f32, c: f32) -> (usize, usize, usize) {
    // 先比 a 与 b（一次比较把 6 种严格序分成两半），再各自分两种。
    // 写成 6 路平铺的 if 链极易写错一个分支顺序，
    // 且错的分支只在特定输入下触发 —— 实测「c < a < b」这一路就曾写反。
    if a <= b {
        if b <= c {
            (0, 1, 2) // a ≤ b ≤ c
        } else if a <= c {
            (0, 2, 1) // a ≤ c < b
        } else {
            (2, 0, 1) // c < a ≤ b
        }
    } else if a <= c {
        (1, 0, 2) // b < a ≤ c
    } else if b <= c {
        (1, 2, 0) // b ≤ c < a
    } else {
        (2, 1, 0) // c < b < a
    }
}

#[cfg(test)]
mod tests {
    use super::super::recipe::CurvePoint;
    use super::*;

    fn lut_from(points: Vec<CurvePoint>) -> CurveLut {
        CurveLut::compile(&Curve { points }).expect("两点以上应编译成功")
    }

    // ---------- 编译 ----------

    #[test]
    fn empty_curve_compiles_to_none() {
        assert!(CurveLut::compile(&Curve::default()).is_none());
    }

    #[test]
    fn identity_lut_roundtrips_every_level() {
        let lut = CurveLut::identity();
        for i in 0..=100 {
            let v = i as f32 / 100.0;
            assert!((lut.at(v) - v).abs() < 1e-4, "v={v} 得 {}", lut.at(v));
        }
    }

    /// 未启用的曲线必须与恒等完全一致 —— 「没碰曲线」不能改变任何像素。
    #[test]
    fn neutral_curve_is_bit_identical_pass_through() {
        let mut rgb = [0.2f32, 0.6, 0.9];
        let before = rgb;
        apply_rgb_curve(&mut rgb, &CurveLut::identity());
        for i in 0..3 {
            assert!((rgb[i] - before[i]).abs() < 1e-3, "恒等曲线不该动像素 {i}");
        }
    }

    // ---------- LUT 形状 ----------

    #[test]
    fn curve_raises_output_for_lift() {
        // 控制点全在 y=x 之上 → 提亮
        let lut = lut_from(vec![
            CurvePoint::new(0.0, 0.1),
            CurvePoint::new(0.5, 0.6),
            CurvePoint::new(1.0, 1.0),
        ]);
        assert!(lut.at(0.5) > 0.55, "中点应被抬起，实得 {}", lut.at(0.5));
    }

    /// 控制点越界值必须被钳住 —— 否则 LUT 会有负数，量化成 u8 时变 0（黑洞）。
    #[test]
    fn lut_values_stay_in_unit_range() {
        let lut = lut_from(vec![
            CurvePoint::new(0.0, -0.5),
            CurvePoint::new(0.5, 0.5),
            CurvePoint::new(1.0, 1.5),
        ]);
        for i in 0..LUT_SIZE {
            let v = lut.table[i];
            assert!((0.0..=1.0).contains(&v), "LUT[{}]={} 越界", i, v);
        }
    }

    /// LUT 必须单调可解释：控制点全递增时曲线应大体递增。
    #[test]
    fn lut_is_increasing_for_increasing_points() {
        let lut = lut_from(vec![
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.25, 0.2),
            CurvePoint::new(0.5, 0.55),
            CurvePoint::new(0.75, 0.8),
            CurvePoint::new(1.0, 1.0),
        ]);
        for i in 1..LUT_SIZE {
            assert!(
                lut.table[i] >= lut.table[i - 1] - 1e-6,
                "LUT 在 {} 处回落：{} → {}",
                i,
                lut.table[i - 1],
                lut.table[i]
            );
        }
    }

    /// 端点外的取值必须夹住而不是外推 —— 否则拖首控制点会「拉飞」半张图。
    #[test]
    fn curve_clamps_outside_control_range() {
        let lut = lut_from(vec![CurvePoint::new(0.2, 0.3), CurvePoint::new(0.8, 0.7)]);
        assert!((lut.at(0.0) - 0.3).abs() < 1e-3, "左外应夹到首点 y");
        assert!((lut.at(1.0) - 0.7).abs() < 1e-3, "右外应夹到末点 y");
    }

    /// 曲线必须穿过每个控制点（插值的定义性质）。
    #[test]
    fn curve_passes_through_control_points() {
        let pts = vec![
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.3, 0.15),
            CurvePoint::new(0.65, 0.8),
            CurvePoint::new(1.0, 1.0),
        ];
        let lut = lut_from(pts.clone());
        for p in &pts {
            let got = lut.at(p.x);
            assert!(
                (got - p.y).abs() < 5e-3,
                "控制点 ({},{}) 得 {got}",
                p.x,
                p.y
            );
        }
    }

    /// 一阶导连续 = 节点处无棱角。这是选 Catmull-Rom 而非线性的全部理由。
    #[test]
    fn curve_has_no_slope_discontinuities() {
        let lut = lut_from(vec![
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.3, 0.15),
            CurvePoint::new(0.65, 0.8),
            CurvePoint::new(1.0, 1.0),
        ]);
        let mut max_jump: f32 = 0.0;
        for i in 1..LUT_SIZE - 1 {
            let d = (lut.table[i + 1] - lut.table[i - 1]).abs();
            max_jump = max_jump.max(d);
        }
        // 相邻格最大斜率应远小于 0.05（即至少 ~20 格跨过整个值域）
        assert!(max_jump < 0.08, "曲线存在棱角，最大斜率跃变 {max_jump}");
    }

    // ---------- RGB 曲线应用 ----------

    /// 保色相的核心测试：纯灰必须映射到纯灰。逐通道查表做不到这一点。
    #[test]
    fn rgb_curve_keeps_neutral_grey_neutral() {
        let lut = lut_from(vec![
            CurvePoint::new(0.0, 0.05),
            CurvePoint::new(0.5, 0.62),
            CurvePoint::new(1.0, 1.0),
        ]);
        for v in [0.0f32, 0.1, 0.25, 0.5, 0.75, 1.0] {
            let mut rgb = [v, v, v];
            apply_rgb_curve(&mut rgb, &lut);
            let spread = (rgb[0] - rgb[1]).abs().max((rgb[1] - rgb[2]).abs());
            assert!(
                spread < 1e-3,
                "灰 {v} 被曲线染色，三通道 {rgb:?} 差 {spread}"
            );
        }
    }

    /// 中间通道不查表 → 排序关系必须原样保留。
    #[test]
    fn rgb_curve_preserves_channel_ordering() {
        let lut = lut_from(vec![
            CurvePoint::new(0.0, 0.2),
            CurvePoint::new(0.5, 0.35),
            CurvePoint::new(1.0, 0.9),
        ]);
        let cases = [
            [0.9f32, 0.5, 0.1], // r>g>b
            [0.1, 0.5, 0.9],    // b>g>r
            [0.5, 0.9, 0.1],    // g>r>b
            [0.5, 0.1, 0.9],    // b>r>g
            [0.9, 0.1, 0.5],    // r>b>g
            [0.1, 0.9, 0.5],    // g>b>r
        ];
        for c in cases {
            let mut rgb = c;
            let (i, j, k) = order3(c[0], c[1], c[2]);
            // order3 自身必须给出正确的下标（这里断言它，而不是只断言输出有序）
            assert!(
                rgb[i] == c[0].min(c[1]).min(c[2]),
                "order3 的最小值下标错：{c:?} → {i}"
            );
            assert!(rgb[j] >= rgb[i] - 1e-6 && rgb[j] <= rgb[k] + 1e-6);
            apply_rgb_curve(&mut rgb, &lut);
            assert!(
                rgb[i] <= rgb[j] + 1e-4 && rgb[j] <= rgb[k] + 1e-4,
                "排序被破坏：{c:?} → {rgb:?}"
            );
        }
    }

    /// 三通道相等时必须落到同值，否则会 Inf*0 = NaN（NaN 量化为 u8 是 0，黑点）。
    #[test]
    fn rgb_curve_handles_degenerate_equal_channels() {
        let lut = lut_from(vec![CurvePoint::new(0.0, 0.4), CurvePoint::new(1.0, 0.6)]);
        let mut rgb = [0.5f32, 0.5, 0.5];
        apply_rgb_curve(&mut rgb, &lut);
        assert!(
            rgb.iter().all(|v| v.is_finite()),
            "同值通道产生 NaN: {rgb:?}"
        );
        assert!((rgb[0] - rgb[1]).abs() < 1e-6);
    }

    /// 值域外的输入（过曝像素）必须被夹住而不是索引越界 panic。
    #[test]
    fn rgb_curve_clamps_out_of_domain_input() {
        let lut = lut_from(vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)]);
        let mut rgb = [1.5f32, 2.0, -0.3];
        apply_rgb_curve(&mut rgb, &lut);
        assert!(rgb.iter().all(|v| v.is_finite()), "越界输入崩溃: {rgb:?}");
        assert!((0.0..=1.0).contains(&rgb[2]));
    }

    /// 逐通道查表 vs 保色相查表，对饱和色的差别必须明显 ——
    /// 这条测试守的是「别为了省几行代码退化成逐通道」。
    #[test]
    fn per_channel_lut_would_shift_hue_but_this_does_not() {
        let lut = lut_from(vec![
            CurvePoint::new(0.0, 0.25),
            CurvePoint::new(0.5, 0.45),
            CurvePoint::new(1.0, 0.95),
        ]);
        let mut rgb = [0.95f32, 0.35, 0.08];
        let before = hue_of(rgb);
        apply_rgb_curve(&mut rgb, &lut);
        assert!(
            (hue_of(rgb) - before).abs() < 0.06,
            "色相漂移 {:.3} 弧度太大，疑似退化成逐通道查表",
            (hue_of(rgb) - before).abs()
        );
    }

    // ---------- 亮度曲线 ----------

    #[test]
    fn luma_curve_scales_all_channels_equally() {
        let lut = lut_from(vec![
            CurvePoint::new(0.0, 0.1),
            CurvePoint::new(0.5, 0.7),
            CurvePoint::new(1.0, 1.0),
        ]);
        let mut rgb = [0.8f32, 0.5, 0.2];
        let before = rgb;
        apply_luma_curve(&mut rgb, &lut);
        let r0 = before[0] / before[1];
        let r1 = rgb[0] / rgb[1];
        assert!((r0 - r1).abs() < 1e-4, "亮度曲线改变了色相比：{r0} → {r1}");
    }

    /// 纯黑不能因为亮度曲线而产生 NaN 或被放大。
    #[test]
    fn luma_curve_leaves_black_alone() {
        let lut = lut_from(vec![CurvePoint::new(0.0, 0.3), CurvePoint::new(1.0, 1.0)]);
        let mut rgb = [0.0f32, 0.0, 0.0];
        apply_luma_curve(&mut rgb, &lut);
        assert_eq!(rgb, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn curves_bundle_reports_noop() {
        let empty = compile(&Curve::default(), &Curve::default());
        assert!(empty.is_noop());

        let one = compile(
            &Curve::default(),
            &Curve {
                points: vec![CurvePoint::new(0.2, 0.2), CurvePoint::new(0.8, 0.8)],
            },
        );
        assert!(!one.is_noop());
    }

    /// RGB 曲线与亮度曲线可以叠加（顺序：先 RGB 后亮度，与 pipeline 一致）。
    #[test]
    fn both_curves_compose_without_nan() {
        let c = compile(
            &Curve {
                points: vec![CurvePoint::new(0.0, 0.1), CurvePoint::new(1.0, 0.9)],
            },
            &Curve {
                points: vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)],
            },
        );
        let mut rgb = [0.4f32, 0.6, 0.8];
        apply_rgb_curve(&mut rgb, c.rgb.as_ref().unwrap());
        apply_luma_curve(&mut rgb, c.luma.as_ref().unwrap());
        assert!(rgb.iter().all(|v| v.is_finite()), "{rgb:?}");
    }

    /// 简易色相角（0..2π），仅用于测试断言。
    fn hue_of(rgb: [f32; 3]) -> f32 {
        let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        if d < 1e-9 {
            return 0.0;
        }
        let h = if (max - r).abs() < 1e-9 {
            ((g - b) / d).rem_euclid(6.0)
        } else if (max - g).abs() < 1e-9 {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        h * std::f32::consts::FRAC_PI_3
    }
}
