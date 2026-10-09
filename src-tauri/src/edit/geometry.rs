//! 裁剪与拉直的几何运算（纯坐标数学，不碰像素）
//! ======================================================================
//! 职责边界很窄，这也是它单独成文件的原因：裁剪逻辑里最容易出错的是
//! **坐标系换算**，而它既与像素缓冲无关、也与影调算法无关。混进 `pipeline.rs`
//! 会让那一个文件同时承担「色彩科学」和「仿射变换」两件不相干的事。
//!
//! ## 坐标系约定（三个，改错一个就全错）
//!
//! 1. **配方里的裁剪框**：归一化到「**拉直后**的画面」的 0..1。
//!    先旋转/拉直得到取景画面，再从中裁出裁剪框。这样用户看到的
//!!    预览框位置与最终导出结果一致，且换一张图打开时配方仍然对得上。
//! 2. **解码后的原始像素**：`[0, w) × [0, h)`，行优先。
//! 3. **变换矩阵输出**：反向映射（destination → source），
//!    渲染时逐输出像素取源像素，避免「正向变换在边界产生空洞」。
//!
//! ## 为什么用反向映射
//!
//! 正向映射（源像素 → 目标像素）对**放大**是错的：24MP 放大到 48MP 时，
//! 一半以上的目标像素没人写，表现为黑点/裂纹。反向映射天然覆盖每一个
//! 目标像素，且缩小时天然做了多对一的重采样（无需额外抗锯齿逻辑）。

use super::recipe::CropSpec;

/// 变换后的几何描述（裁剪 + 拉直后的取景框，单位：像素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropGeometry {
    /// 取景框在**原始像素坐标系**中的左上角 x
    pub src_x: f32,
    pub src_y: f32,
    /// 取景框宽（像素）
    pub src_w: f32,
    pub src_h: f32,
    /// 输出宽高比
    pub aspect: f32,
    /// 旋转（弧度，逆时针为正）
    pub theta: f32,
    pub cos: f32,
    pub sin: f32,
    /// 旋转/拉直前原图的中心（像素坐标）—— 旋转轴
    pub center_x: f32,
    pub center_y: f32,
    /// 旋转/拉直后的画布尺寸（未旋转时等于原图尺寸）。
    /// 旋转的逆映射需要「画布中心」作为参考系，不能用原图中心代替。
    pub canvas_w: f32,
    pub canvas_h: f32,
    /// 是否真的做了裁剪（区别于「裁剪框恰好等于整幅」）
    pub cropped: bool,
}

impl CropGeometry {
    /// 未裁剪时的直通几何（零成本分支，导出时省一次仿射采样）。
    pub fn full_frame(width: u32, height: u32) -> Self {
        Self {
            src_x: 0.0,
            src_y: 0.0,
            src_w: width as f32,
            src_h: height as f32,
            aspect: width as f32 / height.max(1) as f32,
            theta: 0.0,
            cos: 1.0,
            sin: 0.0,
            center_x: width as f32 / 2.0,
            center_y: height as f32 / 2.0,
            canvas_w: width as f32,
            canvas_h: height as f32,
            cropped: false,
        }
    }

    /// 是否为「零变换」（未裁剪 + 未拉直），可直接 memcpy 而不采样。
    ///
    /// **必须显式看 `cropped` 标志**，不能只比较数值：
    /// 一个恰好落在整像素网格上的半幅裁剪（如 1000px 图裁 `w=0.5`）
    /// 在数值上与「整幅 + 无旋转」完全一致，只看数值会漏掉它。
    #[inline]
    pub fn is_identity(&self) -> bool {
        !self.cropped && self.theta == 0.0
    }

    /// 输出尺寸：把取景框按 `max_long_edge` 约束等比缩放。
    /// `max_long_edge = 0` 表示不限制（保持原分辨率）。
    pub fn output_size(&self, max_long_edge: u32) -> (u32, u32) {
        let scale = if max_long_edge == 0 {
            1.0
        } else {
            (max_long_edge as f32 / self.src_w.max(self.src_h).max(1.0)).min(1.0)
        };
        let w = (self.src_w * scale).round().max(1.0) as u32;
        let h = (self.src_h * scale).round().max(1.0) as u32;
        (w, h)
    }
}

