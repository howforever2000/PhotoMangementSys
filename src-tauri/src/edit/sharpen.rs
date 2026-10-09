//! 锐化：带边缘蒙版的非锐化掩模（USM）
//! =====================================================================
//! 四个参数与 Camera Raw「细节」面板一一对应：
//!
//! | 参数 | ACR 默认 | 本实现含义 |
//! |---|---|---|
//! | 数量 amount | 25% (raw) | 锐化总强度 |
//! | 半径 radius | 1.0 | 高斯 σ，以「短边 1000px 基准」归一化 |
//! | 细节 detail | 25 | 低频残差保留比例 → 压光晕 |
//! | 蒙版 masking | 0~25 | 只锐化强边缘，避开平坦区噪点 |
//!
//! ## 为什么要分「细节」和「蒙版」
//!
//! 朴素 USM = `原图 + amount × (原图 - 高斯模糊)`。它有两个不可回避的问题：
//!
//! 1. **光晕（halo）**：强边缘两侧出现亮/暗轮廓。`detail` 是解法 ——
//!    对残差做「软阈值」再按 `detail` 比例混合，软阈值把**大残差原样保留**、
//!    **小残差压到 0**，于是边缘的细密过渡被抹平，只剩轮廓本身需要提亮。
//! 2. **噪点放大**：平坦区（天空、皮肤）残差虽小但数量巨大，
//!    `amount` 一放大满屏起噪。`masking` 是解法 —— 用残差幅度做边缘蒙版，
//!    只让强边缘通过。
//!
//! ## 为什么在亮度上做，而不是三通道各做一遍
//!
//! 逐通道 USM 会分别放大各通道的色差 → 边缘出现彩色描边。
//! 只在 Rec.709 亮度上算残差、再把亮度增量等量加回三个通道，
//! 色差完全不受影响。darktable `data/kernels/sharpen.cl:165` 正是
//! 单独一个亮度核（`sharpen_mix` 只操作 `pixel.x`）。
//!
//! ## 半径为什么归一化到「短边 1000px」
//!
//! ACR 的半径以「缩放到 100% 时的像素」为单位，且实测行为随输出尺寸漂移
//! （同一条 recipe，导出 6000px 与 4000px 的观感不同）。
//! 本实现把 σ 绑定到**短边比例**，即 σ_px = radius_map × (min(w,h)/1000)。
//! 这样「同一份配方在不同尺寸的图上观感一致」，也符合用户直觉：
//! 半径滑块的意义是「相对图像尺寸的边缘宽度」。

use super::color::{clamp, smoothstep};
use super::recipe::SharpenAdjustments;

/// σ 的取值区间（对应 radius 滑块 0..=1）。
/// ACR 面板标称 0.5~3.0，取同值。
const SIGMA_MIN: f32 = 0.5;
const SIGMA_MAX: f32 = 3.0;

/// 基准短边像素数：σ 就是相对这个尺寸定义的。
const REFERENCE_SHORT_EDGE: f32 = 1000.0;

/// 「细节」的软阈值（残差绝对值低于此视作细节而非轮廓），单位为亮度 0..1。
/// 与 `detail` 滑块无关 —— 调低 `detail` 压的是**保留比例**，
/// 而阈值本身要固定，否则用户拖 `detail` 时噪声也跟着变，体验不可预测。
const SOFT_THRESHOLD: f32 = 0.008;

/// 边缘蒙版的斜率：残差 / 该值 后过 smoothstep。
/// 0.02 意味着「残差达到 2% 亮度（≈5/255）」才算强边缘。
const MASK_THRESHOLD: f32 = 0.02;

/// σ 的绝对下限。
///
/// σ 按「短边 / 1000」缩放意味着缩略图上的 σ 可能远小于 1 像素，
/// 此时 USM 退化成「减本像素」，把噪点放大成椒盐噪声。
/// 0.6px 是「还能看出锐化效果」与「不产生椒盐」的折中。
const SIGMA_FLOOR: f32 = 0.6;

