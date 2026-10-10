//! 局部蒙版：把「调整」限定在画面的一部分区域
//! ====================================================================
//! ## 为什么需要它
//!
//! Lightroom 的「局部调整」是基准调色的自然延伸：整体调完一遍之后，
//! 往往只有某个局部需要单独提亮或压暗。没有蒙版就只能整体拉，
//! 于是「人像压暗、天空提亮」这类需求只能靠手动画框分次处理，
//! 而且每次改一个参数都要重做一遍。
//!
//! ## 语义（与 python-studio 的 `_apply_masked` 一致）
//!
//! ```text
//! out = base * (1 - m) + adjusted * m        m = feather(mask) * strength
//! ```
//!
//! `base` 是「只做几何变换（裁剪/拉直/缩放）、不做任何调整」的结果，
//! `adjusted` 是完整管线的输出。因此：
//!
//! - `m = 0`（蒙版外）→ 完全等于原图（几何变换照旧生效）；
//! - `m = 1`（蒙版内）→ 完全等于调整结果；
//! - 中间值 → 线性过渡。
//!
//! ## 为什么要「渲染两遍」而不是「只算局部」
//!
//! 管线是逐像素的全图运算，局部化只有「算两次再混合」这一种做法：
//! 一遍带调整、一遍不带（只跑几何），然后按蒙版加权。代价是几何阶段
//! 跑两遍，但那本来就是最便宜的一阶段（一次采样 + 双线性插值），
//! 相对后面 1~5 阶段的逐像素 f32 运算可以忽略。
//!
//! ## 分辨率约定
//!
//! 蒙版**按预览分辨率存**，渲染时再缩放到目标尺寸（与 python-studio
//! 用 `INTER_NEAREST` 对齐蒙版的做法同源）。这样有两个好处：
//! 蒙版文件不会随原图尺寸膨胀（6000×4000 的灰度 PNG 约 24MB，
//! 而长边 1600 的只有约 2.5MB），且导出时放大蒙版也不会因为
//! 插值而糊掉选区边界（最近邻保持硬边）。
//!
//! 羽化半径以**蒙版自身像素**为单位定义，随缩放同比例放大 —— 这样
//! 「羽化 20」在预览和全分辨率导出下的**视觉宽度**是一致的。

use super::pipeline::Plane;
use super::recipe::MaskSpec;

/// 灰度蒙版（8 位，0 = 未选中，255 = 完全选中）。
#[derive(Debug, Clone, Default)]
pub struct MaskImage {
    width: u32,
    height: u32,
    /// 长度 = `width * height`，值域 0..=255
    data: Vec<u8>,
}

impl MaskImage {
    /// 空蒙版（等价于「不启用局部调整」）。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 从 PNG 字节解码。解码后强制转灰度（`to_luma8`）。
    ///
    /// 非 PNG 或数据损坏时返回错误字符串，由命令层翻译成用户可读的提示。
    pub fn from_png(bytes: &[u8]) -> Result<Self, String> {
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .map_err(|e| format!("蒙版 PNG 解码失败：{e}"))?;
        let luma = img.to_luma8();
        Ok(Self {
            width: luma.width(),
            height: luma.height(),
            data: luma.into_raw(),
        })
    }

    /// 宽高皆为 0 或没有蒙版。
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0 || self.data.is_empty()
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// 选中区域的平均占比 0..=1（用于 UI 提示「选中了多少」）。
    pub fn coverage(&self) -> f32 {
        if self.is_empty() {
            return 0.0;
        }
        let sum: u64 = self.data.iter().map(|&v| v as u64).sum();
        sum as f32 / (self.data.len() as f32 * 255.0)
    }
}

/// 羽化半径上限（蒙版像素）。超过这个值在实用上已等于「全图渐变」。
pub const MAX_FEATHER_PX: f32 = 120.0;

/// 羽化半径（蒙版像素）→ 缩放到目标尺寸后的实际半径。
fn scaled_radius(feather_px: f32, mask_long: u32, out_long: u32) -> f32 {
    if mask_long == 0 || out_long == 0 {
        return 0.0;
    }
    feather_px * (out_long as f32 / mask_long.max(1) as f32)
}