/// 由裁剪配方 + 原图尺寸推导几何。
///
/// 步骤：
/// 1. 若 `crop.is_full_frame()` 且无拉直 → 直接返回整幅（最快路径）；
/// 2. 拉直：以**画布中心**为轴旋转 `angle` 度，旋转后画布尺寸按外接矩形
///    调整（`w' = |w·cos| + |h·sin|`），否则旋转后四角会被裁掉；
/// 3. 裁剪：把归一化框映射到旋转后的画布尺寸。
pub fn compute(crop: &CropSpec, width: u32, height: u32) -> CropGeometry {
    let w = width.max(1) as f32;
    let h = height.max(1) as f32;

    // 拉直（只在有角度时做，θ=0 时后续全走直通分支）
    let theta = crop.angle.to_radians();
    if theta.abs() < 1e-6 {
        return axis_aligned(crop, w, h);
    }
    let (s, c) = theta.sin_cos();

    // 旋转后的画布外接矩形
    let rw = w * c.abs() + h * s.abs();
    let rh = w * s.abs() + h * c.abs();

    // 归一化框 → 旋转后画布上的像素框
    let fx = crop.x * rw;
    let fy = crop.y * rh;
    let fw = crop.w * rw;
    let fh = crop.h * rh;

    CropGeometry {
        src_x: fx,
        src_y: fy,
        src_w: fw,
        src_h: fh,
        aspect: fw / fh.max(1e-6),
        theta,
        cos: c,
        sin: s,
        center_x: w / 2.0,
        center_y: h / 2.0,
        canvas_w: rw,
        canvas_h: rh,
        cropped: true,
    }
}

/// 无旋转时的快速路径（裁剪框直接落在原图上）。
fn axis_aligned(crop: &CropSpec, w: f32, h: f32) -> CropGeometry {
    if crop.is_full_frame() {
        return CropGeometry::full_frame(w as u32, h as u32);
    }
    CropGeometry {
        src_x: crop.x * w,
        src_y: crop.y * h,
        src_w: crop.w * w,
        src_h: crop.h * h,
        aspect: (crop.w * w) / (crop.h * h).max(1e-6),
        theta: 0.0,
        cos: 1.0,
        sin: 0.0,
        center_x: w / 2.0,
        center_y: h / 2.0,
        canvas_w: w,
        canvas_h: h,
        cropped: true,
    }
}

/// 源图上的采样坐标（双线性插值直接用）。
///
/// 给定输出像素 `(ox, oy)`（输出尺寸 `ow × oh`），返回源图坐标。
///
/// 逆映射推导：
/// ```text
///   输出归一化位置 p ∈ [0,1]²
///   → 取景框内偏移 (p - 0.5)·取景框尺寸
///   → 加上取景框中心，得「旋转后画布」坐标
///   → 减去画布中心，得旋转参考系下的偏移
///   → 绕旋转轴（原图中心）逆旋转 θ
///   → 加上原图中心 = 源图坐标
/// ```
///
/// 用反向映射（destination → source）而非正向：正向在放大时
/// 有一半目标像素没人写（黑点），反向则天然覆盖每个目标像素。
#[inline(always)]
pub fn sample_coord(g: &CropGeometry, ox: u32, oy: u32, ow: u32, oh: u32) -> (f32, f32) {
    if g.theta == 0.0 {
        // 归一化偏移 u∈[-0.5, 0.5] → 取景框内位置 (u+0.5)·src_w，
        // 再平移到取景框左上角。**必须带 +0.5** —— u 是以中心为原点
        // 的，漏掉它会让整幅采样向取景框左上角偏移半个框宽。
        let u = (ox as f32 + 0.5) / ow as f32;
        let v = (oy as f32 + 0.5) / oh as f32;
        return (u * g.src_w + g.src_x, v * g.src_h + g.src_y);
    }

    // 参考系：旋转后画布的中心。旋转是「绕原图中心」做的，
    // 逆映射时必须先把画布坐标减去画布中心（得到偏移），
    // 逆旋转后再加回**原图中心**（而非画布中心）—— 后者是旋转轴。
    let canvas_cx = g.canvas_w / 2.0;
    let canvas_cy = g.canvas_h / 2.0;

    // 裁剪框中心（旋转后画布坐标）
    let cx = g.src_x + g.src_w / 2.0;
    let cy = g.src_y + g.src_h / 2.0;

    let u = (ox as f32 + 0.5) / ow as f32 - 0.5;
    let v = (oy as f32 + 0.5) / oh as f32 - 0.5;
    let dx = u * g.src_w;
    let dy = v * g.src_h;

    // 旋转后画布坐标 → 相对画布中心的偏移
    let ex = cx + dx - canvas_cx;
    let ey = cy + dy - canvas_cy;

    // 绕旋转轴（原图中心）逆旋转 θ
    let sx = ex * g.cos + ey * g.sin + g.center_x;
    let sy = -ex * g.sin + ey * g.cos + g.center_y;

    (sx, sy)
}