/// 可分离高斯的一维核（σ 已在 `plan` 里定好）。
///
/// 半径取 `ceil(3σ)`：三倍标准差之外的高斯权重已 <1%，截断在该处
/// 引入的误差小于 8 位量化误差（±0.5/255）。
#[derive(Debug, Clone)]
struct Kernel {
    weights: Vec<f32>,
    radius: usize,
}

/// 构造归一化高斯核。
fn build_kernel(sigma: f32) -> Kernel {
    let r = (sigma * 3.0).ceil().max(1.0) as usize;
    let mut weights = Vec::with_capacity(r * 2 + 1);
    let denom = 2.0 * sigma * sigma;
    let mut sum = 0.0f32;
    for i in -(r as isize)..=(r as isize) {
        let d = i as f32;
        let w = (-d * d / denom).exp();
        weights.push(w);
        sum += w;
    }
    for w in weights.iter_mut() {
        *w /= sum;
    }
    Kernel { weights, radius: r }
}

/// 一次锐化的全部预计算参数。
#[derive(Debug, Clone)]
pub struct SharpenPlan {
    amount: f32,
    detail_mix: f32,
    mask_strength: f32,
    kernel: Kernel,
    /// 图像短边，σ 归一化用
    short_edge: u32,
}

impl SharpenPlan {
    /// 由配方 + 图像尺寸生成计划。返回 `None` 表示无需锐化。
    pub fn plan(a: &SharpenAdjustments, width: u32, height: u32) -> Option<Self> {
        if a.amount <= 0.0 {
            return None;
        }
        let short_edge = width.min(height).max(1);
        let radius_t = clamp(a.radius, 0.0, 1.0);
        let sigma = (SIGMA_MIN + (SIGMA_MAX - SIGMA_MIN) * radius_t)
            * (short_edge as f32 / REFERENCE_SHORT_EDGE);
        Some(Self {
            amount: a.amount,
            // detail 0 → 只保留「超过软阈值的部分」；detail 1 → 保留全部残差
            detail_mix: clamp(a.detail, 0.0, 1.0),
            mask_strength: clamp(a.masking, 0.0, 1.0),
            kernel: build_kernel(sigma.max(SIGMA_FLOOR)),
            short_edge,
        })
    }

    #[inline]
    pub fn is_noop(&self) -> bool {
        self.amount <= 0.0
    }
}

/// 亮度平面的一维可分离模糊（钳边）。
///
/// 输入输出都是 `w*h` 的 f32 亮度平面。`tmp` 由调用方复用以避免每行重新分配 ——
/// 可分离模糊会做两趟（横向、纵向），横向趟的输出直接写进 `tmp`。
fn blur_plane(plane: &mut [f32], tmp: &mut [f32], w: usize, h: usize, k: &Kernel) {
    let r = k.radius;

    // 横向
    for y in 0..h {
        let row = &plane[y * w..y * w + w];
        let out = &mut tmp[y * w..y * w + w];
        for x in 0..w {
            let mut acc = 0.0;
            for (i, &wt) in k.weights.iter().enumerate() {
                // 钳边：边缘像素重复采样，避免镜像产生虚影
                let sx = (x as isize + i as isize - r as isize).clamp(0, w as isize - 1) as usize;
                acc += row[sx] * wt;
            }
            out[x] = acc;
        }
    }

    // 纵向
    for y in 0..h {
        let out_row = &mut plane[y * w..y * w + w];
        for (x, slot) in out_row.iter_mut().enumerate() {
            let mut acc = 0.0;
            for (i, &wt) in k.weights.iter().enumerate() {
                let sy = (y as isize + i as isize - r as isize).clamp(0, h as isize - 1) as usize;
                acc += tmp[sy * w + x] * wt;
            }
            *slot = acc;
        }
    }
}