/// 把蒙版采样成 `out_w × out_h` 的权重平面（f32，值域 0..=1），
/// 并已应用「反相 / 羽化 / 强度」。结果写入 `scratch`。
///
/// # 返回
/// `true` 表示 `scratch` 里有一张有效的权重平面；`false` 表示「无需混合」
/// （没有蒙版 / 未启用 / 权重全为 0），此时 `scratch` 内容无意义。
///
/// # 为什么写入调用方的暂存而不是返回值
///
/// 返回值若为 `Plane` 会与调用方的 `&mut Workspace` 冲突 —— 借用检查器
/// 不允许「持有 ws 的可变借用同时再交出 ws 的一部分」。写入 + 返回
/// bool 是唯一能让调用方在同一帧里既拿权重平面又写输出缓冲的形态。
///
/// 权重平面每次渲染都重算（尺寸随档位变化），因此**不做跨帧缓存**：
/// 一张长边 1600 的权重平面是 2.7MB，算一遍约 2~5ms，相对管线本身
/// 可以忽略；缓存它反而要处理「蒙版变了要不要失效」的问题。
pub fn write_weight_plane(
    mask: &MaskImage,
    spec: &MaskSpec,
    out_w: u32,
    out_h: u32,
    scratch: &mut Plane,
) -> bool {
    if mask.is_empty() || !spec.enabled {
        return false;
    }
    let ow = out_w.max(1) as usize;
    let oh = out_h.max(1) as usize;

    // ---- 阶段 1：最近邻重采样到输出尺寸 ----
    // 用最近邻而非双线性：蒙版的语义是「选/不选」，双线性会在边界
    // 产生中间值，等于凭空多了一圈没有用户意图的过渡带。
    // 需要柔滑边缘时由羽化阶段负责，那是有明确半径的、可控的。
    let plane = scratch.ensure(ow * oh);
    let mx = mask.width.max(1);
    let my = mask.height.max(1);
    for y in 0..oh {
        let sy = ((y as u32 * my / oh as u32).min(my - 1)) as usize;
        let row = sy * mx as usize;
        let out_row = y * ow;
        for x in 0..ow {
            let sx = ((x as u32 * mx / ow as u32).min(mx - 1)) as usize;
            let v = mask.data[row + sx] as f32 / 255.0;
            plane[out_row + x] = if spec.invert { 1.0 - v } else { v };
        }
    }

    // ---- 阶段 2：羽化（可分离箱式模糊 ×3 ≈ 高斯）----
    // 用三次箱式模糊逼近高斯，而不是直接实现高斯核：
    // 箱式模糊是 O(1) 每像素（滑动窗口），高斯是 O(r)；
    // 半径 120 时两者差 3 个数量级，而羽化的观感差异肉眼不可辨。
    let radius = scaled_radius(spec.feather_px, mask.width.max(mask.height), out_w.max(out_h));
    let radius = radius.round() as i32;
    if radius >= 1 {
        for _ in 0..3 {
            box_blur_h(plane, ow, oh, radius);
            box_blur_v(plane, ow, oh, radius);
        }
    }

    // ---- 阶段 3：强度缩放 ----
    let s = spec.strength.clamp(0.0, 1.0);
    if s < 1.0 {
        for v in plane.iter_mut() {
            *v *= s;
        }
    }

    // 全 0 的权重平面等价于「无蒙版」——提前返回，省掉后面逐像素的乘法。
// 注意这里是 `!all(...)`：全零才是「无蒙版」，因此返回的是「有内容」。
!plane.iter().all(|&v| v <= 0.0)
}