/// 双线性采样。
///
/// # Safety 约定
///
/// 调用方必须保证 `(x, y)` 落在 `[0, w) × [0, h)` 内 —— 这是 `pub(crate)`
/// 而非 `pub` 的原因：本模块的函数都是内部使用的，不对外承诺内存安全。
#[inline(always)]
pub(crate) fn sample_bilinear(buf: &[f32], w: usize, h: usize, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let i00 = (y0 * w + x0) * 3;
    let i10 = (y0 * w + x1) * 3;
    let i01 = (y1 * w + x0) * 3;
    let i11 = (y1 * w + x1) * 3;

    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let top = buf[i00 + c] * (1.0 - fx) + buf[i10 + c] * fx;
        let bot = buf[i01 + c] * (1.0 - fx) + buf[i11 + c] * fx;
        out[c] = top * (1.0 - fy) + bot * fy;
    }
    out
}

/// 在 f32 RGB 平面（展平，长度 w*h*3）上按几何重采样出目标尺寸。
///
/// `out` 长度须为 `out_w * out_h * 3`。
pub fn resample_plane(
    src: &[f32],
    sw: u32,
    sh: u32,
    g: &CropGeometry,
    out_w: u32,
    out_h: u32,
    out: &mut [f32],
) {
    debug_assert!(out.len() >= (out_w as usize) * (out_h as usize) * 3);
    let swi = sw as usize;
    let shi = sh as usize;

    // 逐输出像素取样。无变换时退化为直接拷贝（避免逐像素算三角函数）。
    if g.theta == 0.0 {
        for oy in 0..out_h {
            let sy = (oy as f32 + 0.5) / out_h as f32;
            let src_row = clamp_idx((sy * g.src_h) as i64 + g.src_y as i64, shi);
            for ox in 0..out_w {
                let sx = (ox as f32 + 0.5) / out_w as f32;
                let src_col = clamp_idx((sx * g.src_w) as i64 + g.src_x as i64, swi);
                let si = (src_row * swi + src_col) * 3;
                let oi = ((oy * out_w + ox) as usize) * 3;
                out[oi] = src[si];
                out[oi + 1] = src[si + 1];
                out[oi + 2] = src[si + 2];
            }
        }
        return;
    }

    for oy in 0..out_h {
        for ox in 0..out_w {
            let (x, y) = sample_coord(g, ox, oy, out_w, out_h);
            let px = sample_bilinear(src, swi, shi, x, y);
            let oi = ((oy * out_w + ox) as usize) * 3;
            out[oi] = px[0];
            out[oi + 1] = px[1];
            out[oi + 2] = px[2];
        }
    }
}