/// 就地锐化一个 f32 RGB 平面缓冲（展平存储，长度 = w*h*3）。
///
/// `buf` 会被原地改写。`scratch_luma` / `scratch_tmp` 是调用方持有的
/// 复用缓冲（长度均 ≥ w*h），用于避免每次锐化都重新分配 2×w×h 的 f32。
///
/// **性能**：这是整条管线里唯一的邻域算子（O(w·h·r)），也是最慢的一步。
/// 预览尺寸（≤1600px）下 σ 通常 < 3px，`w·h·2r ≈ 1000 万次乘加，
/// 在 rayon 下约 10~20ms，可以接受。导出全分辨率下会慢 10 倍以上，
/// 因此导出走的是「先缩到输出尺寸、再锐化」的次序（见 `pipeline.rs`）。
pub fn sharpen_in_place(
    buf: &mut [f32],
    w: usize,
    h: usize,
    plan: &SharpenPlan,
    scratch_luma: &mut [f32],
    scratch_tmp: &mut [f32],
) {
    let n = w * h;
    debug_assert!(buf.len() >= n * 3);
    debug_assert!(scratch_luma.len() >= n && scratch_tmp.len() >= n);

    // 1) 抽亮度（Rec.709）
    let [wr, wg, wb] = super::color::luminance_weights();
    for i in 0..n {
        scratch_luma[i] = wr * buf[i * 3] + wg * buf[i * 3 + 1] + wb * buf[i * 3 + 2];
    }

    // 2) 高斯模糊原亮度（scratch_luma 被原地覆盖为模糊结果）
    blur_plane(scratch_luma, scratch_tmp, w, h, &plan.kernel);

    // 3) 逐像素：残差 → 软阈值 → 边缘蒙版 → 加回亮度
    let amount = plan.amount;
    let mix = plan.detail_mix;
    let mask_s = plan.mask_strength;
    for i in 0..n {
        let y = wr * buf[i * 3] + wg * buf[i * 3 + 1] + wb * buf[i * 3 + 2];
        let blur = scratch_luma[i];
        let hp = y - blur;

        // 「细节」：软阈值。软阈值函数 S(x)=sign(x)·max(|x|-t, 0) 是分段线性的
        // Lipschitz 函数，Laplacian 先导数在 |x|=t 处有跳变，理论上会有 1 阶振铃；
        // 用 smoothstep 在 t 附近做 1 格软化把 C⁰ 提到 C¹，代价是每像素多一次 clamp。
        let mag = hp.abs();
        let over = (mag - SOFT_THRESHOLD).max(0.0);
        // 在 [SOFT_THRESHOLD, 2*SOFT_THRESHOLD] 区间做 smoothstep 过渡
        let t = ((mag - SOFT_THRESHOLD) / SOFT_THRESHOLD).clamp(0.0, 1.0);
        let soft = over * (t * t * (3.0 - 2.0 * t) * 0.5 + 0.5);
        let shaped = if hp < 0.0 { -soft } else { soft };

        // detail=1 → 用原始残差；detail=0 → 只用软阈值后的残差
        let residual = hp + (shaped - hp) * (1.0 - mix);

        // 「蒙版」：残差越大越锐
        let mask = if mask_s <= 0.0 {
            1.0
        } else {
            let edge = smoothstep(0.0, MASK_THRESHOLD, residual.abs());
            1.0 - (1.0 - edge) * mask_s
        };

        let delta = amount * residual * mask;
        buf[i * 3] += delta;
        buf[i * 3 + 1] += delta;
        buf[i * 3 + 2] += delta;
    }
}

/// 为「无细节面板」的代码路径提供安全占位：`0.0` 金额 → 不做锐化。
pub fn no_plan() -> Option<SharpenPlan> {
    None
}