/// 水平方向箱式模糊（滑动窗口，O(1) 每像素）。
fn box_blur_h(plane: &mut [f32], w: usize, h: usize, radius: i32) {
    let r = radius as usize;
    if r == 0 || w == 0 {
        return;
    }
    // 窗口取 `2r+1`，越界按「最近的有效值」处理（即边缘复制）
    let mut line = vec![0f32; w];
    for y in 0..h {
        let row = &mut plane[y * w..(y + 1) * w];
        line.copy_from_slice(row);
        let win = 2 * r + 1;
        // 初始窗口 x=0 覆盖 [-r, r]，其中负下标按边缘复制（clamp 到 0）。
        // ⚠️ 不能简化成「取 line[0..win]」—— 那样等于把窗口当成 [0, 2r]，
        // 整条模糊会向右偏移 r 个像素。这正是本测试最初失败的原因。
        let mut sum = 0f32;
        for j in 0..win {
            sum += line[(j as i64 - r as i64).clamp(0, w as i64 - 1) as usize];
        }
        for x in 0..w {
            row[x] = sum / win as f32;
            // 滑动到 x+1：窗口从 [x-r, x+r] 变成 [x+1-r, x+1+r]，
            // 即移出 (x-r)、移入 (x+r+1)。
            // ⚠️ 移入的是 `x+r+1` 而非 `x+win`：后者(=x+2r+1) 会跳过
            // r 个元素，累积后窗口和越来越小，权重甚至会变成负数。
            let out_idx = (x as i64 - r as i64).clamp(0, w as i64 - 1) as usize;
            let in_idx = (x as i64 + r as i64 + 1).clamp(0, w as i64 - 1) as usize;
            sum += line[in_idx] - line[out_idx];
        }
    }
}

/// 垂直方向箱式模糊（滑动窗口，O(1) 每像素）。
fn box_blur_v(plane: &mut [f32], w: usize, h: usize, radius: i32) {
    let r = radius as usize;
    if r == 0 || h == 0 {
        return;
    }
    let mut line = vec![0f32; h];
    let win = 2 * r + 1;
    for x in 0..w {
        for y in 0..h {
            line[y] = plane[y * w + x];
        }
        let mut sum = 0f32;
        for j in 0..win {
            sum += line[(j as i64 - r as i64).clamp(0, h as i64 - 1) as usize];
        }
        for y in 0..h {
            plane[y * w + x] = sum / win as f32;
            let out_idx = (y as i64 - r as i64).clamp(0, h as i64 - 1) as usize;
            let in_idx = (y as i64 + r as i64 + 1).clamp(0, h as i64 - 1) as usize;
            sum += line[in_idx] - line[out_idx];
        }
    }
}