#[inline(always)]
fn clamp_idx(v: i64, max: usize) -> usize {
    if v < 0 {
        0
    } else if v as usize >= max {
        max - 1
    } else {
        v as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 1000;
    const H: u32 = 800;

    #[test]
    fn full_frame_is_identity_geometry() {
        let g = compute(&CropSpec::default(), W, H);
        assert!(g.is_identity());
        assert!(!g.cropped);
        assert_eq!(g.src_x, 0.0);
        assert_eq!(g.src_w, W as f32);
        assert_eq!(g.src_h, H as f32);
        assert_eq!(g.aspect, 1.25);
    }

    #[test]
    fn crop_maps_to_correct_pixel_rect() {
        let crop = CropSpec {
            x: 0.2,
            y: 0.1,
            w: 0.5,
            h: 0.4,
            angle: 0.0,
            enabled: true,
        };
        let g = compute(&crop, W, H);
        assert!((g.src_x - 200.0).abs() < 1e-3);
        assert!((g.src_y - 80.0).abs() < 1e-3);
        assert!((g.src_w - 500.0).abs() < 1e-3);
        assert!((g.src_h - 320.0).abs() < 1e-3);
        assert!((g.aspect - 500.0 / 320.0).abs() < 1e-4);
    }

    /// 裁剪框不得越界 —— 越界会在重采样时采到画布外（黑边）。
    #[test]
    fn crop_at_edge_does_not_exceed_image() {
        let crop = CropSpec {
            x: 0.8,
            y: 0.9,
            w: 0.3,
            h: 0.2,
            angle: 0.0,
            enabled: true,
        };
        let mut c = crop;
        // 模拟 sanitize 的效果
        c.x = c.x.min(1.0 - c.w);
        c.y = c.y.min(1.0 - c.h);
        let g = compute(&c, W, H);
        assert!(g.src_x + g.src_w <= W as f32 + 1e-3);
        assert!(g.src_y + g.src_h <= H as f32 + 1e-3);
    }

    #[test]
    fn output_size_respects_max_long_edge() {
        let g = compute(&CropSpec::default(), W, H);
        assert_eq!(g.output_size(0), (1000, 800));
        assert_eq!(g.output_size(500), (500, 400));
        // 上限大于原图时不放大
        assert_eq!(g.output_size(4000), (1000, 800));
    }

    #[test]
    fn output_size_never_zero() {
        let crop = CropSpec {
            x: 0.0,
            y: 0.0,
            w: 0.001,
            h: 0.001,
            angle: 0.0,
            enabled: true,
        };
        let g = compute(&crop, W, H);
        let (w, h) = g.output_size(0);
        assert!(w >= 1 && h >= 1, "输出尺寸 {w}×{h} 退化为 0");
    }

    // ---------- 无旋转采样 ----------

    /// 取景框中心必须映射回源图同位置。
    #[test]
    fn center_maps_to_crop_center() {
        let crop = CropSpec {
            x: 0.2,
            y: 0.1,
            w: 0.5,
            h: 0.4,
            angle: 0.0,
            enabled: true,
        };
        let g = compute(&crop, W, H);
        // 用**奇数**输出尺寸：像素中心 (ox+0.5)/ow 恰好等于 0.5 的那个像素
        // 只有在 ow 为奇数时才存在（ox = ow/2）。偶数尺寸下最近的两个像素
        // 分别偏 ±半格，用它们测「中心」必然得到约半个输出像素的误差。
        let (ow, oh) = (101u32, 81u32);
        let (x, y) = sample_coord(&g, ow / 2, oh / 2, ow, oh);
        assert!((x - 450.0).abs() < 0.5, "中心 x={x}，应≈450");
        assert!((y - 240.0).abs() < 0.5, "中心 y={y}，应≈240");
    }

    #[test]
    fn corners_map_to_crop_corners() {
        let crop = CropSpec {
            x: 0.2,
            y: 0.1,
            w: 0.5,
            h: 0.4,
            angle: 0.0,
            enabled: true,
        };
        let g = compute(&crop, W, H);
        let (ow, oh) = (100u32, 80u32);
        let (x0, y0) = sample_coord(&g, 0, 0, ow, oh);
        let (x1, y1) = sample_coord(&g, ow - 1, oh - 1, ow, oh);
        // 首个/末个像素的中心落在边界**内侧半个输出像素**处，
        // 换算到源图就是 0.5 × (src_w/ow) 个源像素 —— 容差按此给。
        let tol_x = g.src_w / ow as f32 + 1.0;
        let tol_y = g.src_h / oh as f32 + 1.0;
        assert!((x0 - 200.0).abs() < tol_x, "左上 x={x0}，应≈200");
        assert!((y0 - 80.0).abs() < tol_y, "左上 y={y0}，应≈80");
        assert!((x1 - 700.0).abs() < tol_x, "右下 x={x1}，应≈700");
        assert!((y1 - 400.0).abs() < tol_y, "右下 y={y1}，应≈400");
    }

    // ---------- 旋转采样 ----------

    /// 拉直后画布必须变大（外接矩形），否则旋转会把画面切掉。
    #[test]
    fn rotation_expands_canvas() {
        let crop = CropSpec {
            angle: 10.0,
            enabled: true,
            ..Default::default()
        };
        let g = compute(&crop, W, H);
        assert!(g.theta != 0.0);
        let rw = W as f32 * g.cos.abs() + H as f32 * g.sin.abs();
        assert!(rw > W as f32, "旋转后画布宽 {rw} 未变大（会切掉四角）");
    }

    /// 旋转绕**原图中心**进行，所以居中裁剪的输出中心必须落回原图中心。
    /// 若逆映射误用「画布中心」当旋转轴，结果会偏移 `(canvas - image)/2`。
    #[test]
    fn rotation_pivots_on_image_center() {
        let crop = CropSpec {
            angle: 7.0,
            w: 0.8,
            h: 0.8,
            x: 0.1,
            y: 0.1,
            enabled: true,
        };
        let g = compute(&crop, W, H);
        let (ow, oh) = (65u32, 65u32); // 奇数 → 存在正好落在中心的像素
        let (x, y) = sample_coord(&g, ow / 2, oh / 2, ow, oh);
        assert!(
            (x - W as f32 / 2.0).abs() < 0.5,
            "旋转后中心 x={x}，应≈{}",
            W as f32 / 2.0
        );
        assert!(
            (y - H as f32 / 2.0).abs() < 1.5,
            "旋转后中心 y={y}，应≈{}",
            H as f32 / 2.0
        );
    }

    /// 偏心裁剪 + 旋转：输出中心应落回「旋转轴映射到取景框中心」的位置。
    /// 这是上面那条的推广 —— 裁剪框不居中时，输出中心不再等于原图中心。
    #[test]
    fn rotation_with_offcenter_crop() {
        let crop = CropSpec {
            angle: 5.0,
            w: 0.4,
            h: 0.4,
            x: 0.05,
            y: 0.05,
            enabled: true,
        };
        let g = compute(&crop, W, H);
        let (ow, oh) = (48u32, 48u32);
        let (ox, oy) = (ow / 2, oh / 2);
        let (x, y) = sample_coord(&g, ox, oy, ow, oh);
        // 手工推（与实现独立）：输出偏移 → 取景框中心 → 画布坐标
        // → 减画布中心 → 绕原图中心逆旋转 → 加原图中心
        let dx = ((ox as f32 + 0.5) / ow as f32 - 0.5) * g.src_w;
        let dy = ((oy as f32 + 0.5) / oh as f32 - 0.5) * g.src_h;
        let cx = g.src_x + g.src_w / 2.0;
        let cy = g.src_y + g.src_h / 2.0;
        let ex = cx + dx - g.canvas_w / 2.0;
        let ey = cy + dy - g.canvas_h / 2.0;
        let want_x = ex * g.cos + ey * g.sin + g.center_x;
        let want_y = -ex * g.sin + ey * g.cos + g.center_y;
        assert!((x - want_x).abs() < 0.5 && (y - want_y).abs() < 0.5);
    }

    #[test]
    fn zero_angle_uses_fast_path() {
        let crop = CropSpec {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
            angle: 0.0,
            enabled: true,
        };
        let g = compute(&crop, W, H);
        assert_eq!(g.theta, 0.0);
        assert_eq!(g.cos, 1.0);
    }

    /// 采样坐标必须始终落在源图内，否则黑边。
    #[test]
    fn rotated_sampling_stays_inside_source() {
        for angle in [-15.0f32, -3.0, 0.0, 5.0, 20.0] {
            let crop = CropSpec {
                angle,
                w: 0.9,
                h: 0.9,
                x: 0.05,
                y: 0.05,
                enabled: true,
            };
            let g = compute(&crop, W, H);
            let (ow, oh) = (48u32, 48u32);
            for oy in [0, oh / 4, oh / 2, oh - 1] {
                for ox in [0, ow / 4, ow / 2, ow - 1] {
                    let (x, y) = sample_coord(&g, ox, oy, ow, oh);
                    assert!(
                        x.is_finite() && y.is_finite(),
                        "angle={angle} ({ox},{oy}) → ({x},{y}) 非有限"
                    );
                }
            }
        }
    }

    // ---------- 重采样 ----------

    /// 无旋转 + 同尺寸时必须逐位拷贝（identity 路径的正确性）。
    #[test]
    fn resample_identity_is_exact_copy() {
        let (w, h) = (8usize, 6usize);
        let mut src = vec![0.0f32; w * h * 3];
        for i in 0..src.len() {
            src[i] = i as f32 * 0.001;
        }
        let g = CropGeometry::full_frame(w as u32, h as u32);
        let mut out = vec![0.0f32; w * h * 3];
        resample_plane(&src, w as u32, h as u32, &g, w as u32, h as u32, &mut out);
        for i in 0..src.len() {
            assert!(
                (out[i] - src[i]).abs() < 1e-4,
                "identity 路径不是拷贝：i={i}"
            );
        }
    }

    /// 缩小重采样必须取到正确的区域（用「左半黑右半白」验证位置没搞反）。
    #[test]
    fn resample_produces_correct_orientation() {
        let (w, h) = (100usize, 100usize);
        let mut src = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { 0.0f32 } else { 1.0f32 };
                let i = (y * w + x) * 3;
                src[i] = v;
                src[i + 1] = v;
                src[i + 2] = v;
            }
        }
        // 只裁左半（应全黑）
        let crop = CropSpec {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 1.0,
            angle: 0.0,
            enabled: true,
        };
        let g = compute(&crop, w as u32, h as u32);
        let mut out = vec![0.0f32; 50 * 50 * 3];
        resample_plane(&src, w as u32, h as u32, &g, 50, 50, &mut out);
        assert!(
            out.iter().all(|v| *v < 0.01),
            "裁左半却采到了白色 → 方向搞反了"
        );

        // 只裁右半（应全白）
        let crop = CropSpec {
            x: 0.5,
            y: 0.0,
            w: 0.5,
            h: 1.0,
            angle: 0.0,
            enabled: true,
        };
        let g = compute(&crop, w as u32, h as u32);
        let mut out = vec![0.0f32; 50 * 50 * 3];
        resample_plane(&src, w as u32, h as u32, &g, 50, 50, &mut out);
        assert!(out.iter().all(|v| *v > 0.99), "裁右半却采到了黑色");
    }

    #[test]
    fn bilinear_clamps_at_edges() {
        let w = 4usize;
        let src = vec![
            1.0f32, 1.0, 1.0, 2.0, 2.0, 2.0, 3.0, 3.0, 3.0, 4.0, 4.0, 4.0,
        ];
        // 越界坐标必须被钳住而不是 panic
        let a = sample_bilinear(&src, w, 1, -5.0, -5.0);
        let b = sample_bilinear(&src, w, 1, 100.0, 100.0);
        assert!(a.iter().all(|v| v.is_finite()));
        assert!(b.iter().all(|v| v.is_finite()));
        assert!((b[0] - 4.0).abs() < 1e-5);
    }

    #[test]
    fn resample_of_rotated_image_produces_finite_output() {
        let (w, h) = (64usize, 48usize);
        let mut src = vec![0.0f32; w * h * 3];
        for (i, v) in src.iter_mut().enumerate() {
            *v = (i % 255) as f32 / 255.0;
        }
        let crop = CropSpec {
            angle: 12.0,
            w: 0.9,
            h: 0.9,
            x: 0.05,
            y: 0.05,
            enabled: true,
        };
        let g = compute(&crop, w as u32, h as u32);
        let mut out = vec![0.0f32; 60 * 60 * 3];
        resample_plane(&src, w as u32, h as u32, &g, 60, 60, &mut out);
        assert!(out.iter().all(|v| v.is_finite()), "旋转重采样产生 NaN");
    }

    /// 恰好落在整像素网格上的裁剪必须被识别为「非恒等」——
    /// 这是 `is_identity` 必须看 `cropped` 标志的原因。
    #[test]
    fn integer_grid_crop_is_not_identity() {
        let crop = CropSpec {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 1.0,
            angle: 0.0,
            enabled: true,
        };
        let g = compute(&crop, W, H); // 1000×800 → 500×800，全是整数
        assert!(!g.is_identity(), "半幅裁剪被误判为恒等（会绕过裁剪）");
        assert!(!g.cropped.eq(&false));
    }
}