/// 供 UI 显示「当前 σ 像素数」（**请求值**，未含 `SIGMA_FLOOR` 下限）。
pub fn effective_sigma(a: &SharpenAdjustments, width: u32, height: u32) -> f32 {
    let short_edge = width.min(height).max(1);
    let radius_t = clamp(a.radius, 0.0, 1.0);
    (SIGMA_MIN + (SIGMA_MAX - SIGMA_MIN) * radius_t) * (short_edge as f32 / REFERENCE_SHORT_EDGE)
}

/// 供 UI 显示锐化是否启用。
pub fn short_edge_of(plan: &SharpenPlan) -> u32 {
    plan.short_edge
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张测试图：`base` 为背景亮度，`mode` 决定叠加什么边缘。
    fn make_image(w: usize, h: usize, base: f32, mode: EdgePattern) -> Vec<f32> {
        let mut buf = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let e = match mode {
                    EdgePattern::None => false,
                    EdgePattern::Cross => x == w / 2 || y == h / 2,
                    EdgePattern::Vertical => x == w / 2,
                    EdgePattern::Checker => (x / 4 + y / 4) % 2 == 0,
                };
                let v = if e { base + 0.3 } else { base };
                let i = (y * w + x) * 3;
                buf[i] = v;
                buf[i + 1] = v;
                buf[i + 2] = v;
            }
        }
        buf
    }

    #[derive(Clone, Copy, PartialEq)]
    enum EdgePattern {
        None,
        Cross,
        Vertical,
        Checker,
    }

    // ---------- 计划 ----------

    #[test]
    fn zero_amount_produces_no_plan() {
        let a = SharpenAdjustments::default();
        assert!(SharpenPlan::plan(&a, 1000, 1000).is_none());
    }

    #[test]
    fn nonzero_amount_produces_plan() {
        let a = SharpenAdjustments {
            amount: 0.3,
            ..Default::default()
        };
        assert!(SharpenPlan::plan(&a, 1000, 1000).is_some());
    }

    /// σ 必须随图像尺寸缩放，否则同一份 recipe 在大图上光晕宽度完全不同。
    #[test]
    fn sigma_scales_with_short_edge() {
        let a = SharpenAdjustments {
            amount: 0.5,
            radius: 0.5,
            ..Default::default()
        };
        let s_small = effective_sigma(&a, 500, 500);
        let s_large = effective_sigma(&a, 2000, 2000);
        assert!(
            (s_large / s_small - 4.0).abs() < 0.01,
            "边长翻 4 倍 σ 应翻 4 倍，实得 {} vs {}",
            s_small,
            s_large
        );
    }

    #[test]
    fn sigma_within_documented_range_at_reference_size() {
        let a = SharpenAdjustments {
            radius: 0.0,
            amount: 0.5,
            ..Default::default()
        };
        assert!((effective_sigma(&a, 1000, 1000) - SIGMA_MIN).abs() < 1e-4);
        let a = SharpenAdjustments {
            radius: 1.0,
            amount: 0.5,
            ..Default::default()
        };
        assert!((effective_sigma(&a, 1000, 1000) - SIGMA_MAX).abs() < 1e-4);
    }

    #[test]
    fn kernel_is_normalized() {
        let k = build_kernel(1.5);
        let sum: f32 = k.weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "核未归一化：{sum}");
    }

    /// 核半径过小会让 USM 退化成「减本像素」，产生椒盐噪点。
    #[test]
    fn kernel_radius_is_at_least_three_sigma() {
        for sigma in [0.5f32, 1.0, 2.0, 3.0] {
            let k = build_kernel(sigma);
            assert!(
                k.radius >= (3.0 * sigma) as usize - 1,
                "σ={sigma} 半径 {} 太小",
                k.radius
            );
        }
    }

    // ---------- 行为 ----------

    /// 核心行为：锐化必须在边缘**增加**对比度。
    #[test]
    fn sharpening_increases_edge_contrast() {
        // σ 随短边缩放，小图上 σ 会退化到下限，故用 400px 测真实行为
        let (w, h) = (400, 400);
        let a = SharpenAdjustments {
            amount: 0.8,
            radius: 0.5,
            detail: 0.5,
            masking: 0.0,
        };
        let plan = SharpenPlan::plan(&a, w as u32, h as u32).unwrap();

        // ⚠️ 必须用**纯竖直**边缘。若用十字边缘，「边缘左侧」那个像素
        // 本身就落在亮横线上，它的邻域上下是暗的 → 残差为正 → 被提亮，
        // 与「光晕应把边缘外侧压暗」的现象完全相反。测试图选错，
        // 断言再合理也测不到想测的东西。
        let y = h / 3; // 避开任何横向结构
        let mut buf = make_image(w, h, 0.4, EdgePattern::Vertical);
        let before_edge = buf[(y * w + w / 2) * 3];
        let before_left = buf[(y * w + w / 2 - 2) * 3];

        let mut luma = vec![0.0f32; w * h];
        let mut tmp = vec![0.0f32; w * h];
        sharpen_in_place(&mut buf, w, h, &plan, &mut luma, &mut tmp);

        let after_edge = buf[(y * w + w / 2) * 3];
        let after_left = buf[(y * w + w / 2 - 2) * 3];
        assert!(
            after_edge > before_edge + 0.01,
            "边缘应被提亮：{before_edge} → {after_edge}"
        );
        assert!(
            after_left < before_left - 0.001,
            "边缘外侧应压暗（形成光晕两侧）：{before_left} → {after_left}"
        );
    }

    /// 蒙版开启后，平坦区噪点不得被放大 —— 这是 masking 存在的唯一理由。
    #[test]
    fn masking_protects_flat_regions() {
        let (w, h) = (200, 200);
        // 平坦区 + 逐像素交替的 ±1% 「噪点」
        let mut template = make_image(w, h, 0.4, EdgePattern::None);
        for p in 0..(w * h) {
            let n = if p % 2 == 0 { 0.01f32 } else { -0.01 };
            let i = p * 3;
            template[i] += n;
            template[i + 1] += n;
            template[i + 2] += n;
        }
        let before: f32 = template[0];
        let mut luma = vec![0.0f32; w * h];
        let mut tmp = vec![0.0f32; w * h];

        // **两次必须各用一份独立缓冲**：在同一个 buf 上连续锐化两次，
        // 第二次的增量会叠加在第一次之上，量出来的是累计量而不是对比。
        let mut open = template.clone();
        let a = SharpenAdjustments {
            amount: 1.0,
            masking: 0.0,
            ..Default::default()
        };
        let p0 = SharpenPlan::plan(&a, w as u32, h as u32).unwrap();
        sharpen_in_place(&mut open, w, h, &p0, &mut luma, &mut tmp);
        let noise_open = (open[0] - before).abs();

        let mut masked = template.clone();
        let a = SharpenAdjustments {
            amount: 1.0,
            masking: 1.0,
            ..Default::default()
        };
        let p1 = SharpenPlan::plan(&a, w as u32, h as u32).unwrap();
        sharpen_in_place(&mut masked, w, h, &p1, &mut luma, &mut tmp);
        let noise_masked = (masked[0] - before).abs();

        assert!(
            noise_open > 1e-5,
            "平坦区本不该有明显变化（测试无效）：{noise_open}"
        );
        assert!(
            noise_masked < noise_open * 0.6,
            "蒙版开启后平坦区变化应显著变小：{noise_masked} vs {noise_open}"
        );
    }

    /// 三通道增量必须完全相同 —— 否则边缘出现彩色描边。
    #[test]
    fn sharpening_adds_equal_delta_to_all_channels() {
        let (w, h) = (240, 240);
        let a = SharpenAdjustments {
            amount: 0.7,
            ..Default::default()
        };
        let plan = SharpenPlan::plan(&a, w as u32, h as u32).unwrap();
        let mut buf = vec![0.0f32; w * h * 3];
        // 造一个红蓝差异极大的边缘
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                buf[i] = if x < w / 2 { 0.9 } else { 0.1 };
                buf[i + 1] = 0.4;
                buf[i + 2] = 0.2;
            }
        }
        let before: Vec<f32> =
            buf[((h / 2) * w + w / 2) * 3..((h / 2) * w + w / 2) * 3 + 3].to_vec();

        let mut luma = vec![0.0f32; w * h];
        let mut tmp = vec![0.0f32; w * h];
        sharpen_in_place(&mut buf, w, h, &plan, &mut luma, &mut tmp);

        let after = &buf[((h / 2) * w + w / 2) * 3..((h / 2) * w + w / 2) * 3 + 3];
        let d = [
            after[0] - before[0],
            after[1] - before[1],
            after[2] - before[2],
        ];
        assert!(
            (d[0] - d[1]).abs() < 1e-6 && (d[1] - d[2]).abs() < 1e-6,
            "三通道增量不等 {d:?} —— 会出现彩色描边"
        );
    }

    /// 恒定图不得被锐化（残差处处为 0）—— 否则整幅图被整体推亮或压暗。
    #[test]
    fn flat_image_is_unchanged() {
        let (w, h) = (120, 120);
        let a = SharpenAdjustments {
            amount: 1.0,
            detail: 1.0,
            masking: 0.0,
            ..Default::default()
        };
        let plan = SharpenPlan::plan(&a, w as u32, h as u32).unwrap();
        let mut buf = make_image(w, h, 0.55, EdgePattern::None);
        let before = buf.clone();
        let mut luma = vec![0.0f32; w * h];
        let mut tmp = vec![0.0f32; w * h];
        sharpen_in_place(&mut buf, w, h, &plan, &mut luma, &mut tmp);
        for i in 0..buf.len() {
            assert!((buf[i] - before[i]).abs() < 1e-5, "平坦图被改动 {i}");
        }
    }

    #[test]
    fn sharpening_never_produces_nan() {
        let (w, h) = (64, 64);
        let a = SharpenAdjustments {
            amount: 1.0,
            detail: 0.0,
            masking: 1.0,
            radius: 0.0,
        };
        let plan = SharpenPlan::plan(&a, w as u32, h as u32).unwrap();
        let mut buf = vec![0.0f32; w * h * 3];
        for (i, v) in buf.iter_mut().enumerate() {
            *v = if i % 7 == 0 { 0.0 } else { 1.0 };
        }
        let mut luma = vec![0.0f32; w * h];
        let mut tmp = vec![0.0f32; w * h];
        sharpen_in_place(&mut buf, w, h, &plan, &mut luma, &mut tmp);
        assert!(buf.iter().all(|v| v.is_finite()), "出现 NaN/Inf");
    }

    /// 极小图（短边 1px）不得 panic —— 用户确实可能对 1px 缩略图点导出。
    #[test]
    fn degenerate_sizes_do_not_panic() {
        for (w, h) in [(1usize, 1usize), (1, 10), (10, 1), (2, 2)] {
            let a = SharpenAdjustments {
                amount: 0.5,
                ..Default::default()
            };
            let plan = SharpenPlan::plan(&a, w as u32, h as u32).unwrap();
            let mut buf = vec![0.5f32; w * h * 3];
            let mut luma = vec![0.0f32; w * h];
            let mut tmp = vec![0.0f32; w * h];
            sharpen_in_place(&mut buf, w, h, &plan, &mut luma, &mut tmp);
            assert!(buf.iter().all(|v| v.is_finite()), "{w}×{h} 出 NaN");
        }
    }

    #[test]
    fn no_plan_is_none() {
        assert!(no_plan().is_none());
    }

    #[test]
    fn plan_exposes_short_edge() {
        let a = SharpenAdjustments {
            amount: 0.5,
            ..Default::default()
        };
        let p = SharpenPlan::plan(&a, 800, 1200).unwrap();
        assert_eq!(short_edge_of(&p), 800);
        assert!(!p.is_noop());
    }
}