/// 按权重平面把 `base`（几何结果）与 `adjusted`（完整管线结果）混合，
/// 结果**原地写回** `adjusted`。
///
/// # 为什么是「原地」而不是 `out: &mut [u8]`
///
/// 管线的输出缓冲只有一个（`ws.out_rgb`）。若签名要求 `adjusted: &[u8]`
/// 与 `out: &mut [u8]` 是两块内存，调用方就得先 clone 一份 200KB 的
/// 图像出来只为绕过借用检查器 —— 每帧多一次完整拷贝。
///
/// 原地是安全的：每个输出像素只依赖 `base` 的同一位置与 `adjusted` 的
/// 同一位置，没有任何跨像素的邻域读取，因此「先读后写同一索引」不会
/// 读到被自己改过的值。
///
/// 三者长度必须一致（`out_w * out_h * 3`）。
pub fn blend_in_place(base: &[u8], adjusted: &mut [u8], weights: &[f32]) {
    debug_assert_eq!(base.len(), adjusted.len());
    for (px, &w) in weights.iter().enumerate() {
        let i = px * 3;
        if w >= 1.0 {
            // 完全选中：adjusted 已是正确值，无需写
            continue;
        }
        if w <= 0.0 {
            // 完全未选中：必须**写回 base**，不能跳过 ——
            // 跳过会让该像素停留在 adjusted，而契约是 m=0 时输出等于原图。
            adjusted[i] = base[i];
            adjusted[i + 1] = base[i + 1];
            adjusted[i + 2] = base[i + 2];
            continue;
        }
        let inv = 1.0 - w;
        adjusted[i] = (base[i] as f32 * inv + adjusted[i] as f32 * w).round() as u8;
        adjusted[i + 1] = (base[i + 1] as f32 * inv + adjusted[i + 1] as f32 * w).round() as u8;
        adjusted[i + 2] = (base[i + 2] as f32 * inv + adjusted[i + 2] as f32 * w).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(enabled: bool) -> MaskSpec {
        MaskSpec {
            enabled,
            feather_px: 0.0,
            strength: 1.0,
            invert: false,
            has_image: true,
        }
    }

    /// 跑一遍 `write_weight_plane` 并取回权重（测试用）。
    fn weights_of(m: &MaskImage, s: &MaskSpec, w: u32, h: u32) -> Option<Vec<f32>> {
        let mut scratch = Plane::default();
        if super::write_weight_plane(m, s, w, h, &mut scratch) {
            Some(scratch.as_slice().to_vec())
        } else {
            None
        }
    }

    /// 造一张蒙版：指定位置置 255，其余 0。
    fn mask_with(mw: u32, mh: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> MaskImage {
        let mut m = MaskImage {
            width: mw,
            height: mh,
            data: vec![0u8; (mw * mh) as usize],
        };
        for y in y0..y1 {
            for x in x0..x1 {
                m.data[(y * mw + x) as usize] = 255;
            }
        }
        m
    }

    #[test]
    fn empty_mask_yields_no_weight_plane() {
        assert!(weights_of(&MaskImage::empty(), &spec(true), 8, 8).is_none());
    }

    #[test]
    fn disabled_spec_yields_no_weight_plane() {
        let m = mask_with(8, 8, 0, 0, 4, 4);
        assert!(weights_of(&m, &spec(false), 8, 8).is_none());
    }

    #[test]
    fn all_zero_mask_yields_no_weight_plane() {
        let m = mask_with(8, 8, 0, 0, 0, 0); // 没有任何像素被选中
        assert!(weights_of(&m, &spec(true), 8, 8).is_none());
    }

    #[test]
    fn interior_is_full_and_exterior_is_zero() {
        let m = mask_with(8, 8, 2, 2, 6, 6);
        let p = weights_of(&m, &spec(true), 8, 8).unwrap();
        // 选区内
        assert!((p[3 * 8 + 3] - 1.0).abs() < 1e-6, "选区内应为 1，实际 {}", p[3 * 8 + 3]);
        // 选区外
        assert!(p[0] <= 0.0, "选区外应为 0，实际 {}", p[0]);
        assert!(p[7 * 8 + 7] <= 0.0, "选区外应为 0，实际 {}", p[7 * 8 + 7]);
    }

    #[test]
    fn invert_flips_selection() {
        let m = mask_with(8, 8, 2, 2, 6, 6);
        let mut s = spec(true);
        s.invert = true;
        let p = weights_of(&m, &s, 8, 8).unwrap();
        assert!(p[0] > 0.99, "反相后原选区外应变选中，实际 {}", p[0]);
        assert!(p[3 * 8 + 3] <= 0.01, "反相后原选区内应变未选中，实际 {}", p[3 * 8 + 3]);
    }

    #[test]
    fn strength_scales_weights() {
        let m = mask_with(8, 8, 2, 2, 6, 6);
        let mut s = spec(true);
        s.strength = 0.5;
        let p = weights_of(&m, &s, 8, 8).unwrap();
        assert!((p[3 * 8 + 3] - 0.5).abs() < 1e-6, "强度 0.5 应得 0.5，实际 {}", p[3 * 8 + 3]);
    }

    #[test]
    fn strength_zero_collapses_to_no_plane() {
        let m = mask_with(8, 8, 2, 2, 6, 6);
        let mut s = spec(true);
        s.strength = 0.0;
        assert!(weights_of(&m, &s, 8, 8).is_none(), "强度 0 应等价于无蒙版");
    }

    #[test]
    fn feather_softens_the_boundary() {
        let m = mask_with(16, 16, 4, 4, 12, 12);
        let mut s = spec(true);
        s.feather_px = 3.0;
        let p = weights_of(&m, &s, 16, 16).unwrap();
        // 边界上应出现中间值（不再是硬 0/1）
        let edge = p[4 * 16 + 8]; // 选区上边缘那一行
        assert!(edge > 0.01 && edge < 0.99, "羽化后边界应是中间值，实际 {edge}");
    }

    #[test]
    fn feather_keeps_interior_saturated_and_exterior_clear() {
        // 选区 6..18（24px 画布），中心距边界 6px；羽化 2px 三次箱式 ≈ σ 2，
        // 中心应几乎不受影响，角落外侧应几乎为 0。
        let m = mask_with(24, 24, 6, 6, 18, 18);
        let mut s = spec(true);
        s.feather_px = 2.0;
        let p = weights_of(&m, &s, 24, 24).unwrap();
        assert!(
            (p[12 * 24 + 12] - 1.0).abs() < 0.02,
            "选区中心应接近 1，实际 {}",
            p[12 * 24 + 12]
        );
        assert!(p[0] <= 0.02, "远外侧应接近 0，实际 {}", p[0]);
        assert!(p[2 * 24 + 2] <= 0.05, "角落外侧应接近 0，实际 {}", p[2 * 24 + 2]);
    }

    #[test]
    fn feather_is_antisymmetric_about_the_boundary() {
        // 防「模糊整体偏移」这类 bug。
        //
        // 阶跃边界落在像素 15 与 16 之间，中心在 15.5。对称的模糊必然让
        // 关于该中心镜像的两个像素**互补**（和为 1），而不是相等 ——
        // 选区内侧偏 1、外侧偏 0。若实现把窗口当成 [0, 2r] 而非 [-r, r]，
        // 整条过渡带会右移 r 像素，这里立刻暴露。
        let m = mask_with(32, 32, 16, 0, 32, 32); // 右半选中
        let mut s = spec(true);
        s.feather_px = 3.0;
        let p = weights_of(&m, &s, 32, 32).unwrap();
        let row = 16usize;
        for d in 1..=6 {
            let inside = p[row * 32 + (16 - d)];
            let outside = p[row * 32 + (16 + d - 1)];
            assert!(
                (inside + outside - 1.0).abs() < 1e-5,
                "距边界 {d} 处镜像像素不互补：{inside} + {outside}"
            );
        }
    }

    #[test]
    fn wider_feather_spreads_further() {
        let m = mask_with(48, 32, 24, 0, 48, 32);
        let mut s = spec(true);
        s.feather_px = 1.0;
        let narrow = weights_of(&m, &s, 48, 32).unwrap();
        s.feather_px = 8.0;
        let wide = weights_of(&m, &s, 48, 32).unwrap();
        // 选区外 5px 处（边界在 x=24）：羽化越宽，该点越被「拉」向 0.5
        let probe = 24 - 5;
        assert!(
            wide[16 * 48 + probe] > narrow[16 * 48 + probe],
            "更宽的羽化在选区外同一点应给出更高的权重（过渡带更宽）：\
             窄 {} vs 宽 {}",
            narrow[16 * 48 + probe],
            wide[16 * 48 + probe]
        );
    }

    #[test]
    fn resize_uses_nearest_neighbour_so_edges_stay_hard() {
        // 8px 蒙版的左 4 列选中 → 放大到 16px：应恰好左 8 列选中。
        let m = mask_with(8, 8, 0, 0, 4, 8);
        let p = weights_of(&m, &spec(true), 16, 16).unwrap();
        let row = &p[8 * 16..9 * 16]; // 取 y=8 那一行
        // 关键：边界两侧必须是硬 0/1。用双线性会得到 0.5 的过渡带，
        // 那等于凭空多了一圈用户没画过的柔边。
        assert!(
            row[..8].iter().all(|&v| v == 1.0),
            "放大的左半应恒为 1，实际 {:?}",
            &row[..8]
        );
        assert!(
            row[8..].iter().all(|&v| v == 0.0),
            "放大的右半应恒为 0，实际 {:?}",
            &row[8..]
        );
    }

    #[test]
    fn downscale_also_uses_nearest_neighbour() {
        // 左半全选（4px 宽）→ 缩到 4px 宽：应只剩左两列被选中
        let m = mask_with(8, 8, 0, 0, 4, 8);
        let p = weights_of(&m, &spec(true), 4, 4).unwrap();
        assert!(p[0] > 0.99, "缩小后左列仍应选中，实际 {}", p[0]);
        assert!(p[3] <= 0.01, "缩小后右列仍应不选，实际 {}", p[3]);
    }

    #[test]
    fn blend_picks_adjusted_inside_and_base_outside() {
        // 2 像素：底图全 0，调整结果全 200
        let base = [0u8, 0, 0, 0, 0, 0];
        let mut adjusted = [200u8, 200, 200, 200, 200, 200];
        let weights = [0.0f32, 1.0];
        blend_in_place(&base, &mut adjusted, &weights);
        assert_eq!(&adjusted[0..3], &[0, 0, 0], "未选中处应回到底图");
        assert_eq!(&adjusted[3..6], &[200, 200, 200], "选中处应保持调整结果");
    }

    #[test]
    fn blend_interpolates_at_half_weight() {
        let base = [0u8, 0, 0];
        let mut adjusted = [200u8, 200, 200];
        blend_in_place(&base, &mut adjusted, &[0.5f32]);
        assert_eq!(&adjusted, &[100, 100, 100], "半权重应线性插值");
    }

    #[test]
    fn blend_restores_base_at_zero_weight() {
        // m=0 的契约是「输出等于原图」，所以必须写回 base 而不是跳过。
        // 跳过的话，选区外会残留调整结果 —— 局部调整就变成了全局调整。
        let base = [10u8, 20, 30];
        let mut adjusted = [200u8, 100, 50];
        blend_in_place(&base, &mut adjusted, &[0.0f32]);
        assert_eq!(&adjusted, &base, "零权重处应回到底图");
    }

    #[test]
    fn blend_pixels_are_independent_of_neighbours() {
        // 原地安全的前提：每个像素只读 base/adjusted 的同一位置。
        // 若实现里误引入邻域读取，后处理的像素就会读到已被改写的邻居。
        // 这里三个像素用三种权重（1.0 / 0.5 / 0.25）并手算期望值：
        //   px0  w=1.00 → 保持 adjusted           = 200,200,200
        //   px1  w=0.50 → 0.5*(30..50)+0.5*(210)  = 120,125,130
        //   px2  w=0.25 → 0.75*(60..80)+0.25*(220)= 100,108,115（0.5 处四舍五入到 108）
        let base: Vec<u8> = (0..9u32).map(|i| (i * 10) as u8).collect();
        let mut adjusted = vec![200u8, 200, 200, 210, 210, 210, 220, 220, 220];
        blend_in_place(&base, &mut adjusted, &[1.0, 0.5, 0.25]);
        assert_eq!(
            adjusted,
            vec![200, 200, 200, 120, 125, 130, 100, 108, 115],
            "混合结果应与逐像素手算一致"
        );
    }

    #[test]
    fn coverage_reflects_selected_fraction() {
        let m = mask_with(10, 10, 0, 0, 5, 10); // 左半
        assert!((m.coverage() - 0.5).abs() < 0.01, "覆盖率应约 0.5，实际 {}", m.coverage());
        assert_eq!(MaskImage::empty().coverage(), 0.0);
    }

    #[test]
    fn png_roundtrip_preserves_selection() {
        // 造一张 PNG 再解回来，确认灰度取值没被转换吃掉
        let mut img = image::GrayImage::from_pixel(8, 8, image::Luma([0u8]));
        for y in 2..6 {
            for x in 2..6 {
                img.put_pixel(x, y, image::Luma([255u8]));
            }
        }
        let mut buf = Vec::new();
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();

        let m = MaskImage::from_png(&buf).unwrap();
        assert_eq!((m.width(), m.height()), (8, 8));
        assert!((m.coverage() - 0.25).abs() < 0.01, "覆盖率应约 0.25，实际 {}", m.coverage());
    }

    #[test]
    fn invalid_png_reports_error() {
        assert!(MaskImage::from_png(b"not a png").is_err());
    }
}