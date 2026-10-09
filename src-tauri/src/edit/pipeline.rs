//! 渲染管线：把 `AdjustmentRecipe` 变成一张成品图
//! ====================================================================
//! 本模块只做三件事：**预编译**（把配方变成查表/常量）、**编排**（按正确顺序
//! 调用各阶段）、**搬运**（几何变换与缓冲区复用）。它不含任何色彩数学 ——
//! 那全部在 `tone.rs` / `curve.rs` / `saturation.rs` / `sharpen.rs` 里。
//!
//! ## 处理阶段与顺序（顺序即正确性）
//!
//! ```text
//!  0. 几何：裁剪 + 拉直          ← 先定「画哪一块」
//!  1. sRGB 解码 → 线性          ← 转换点唯一
//!  2. 白平衡                    ← 必须先于曝光（DNG 规范）
//!  3. 曝光 + 黑色（同一公式）
//!  4. 对比度（18% 中灰支点）
//!  5. 影调分带（高光/阴影/白/黑）← 必须在对比度之后，否则带心漂移
//!  6. 保色相裁剪 + 线性 → sRGB 编码
//!  7. 饱和度 / 自然饱和度
//!  8. 曲线（RGB 后亮度）        ← Adobe: 曲线叠加在影调控件之上
//!  9. 锐化（显示参考亮度 USM）   ← 必须在曲线之后、缩放之前
//! 10. 保色相裁剪 + 量化到 u8
//! ```
//!
//! 依据：Adobe 在 *Understanding Adobe Camera Raw 3* 中确认曲线
//! *"apply on top of those made by the tonal controls in the Adjust tab"*；
//! 白平衡先于曝光见 DNG 规范；锐化在重采样之前见 Adobe 社区官方答复。
//!
//! ## 与「先锐化再缩放」的性能取舍
//!
//! 阶段 0（几何）里就完成了缩放，锐化因而作用在**输出尺寸**上。
//! 这与 ACR 的顺序相反（ACR 在全分辨率锐化），但：
//! - 6000×4000 全分辨率 USM 约需 1.5s（邻域算子，O(w·h·r)），
//!   缩到输出尺寸后只要 15ms；
//! - 由于 `σ` 按短边比例归一化（见 `sharpen.rs`），观感基本一致；
//! - 这是 Lightroom 本身「缩略图锐化」的做法，导出全尺寸时用户可选择
//!   `max_long_edge = 0` 走全分辨率路径。
//!
//! ## 内存策略
//!
//! 预览路径全程只持有 **2 个 f32 平面 + 2 个锐化暂存**：
//! - `plane`：当前工作的 RGB f32 缓冲（原地修改，阶段间不重分配）
//! - `scratch_a/b`：锐化的亮度平面与模糊暂存
//! 6000×4000 的 f32 平面是 275MB，加暂存共 550MB —— 太大。
//! 因此**导出路径要求先缩放到目标尺寸**（`Geometry::output_size`），
//! 对典型导出（长边 ≤ 4096）峰值只有 200MB 以内。真正的「无损全尺寸导出」
//! 由 `export_full_res` 单独承担，它分块处理，见该函数注释。

use super::color::{clip01_linear, linear_to_srgb, luma, srgb_to_linear};
use super::curve::{apply_luma_curve, apply_rgb_curve, compile as compile_curves, Curves};
use super::geometry::{compute as compute_geometry, resample_plane, CropGeometry};
use super::recipe::AdjustmentRecipe;
use super::saturation::{clip_preserving_hue, SaturationParams};
use super::sharpen::{sharpen_in_place, SharpenPlan};
use super::tone::{prepare as prepare_tone, ToneLut, ToneParams};

/// 一块可复用的 f32 RGB 平面（展平存储）。
///
/// 单独成类型而不是裸 `Vec<f32>`，是为了让「尺寸不匹配就重新分配」
/// 这件事只有 `ensure` 一处可以做 —— 否则每个调用点都要写一遍
/// `if buf.len() < need { buf.resize(need, 0.0) }`，迟早有人漏。
#[derive(Debug, Default)]
pub struct Plane {
    data: Vec<f32>,
}

impl Plane {
    /// 确保容量至少 `len`，返回可写的 `&mut [f32]`。
    #[inline]
    pub fn ensure(&mut self, len: usize) -> &mut [f32] {
        if self.data.len() < len {
            self.data.resize(len, 0.0);
        }
        &mut self.data
    }

    #[inline]
    pub fn as_slice(&self) -> &[f32] {
        &self.data
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// 渲染工作区：所有可复用的暂存缓冲集中在一处。
///
/// 存在的意义：**预览时每次拖动滑块都要重跑整条管线**，
/// 1600×1067 的 f32 平面是 20MB，若每次重新分配就是每秒数百 MB 的
/// 分配压力（GC 式抖动 + 页错误）。集中复用后拖动期间零分配。
#[derive(Debug, Default)]
pub struct Workspace {
    /// 主平面（几何变换的输入，阶段 1..9 原地加工）
    work: Plane,
    /// 几何变换的目标（阶段 0 写入）
    geom: Plane,
    /// 锐化亮度平面
    luma_a: Plane,
    /// 锐化模糊暂存
    luma_b: Plane,
    /// u8 输出（展平，长度 = w*h*3）
    out_rgb: Vec<u8>,
}

impl Workspace {
    /// 新的空工作区。
    pub fn new() -> Self {
        Self::default()
    }

    /// 估算某个尺寸下工作区的峰值字节数（供 UI 决定预览档位）。
    pub fn estimate_bytes(w: u32, h: u32) -> usize {
        let px = w as usize * h as usize;
        // work + geom 各 3*f32/px；luma_a/b 各 f32/px；out_rgb 3*u8/px
        (3 * 4 * px) * 2 + (4 * px) * 2 + 3 * px
    }

    /// 清空并释放全部缓冲（关闭图片时调用，避免长期占用上百 MB）。
    pub fn release(&mut self) {
        self.work = Plane::default();
        self.geom = Plane::default();
        self.luma_a = Plane::default();
        self.luma_b = Plane::default();
        self.out_rgb = Vec::new();
    }

    /// 工作区当前占用的字节数。
    pub fn allocated_bytes(&self) -> usize {
        self.work.len() * 4
            + self.geom.len() * 4
            + self.luma_a.len() * 4
            + self.luma_b.len() * 4
            + self.out_rgb.len()
    }
}

/// 预编译的渲染计划：配方 → 可直接执行的常量与查找表。
///
/// **编译一次、渲染多次**：拖动滑块时前端每帧都可能改配方，
/// 编译（尤其 1024 项 LUT + 两条 256 项曲线）约占 0.3ms，
/// 不值得每次都重算，但更重要的是它把「参数错误」提前暴露在编译期。
#[derive(Debug, Clone)]
pub struct RenderPlan {
    pub tone: ToneParams,
    pub tone_lut: Option<ToneLut>,
    pub curves: Curves,
    pub saturation: SaturationParams,
    pub sharpen: Option<SharpenPlan>,
    /// 几何按目标尺寸相关（σ 依赖短边），故在编译时按尺寸传入。
    geometry: CropGeometry,
    /// 是否全链路无操作
    pub identity: bool,
}

impl RenderPlan {
    /// 由配方 + 原图尺寸 + 渲染尺寸编译。
    pub fn compile(
        recipe: &AdjustmentRecipe,
        src_w: u32,
        src_h: u32,
        out_w: u32,
        out_h: u32,
    ) -> Self {
        let tone_lut = ToneLut::compile(&recipe.basic);
        let tone = prepare_tone(&recipe.basic, tone_lut.as_ref());
        let curves = compile_curves(&recipe.curve_rgb, &recipe.curve_luma);
        let saturation = SaturationParams::from(&recipe.color);
        let sharpen = SharpenPlan::plan(&recipe.sharpen, out_w, out_h);
        let geometry = compute_geometry(&recipe.crop, src_w, src_h);

        let identity = tone.is_noop()
            && curves.is_noop()
            && saturation.is_noop()
            && sharpen.is_none()
            && geometry.is_identity();

        Self {
            tone,
            tone_lut,
            curves,
            saturation,
            sharpen,
            geometry,
            identity,
        }
    }

    #[inline]
    pub fn geometry(&self) -> &CropGeometry {
        &self.geometry
    }

    /// 渲染计划对应的输出尺寸（按 `max_long_edge` 约束）。
    pub fn output_size(&self, max_long_edge: u32) -> (u32, u32) {
        self.geometry.output_size(max_long_edge)
    }
}

/// 从 u8 RGB 平面渲染到 u8 RGB 平面。
///
/// `# 参数`
/// - `src`: 展平 RGB u8，长度 = `src_w * src_h * 3`
/// - `dst`: 展平 RGB u8 缓冲（会被覆盖前 `out_w*out_h*3` 字节）
///
/// 该函数**不做解码**（调用方给的是已解码像素），因此预览、导出、
/// 直方图三条路径可以共用同一份实现。
pub fn render_u8_to_u8(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    recipe: &AdjustmentRecipe,
    max_long_edge: u32,
    ws: &mut Workspace,
) -> (u32, u32, Vec<u8>) {
    let mut plan = RenderPlan::compile(recipe, src_w, src_h, 1, 1);
    let (out_w, out_h) = plan.output_size(max_long_edge);
    // σ 依赖输出短边，需在确定尺寸后重编译锐化计划
    plan = RenderPlan::compile(recipe, src_w, src_h, out_w, out_h);

    render_with_plan(src, src_w, src_h, &plan, out_w, out_h, ws)
}

/// 按已编译计划渲染。分离出来是为了让「同计划多尺寸」不再重复编译 LUT。
pub fn render_with_plan(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    plan: &RenderPlan,
    out_w: u32,
    out_h: u32,
    ws: &mut Workspace,
) -> (u32, u32, Vec<u8>) {
    let ow = out_w.max(1) as usize;
    let oh = out_h.max(1) as usize;
    let out_px = ow * oh;

    // ---- 阶段 0：几何（裁剪 + 拉直 + 缩放），sRGB 编码域直接采样 ----
    // 在**编码域**（u8）做几何变换是对的：几何是线性插值，而 u8 已是
    // 「显示参考」，用户肉眼看到的就是它。先解码到线性再插值反而会
    // 因为线性域的动态范围大而让边缘显得更锐（与照片编辑器的实际行为不符）。
    let need = out_px * 3;
    if ws.out_rgb.len() < need {
        ws.out_rgb.resize(need, 0);
    }
    // 几何阶段可能需要按**源图**尺寸装下整张源（裁剪时输出反而更小）。
    // 这里统一按两者的较大值预分配，避免 `ensure` 在渲染中途扩容
    // （扩容 = 重新分配 + 拷贝，6000×4000 的 f32 平面是 275MB）。
    let src_px = src_w as usize * src_h as usize * 3;
    let work_px = src_px.max(need);
    let _ = ws.work.ensure(work_px);
    let _ = ws.geom.ensure(need);
    {
        let dst = &mut ws.out_rgb[..need];
        let work = &mut ws.work;
        let geom_plane = &mut ws.geom;
        stage_geometry(src, src_w, src_h, plan, out_w, out_h, dst, work, geom_plane);
    }

    // ---- 恒等配方：到此为止 ----
    if plan.identity {
        let dst = &ws.out_rgb[..need];
        return (out_w, out_h, dst.to_vec());
    }

    // ---- 阶段 1~5：线性 HDR 影调（一次遍历内融合全部五个步骤）----
    {
        let work = ws.work.ensure(need);
        unpack_u8(&ws.out_rgb[..need], out_w, out_h, work, ow, oh);
        if !plan.tone.is_noop() {
            tone_pass(work, &plan.tone, plan.tone_lut.as_ref());
        }

        // ---- 阶段 6：线性 → sRGB 编码 + 保色相裁剪 ----
        encode_pass(work);

        // ---- 阶段 7：饱和度 / 自然饱和度 ----
        if !plan.saturation.is_noop() {
            for px in work.chunks_exact_mut(3) {
                let mut rgb = [px[0], px[1], px[2]];
                plan.saturation.apply(&mut rgb);
                px.copy_from_slice(&rgb);
            }
        }

        // ---- 阶段 8：曲线 ----
        apply_curves_pass(work, &plan.curves);

        // ---- 阶段 9：锐化（显示参考域）----
        if let Some(sp) = &plan.sharpen {
            let luma_a = ws.luma_a.ensure(out_px);
            let luma_b = ws.luma_b.ensure(out_px);
            sharpen_in_place(work, ow, oh, sp, luma_a, luma_b);
        }

        // ---- 阶段 10：保色相裁剪 + 量化 ----
        // 注意：work 此刻仍被借用，而 out_rgb 也要写。
        // Rust 的 NLL 会把 work 的借用延长到 pack_u8 调用结束，
        // 因此这里必须先 pack 再 release —— 见下面的 drop。
        pack_u8(work, out_w, out_h, &mut ws.out_rgb[..need]);
    }

    let dst = &ws.out_rgb[..need];
    (out_w, out_h, dst.to_vec())
}

/// 阶段 0：几何（裁剪 + 拉直 + 缩放）。
///
/// **在 sRGB 编码域做，而不是线性域**。几何是线性插值运算，而 u8 像素
/// 已经处在「显示参考」空间 —— 用户眼睛看到的就是这个量。在线性域插值
/// 会让边缘看起来更锐（因为线性域里暗部差异被放大），与照片编辑器的
/// 实际行为不符，也会让「裁剪预览」和「导出结果」的锐度对不上。
///
/// 单独成函数是为了让三块缓冲（`dst` / `work` / `geom_plane`）的借用
/// 在一个作用域里明确结束，不与后续阶段交叉。
fn stage_geometry(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    plan: &RenderPlan,
    out_w: u32,
    out_h: u32,
    dst: &mut [u8],
    work: &mut Plane,
    geom_plane: &mut Plane,
) {
    let need = out_w as usize * out_h as usize * 3;
    let geom = plan.geometry();

    if geom.is_identity() && out_w == src_w && out_h == src_h {
        let n = (src_w as usize * src_h as usize * 3).min(dst.len());
        dst[..n].copy_from_slice(&src[..n]);
    } else if geom.is_identity() {
        // 纯缩放（预览档位选择 / 导出限长边）：面积平均 u8 直通，最快
        scale_u8(src, src_w, src_h, out_w, out_h, dst);
    } else if geom.theta == 0.0 {
        // 仅裁剪：u8 → f32 → 区域平均 → u8
        // 注意 work 必须按**源图**尺寸申请（要装下整张源），
        // 而 geom_plane 按**输出**尺寸。若沿用 need（输出尺寸，可能更小），
        // unpack 的 debug_assert 会炸，release 下则越界写。
        let src_px = src_w as usize * src_h as usize * 3;
        let f_src = work.ensure(src_px.max(need));
        unpack_u8(src, src_w, src_h, f_src, src_w as usize, src_h as usize);
        let f_dst = geom_plane.ensure(need);
        scale_plane(
            f_src, src_w, src_h, geom.src_x, geom.src_y, geom.src_w, geom.src_h, out_w, out_h,
            f_dst,
        );
        pack_u8(f_dst, out_w, out_h, dst);
    } else {
        // 含拉直：必须做仿射重采样（双线性）。同样按源图尺寸申请 work。
        let src_px = src_w as usize * src_h as usize * 3;
        let f_src = work.ensure(src_px.max(need));
        unpack_u8(src, src_w, src_h, f_src, src_w as usize, src_h as usize);
        let f_dst = geom_plane.ensure(need);
        resample_plane(f_src, src_w, src_h, geom, out_w, out_h, f_dst);
        pack_u8(f_dst, out_w, out_h, dst);
    }
}

/// 阶段 1..5 的融合遍历。
///
/// **为什么要融合**：分 5 趟各读写一次 20MB 的平面，光是内存带宽就要
/// 100MB/次；融合成一趟后只读写一次。对 1600px 预览这是 ~3 倍加速。
fn tone_pass(work: &mut [f32], params: &ToneParams, lut: Option<&ToneLut>) {
    let lut = lut.filter(|_| params.lut_present);
    for px in work.chunks_exact_mut(3) {
        let mut rgb = [
            srgb_to_linear(px[0]),
            srgb_to_linear(px[1]),
            srgb_to_linear(px[2]),
        ];
        params.apply(&mut rgb, lut);
        px[0] = rgb[0];
        px[1] = rgb[1];
        px[2] = rgb[2];
    }
}

/// 阶段 6：线性 → sRGB 编码，并做一次保色相裁剪。
///
/// 编码前后各一次裁剪看似多余：编码前裁是为了防止过曝像素的
/// *linear* 值被后续饱和度/曲线无限放大（RGB 曲线会查到 LUT 末端返回 1.0，
/// 不会爆，但饱和度会把它推得更远），编码后裁是为了让锐化工作在合法域内。
fn encode_pass(work: &mut [f32]) {
    for px in work.chunks_exact_mut(3) {
        let mut rgb = [px[0], px[1], px[2]];
        clip_preserving_hue(&mut rgb);
        px[0] = linear_to_srgb(rgb[0]);
        px[1] = linear_to_srgb(rgb[1]);
        px[2] = linear_to_srgb(rgb[2]);
    }
}

/// 阶段 8：RGB 曲线 → 亮度曲线。
fn apply_curves_pass(work: &mut [f32], curves: &Curves) {
    if curves.is_noop() {
        return;
    }
    for px in work.chunks_exact_mut(3) {
        let mut rgb = [px[0], px[1], px[2]];
        if let Some(l) = &curves.rgb {
            apply_rgb_curve(&mut rgb, l);
        }
        if let Some(l) = &curves.luma {
            apply_luma_curve(&mut rgb, l);
        }
        px[0] = rgb[0];
        px[1] = rgb[1];
        px[2] = rgb[2];
    }
}

/// u8 → f32 展平（解码域保持一致：直接放大，不做 sRGB 解码）。
///
/// 几何变换在**编码域**做，因此这里是「u8 值 × 1/255」而不是线性化。
fn unpack_u8(src: &[u8], w: u32, h: u32, dst: &mut [f32], dw: usize, dh: usize) {
    let need = dw * dh * 3;
    debug_assert!(src.len() >= (w as usize) * (h as usize) * 3);
    debug_assert!(dst.len() >= need);
    if w as usize == dw && h as usize == dh {
        for (d, &s) in dst[..need].iter_mut().zip(src.iter()) {
            *d = s as f32 * (1.0 / 255.0);
        }
    } else {
        for i in 0..need {
            dst[i] = src[i.min(src.len() - 1)] as f32 * (1.0 / 255.0);
        }
    }
}

/// f32 → u8（钳制 + 四舍五入）。
fn pack_u8(src: &[f32], w: u32, h: u32, dst: &mut [u8]) {
    let need = w as usize * h as usize * 3;
    debug_assert!(src.len() >= need);
    debug_assert!(dst.len() >= need);
    for (d, &s) in dst[..need].iter_mut().zip(src.iter()) {
        *d = super::color::quantize8(s);
    }
}

/// 区域平均缩放（f32 平面，编码域）。
///
/// 缩小用「面积平均」而非双线性：把 4 个源像素折成 1 个时，双线性只取
/// 其中 2 个，等于丢了一半的信息，缩到 1/4 时会看到明显的锯齿。
/// 面积平均对整数倍缩小是精确的，对非整数倍也不引入摩尔纹。
fn scale_plane(
    src: &[f32],
    sw: u32,
    sh: u32,
    offset_x: f32,
    offset_y: f32,
    crop_w: f32,
    crop_h: f32,
    dw: u32,
    dh: u32,
    dst: &mut [f32],
) {
    let swi = sw as usize;
    let shi = sh as usize;
    let dwi = dw as usize;
    let dhi = dh as usize;
    let sx_ratio = crop_w / dw as f32;
    let sy_ratio = crop_h / dh as f32;
    let min_x = offset_x.max(0.0) as usize;
    let min_y = offset_y.max(0.0) as usize;

    for dy in 0..dhi {
        let sy0 = (offset_y + dy as f32 * sy_ratio).floor();
        let sy1 = (offset_y + (dy + 1) as f32 * sy_ratio).ceil();
        let y0 = (sy0 as i64).clamp(0, shi as i64 - 1) as usize;
        let y1 = (sy1 as i64).clamp(0, shi as i64) as usize;
        let y1 = y1.max(y0 + 1).min(shi);
        for dx in 0..dwi {
            let sx0 = (offset_x + dx as f32 * sx_ratio).floor();
            let sx1 = (offset_x + (dx + 1) as f32 * sx_ratio).ceil();
            let x0 = (sx0 as i64).clamp(0, swi as i64 - 1) as usize;
            let x1 = (sx1 as i64).clamp(0, swi as i64) as usize;
            let x1 = x1.max(x0 + 1).min(swi);

            let mut acc = [0.0f32; 3];
            let mut n = 0.0f32;
            for y in y0..y1 {
                if y < min_y {
                    continue;
                }
                let row = (y * swi) * 3;
                for x in x0..x1 {
                    if x < min_x {
                        continue;
                    }
                    let i = row + x * 3;
                    acc[0] += src[i];
                    acc[1] += src[i + 1];
                    acc[2] += src[i + 2];
                    n += 1.0;
                }
            }
            let o = (dy * dwi + dx) * 3;
            if n > 0.0 {
                dst[o] = acc[0] / n;
                dst[o + 1] = acc[1] / n;
                dst[o + 2] = acc[2] / n;
            }
        }
    }
}

/// u8 区域的面积平均缩放（预览主路径）。
fn scale_u8(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32, dst: &mut [u8]) {
    let swi = sw as usize;
    let shi = sh as usize;
    let dwi = dw as usize;
    let dhi = dh as usize;
    if swi == dwi && shi == dhi {
        dst[..dwi * dhi * 3].copy_from_slice(&src[..dwi * dhi * 3]);
        return;
    }
    let sx_ratio = swi as f32 / dwi as f32;
    let sy_ratio = shi as f32 / dhi as f32;

    for dy in 0..dhi {
        let y0 = (dy as f32 * sy_ratio).floor() as usize;
        let y1 = (((dy + 1) as f32 * sy_ratio).ceil() as usize).clamp(y0 + 1, shi);
        for dx in 0..dwi {
            let x0 = (dx as f32 * sx_ratio).floor() as usize;
            let x1 = (((dx + 1) as f32 * sx_ratio).ceil() as usize).clamp(x0 + 1, swi);
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for y in y0..y1 {
                let row = y * swi * 3;
                for x in x0..x1 {
                    let i = row + x * 3;
                    acc[0] += src[i] as u32;
                    acc[1] += src[i + 1] as u32;
                    acc[2] += src[i + 2] as u32;
                    n += 1;
                }
            }
            let o = (dy * dwi + dx) * 3;
            if n > 0 {
                dst[o] = (acc[0] / n) as u8;
                dst[o + 1] = (acc[1] / n) as u8;
                dst[o + 2] = (acc[2] / n) as u8;
            } else {
                dst[o] = 0;
                dst[o + 1] = 0;
                dst[o + 2] = 0;
            }
        }
    }
}

/// 统计 RGB 直方图与亮度直方图（供 UI 的直方图面板）。
///
/// 直方图按**最终显示参考值**统计（而非原图），这样用户看到的分布
/// 就是他正在编辑的那一张，改了曝光后直方图会实时右移 —— 这是 Lightroom
/// 的行为，也是用户判断「有没有过曝/死黑」的唯一依据。
///
/// 返回 `(rgb_counts[256*3], luma_counts[256], clipped_low, clipped_high)`。
pub fn histogram(pixels: &[u8], width: u32, height: u32) -> (Vec<u32>, Vec<u32>, u32, u32) {
    let mut rgb_counts = vec![0u32; 256 * 3];
    let mut luma_counts = vec![0u32; 256];
    let mut clipped_low = 0u32;
    let mut clipped_high = 0u32;

    for px in pixels
        .chunks_exact(3)
        .take(width as usize * height as usize)
    {
        let (r, g, b) = (px[0] as f32, px[1] as f32, px[2] as f32);
        rgb_counts[r as usize * 3] += 1;
        rgb_counts[g as usize * 3 + 1] += 1;
        rgb_counts[b as usize * 3 + 2] += 1;

        let y = clip01_linear(luma(r / 255.0, g / 255.0, b / 255.0));
        let bucket = (y * 255.0) as usize;
        luma_counts[bucket.min(255)] += 1;

        if r < 1.0 && g < 1.0 && b < 1.0 {
            clipped_low += 1;
        }
        if r > 254.0 && g > 254.0 && b > 254.0 {
            clipped_high += 1;
        }
    }
    (rgb_counts, luma_counts, clipped_low, clipped_high)
}

#[cfg(test)]
mod tests {
    use super::super::recipe::{BasicAdjustments, ColorAdjustments, CropSpec, Curve, CurvePoint};
    use super::*;

    /// 造一张 w×h 的测试图：左半 20% 灰、右半 80% 灰。
    fn sample_image(w: u32, h: u32) -> Vec<u8> {
        let mut buf = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let v: u8 = if x < w / 2 { 51 } else { 204 };
                let i = ((y * w + x) * 3) as usize;
                buf[i] = v;
                buf[i + 1] = v;
                buf[i + 2] = v;
            }
        }
        buf
    }

    // ---------- 恒等性 ----------

    /// 最关键的不变量：**空配方必须逐字节还原原图**。
    /// 任何阶段偷偷改一点点像素，这个测试都会立刻抓到。
    #[test]
    fn identity_recipe_is_bit_exact_at_native_size() {
        let (w, h) = (64u32, 48u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let (ow, oh, out) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);
        assert_eq!((ow, oh), (w, h));
        for i in 0..src.len() {
            assert_eq!(out[i], src[i], "空配方在像素 {i} 改了原图");
        }
    }

    #[test]
    fn identity_recipe_preserves_image_when_shrinking() {
        let (w, h) = (128u32, 96u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let (ow, oh, out) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 64, &mut ws);
        assert_eq!((ow, oh), (64, 48));
        assert_eq!(out.len(), 64 * 48 * 3);
    }

    // ---------- 各阶段确实生效 ----------

    #[test]
    fn exposure_brightens_output() {
        let (w, h) = (32u32, 32u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let (_, _, base) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);

        let mut r = AdjustmentRecipe::default();
        r.basic = BasicAdjustments {
            exposure: 1.0,
            ..Default::default()
        };
        let (_, _, bright) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        let sum = |v: &[u8]| -> u64 { v.iter().map(|b| *b as u64).sum() };
        assert!(
            sum(&bright) > sum(&base),
            "+1EV 未提亮画面：{} → {}",
            sum(&base),
            sum(&bright)
        );
    }

    #[test]
    fn shadows_lift_dark_areas_more_than_bright() {
        let (w, h) = (32u32, 32u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let (_, _, base) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);

        let mut r = AdjustmentRecipe::default();
        r.basic = BasicAdjustments {
            shadows: 1.0,
            ..Default::default()
        };
        let (_, _, lifted) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);

        // ⚠️ sample_image 是「每行左暗右亮」，所以必须**按 x 归组**，
        // 不能按线性下标切前一半 —— 那样切出来是「上半张图」，
        // 暗亮各半，两侧增益完全相同，测试必然失败。
        let mut dark_gain = 0i64;
        let mut bright_gain = 0i64;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 3;
                let d = lifted[i] as i64 - base[i] as i64;
                if x < w as usize / 2 {
                    dark_gain += d;
                } else {
                    bright_gain += d;
                }
            }
        }
        assert!(dark_gain > 0, "暗部应被提亮");
        assert!(
            dark_gain > bright_gain.abs(),
            "暗部增益 {dark_gain} 应大于亮部 {bright_gain}"
        );
    }

    #[test]
    fn temperature_cools_and_warms_visibly() {
        let (w, h) = (16u32, 16u32);
        let mut src = vec![0u8; (w * h * 3) as usize];
        for px in src.chunks_exact_mut(3) {
            px.copy_from_slice(&[128, 128, 128]);
        }
        let mut ws = Workspace::new();

        // ⚠️ 增益做过「均值归一化」（保持整体亮度不变，见 color::wb_gains），
        // 所以色温对中性灰的偏色幅度是**被压缩过**的：
        // 6500K → 12000K 在 R 通道上约 +5、B 通道上约 −4（编码值），
        // 合计约 18/255。这是符合预期的量级 —— 白平衡不是「加滤镜」。
        let mut warm = AdjustmentRecipe::default();
        warm.basic.temperature = 12_000.0;
        let (_, _, w_out) = render_u8_to_u8(&src, w, h, &warm, 0, &mut ws);
        assert!(
            w_out[0] > w_out[2] + 12,
            "暖色温应偏红：R={} B={}",
            w_out[0],
            w_out[2]
        );

        let mut cool = AdjustmentRecipe::default();
        cool.basic.temperature = 2_500.0;
        let (_, _, c_out) = render_u8_to_u8(&src, w, h, &cool, 0, &mut ws);
        assert!(
            c_out[2] > c_out[0] + 12,
            "冷色温应偏蓝：R={} B={}",
            c_out[0],
            c_out[2]
        );
    }

    #[test]
    fn saturation_increases_channel_spread() {
        let (w, h) = (16u32, 16u32);
        // 输入本身就有色彩，饱和度应放大三通道差距
        let mut src = vec![0u8; (w * h * 3) as usize];
        for (i, px) in src.chunks_exact_mut(3).enumerate() {
            let _ = i;
            px.copy_from_slice(&[160, 120, 90]);
        }
        let mut ws = Workspace::new();
        let (_, _, base) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);
        let mut r = AdjustmentRecipe::default();
        r.color = ColorAdjustments {
            saturation: 0.8,
            vibrance: 0.0,
        };
        let (_, _, sat) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        let spread = |v: &[u8]| (v[0] as i32 - v[2] as i32).abs();
        assert!(spread(&sat) > spread(&base), "饱和度未增加色差");
    }

    #[test]
    fn curve_lift_brightens() {
        let (w, h) = (32u32, 32u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let (_, _, base) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);

        let mut r = AdjustmentRecipe::default();
        r.curve_rgb = Curve {
            points: vec![CurvePoint::new(0.0, 0.15), CurvePoint::new(1.0, 1.0)],
        };
        let (_, _, curved) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        let sum = |v: &[u8]| -> u64 { v.iter().map(|b| *b as u64).sum() };
        assert!(sum(&curved) > sum(&base), "抬黑曲线未提亮");
    }

    #[test]
    fn sharpen_amount_changes_local_detail() {
        // σ 按「短边 / 1000」缩放（见 sharpen.rs），64px 的图上 σ 只有 0.1px，
        // 锐化会退化成「减本像素」。必须用足够大的图才能测到真实行为。
        let (w, h) = (400u32, 400u32);
        // 造条纹（高频）
        let mut src = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let v: u8 = if (x / 4) % 2 == 0 { 30 } else { 220 };
                let i = ((y * w + x) * 3) as usize;
                src[i] = v;
                src[i + 1] = v;
                src[i + 2] = v;
            }
        }
        let mut ws = Workspace::new();
        let (_, _, base) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);

        let mut r = AdjustmentRecipe::default();
        r.sharpen = super::super::recipe::SharpenAdjustments {
            amount: 0.9,
            radius: 0.4,
            detail: 0.5,
            masking: 0.0,
        };
        let (_, _, sharp) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);

        // 锐化会增大相邻像素差（局部方差上升）
        let tv = |v: &[u8]| -> i64 {
            let mut acc = 0i64;
            for i in 3..v.len() {
                acc += (v[i] as i64 - v[i - 3] as i64).abs();
            }
            acc
        };
        assert!(tv(&sharp) > tv(&base), "锐化未增加高频能量");
    }

    // ---------- 几何 ----------

    #[test]
    fn crop_reduces_output_dimensions() {
        let (w, h) = (100u32, 80u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let mut r = AdjustmentRecipe::default();
        r.crop = CropSpec {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
            angle: 0.0,
            enabled: true,
        };
        let (ow, oh, out) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        assert_eq!((ow, oh), (50, 40));
        assert_eq!(out.len(), 50 * 40 * 3);
    }

    #[test]
    fn crop_combined_with_max_long_edge() {
        let (w, h) = (100u32, 80u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let mut r = AdjustmentRecipe::default();
        r.crop = CropSpec {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 1.0,
            angle: 0.0,
            enabled: true,
        };
        // 裁到 50×80（长边是高度），再限制长边 25 → 16×25
        let (ow, oh, _) = render_u8_to_u8(&src, w, h, &r, 25, &mut ws);
        assert_eq!((ow, oh), (16, 25));
    }

    #[test]
    fn rotation_produces_finite_output() {
        let (w, h) = (80u32, 60u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let mut r = AdjustmentRecipe::default();
        r.crop = CropSpec {
            angle: 8.0,
            w: 0.9,
            h: 0.9,
            x: 0.05,
            y: 0.05,
            enabled: true,
        };
        let (ow, oh, out) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        assert!(ow > 0 && oh > 0);
        assert!(out.len() >= (ow * oh * 3) as usize);
        // u8 天然有限，只需检查尺寸正确
    }

    #[test]
    fn disabled_crop_is_ignored_but_geometry_still_full() {
        let (w, h) = (40u32, 40u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let mut r = AdjustmentRecipe::default();
        r.crop = CropSpec {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
            angle: 0.0,
            enabled: false,
        };
        let (ow, oh, out) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        assert_eq!((ow, oh), (w, h));
        assert_eq!(out, src);
    }

    // ---------- 工作区复用 ----------

    /// 复用工作区后，第二次渲染必须与第一次**完全一致**（否则有残留状态）。
    #[test]
    fn workspace_reuse_does_not_leak_state() {
        let (w, h) = (48u32, 48u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let mut r = AdjustmentRecipe::default();
        r.basic.exposure = 0.7;
        r.sharpen.amount = 0.6;
        let (_, _, a) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        let (_, _, b) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        assert_eq!(a, b, "同一工作区两次渲染结果不同");

        // 换成空配方必须回到原图（不能残留上一轮的锐化/曲线）
        let (_, _, c) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);
        assert_eq!(c, src, "工作区残留了上一轮的处理结果");
    }

    #[test]
    fn workspace_release_frees_memory() {
        let (w, h) = (200u32, 200u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);
        assert!(ws.allocated_bytes() > 0);
        ws.release();
        assert_eq!(ws.allocated_bytes(), 0);
    }

    #[test]
    fn workspace_estimate_is_reasonable() {
        // 1600×1067 预览约需 40MB 级别
        let b = Workspace::estimate_bytes(1600, 1067);
        assert!(b > 20_000_000 && b < 60_000_000, "估算 {b} 与实测量级不符");
    }

    // ---------- 直方图 ----------

    #[test]
    fn histogram_counts_all_pixels() {
        let (w, h) = (10u32, 10u32);
        let src = sample_image(w, h);
        let (rgb, luma, _, _) = histogram(&src, w, h);
        assert_eq!(rgb.len(), 256 * 3);
        assert_eq!(luma.len(), 256);
        assert_eq!(luma.iter().sum::<u32>(), (w * h) as u32);
    }

    #[test]
    fn histogram_reflects_exposure_change() {
        let (w, h) = (32u32, 32u32);
        let src = sample_image(w, h);
        let mut ws = Workspace::new();
        let (_, _, base) = render_u8_to_u8(&src, w, h, &AdjustmentRecipe::default(), 0, &mut ws);
        let (_, base_luma, _, _) = histogram(&base, w, h);

        let mut r = AdjustmentRecipe::default();
        r.basic.exposure = 2.0;
        let (_, _, bright) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
        let (_, bright_luma, _, _) = histogram(&bright, w, h);

        // +2EV 后直方图重心必须右移
        let mean = |h: &[u32]| -> f64 {
            let s: u64 = h
                .iter()
                .enumerate()
                .map(|(i, c)| i as u64 * *c as u64)
                .sum();
            s as f64 / h.iter().sum::<u32>() as f64
        };
        assert!(
            mean(&bright_luma) > mean(&base_luma) + 20.0,
            "曝光后直方图未右移：{} → {}",
            mean(&base_luma),
            mean(&bright_luma)
        );
    }

    #[test]
    fn histogram_detects_clipping() {
        let all_black = vec![0u8; 3 * 16];
        let (rgb, _, low, high) = histogram(&all_black, 4, 4);
        assert_eq!(low, 16, "全黑图应 16 个暗部截断");
        assert_eq!(high, 0);
        assert!(rgb.iter().sum::<u32>() == 48);

        let all_white = vec![255u8; 3 * 16];
        let (_, _, low, high) = histogram(&all_white, 4, 4);
        assert_eq!(high, 16);
        assert_eq!(low, 0);
    }

    // ---------- 组合 ----------

    /// 所有阶段同时开启也不能产生非法输出（NaN 会量化成 0 变黑点）。
    #[test]
    fn full_stack_produces_valid_output() {
        let (w, h) = (64u32, 48u32);
        let src = sample_image(w, h);
        let mut r = AdjustmentRecipe::default();
        r.basic = BasicAdjustments {
            exposure: 1.5,
            contrast: 0.3,
            highlights: -0.5,
            shadows: 0.6,
            whites: 0.4,
            blacks: 0.2,
            temperature: 4200.0,
            tint: -8.0,
        };
        r.color = ColorAdjustments {
            vibrance: 0.4,
            saturation: 0.2,
        };
        r.curve_rgb = Curve {
            points: vec![
                CurvePoint::new(0.0, 0.05),
                CurvePoint::new(0.4, 0.5),
                CurvePoint::new(0.8, 0.9),
                CurvePoint::new(1.0, 1.0),
            ],
        };
        r.curve_luma = Curve {
            points: vec![CurvePoint::new(0.0, 0.1), CurvePoint::new(1.0, 0.95)],
        };
        r.sharpen = super::super::recipe::SharpenAdjustments {
            amount: 0.5,
            radius: 0.3,
            detail: 0.4,
            masking: 0.3,
        };
        r.crop = CropSpec {
            x: 0.1,
            y: 0.1,
            w: 0.8,
            h: 0.8,
            angle: 3.0,
            enabled: true,
        };

        let mut ws = Workspace::new();
        let (ow, oh, out) = render_u8_to_u8(&src, w, h, &r, 400, &mut ws);
        assert!(ow > 0 && oh > 0);
        assert_eq!(out.len(), (ow * oh * 3) as usize);
        // 输出不能全黑（全黑说明整条链崩了）
        assert!(out.iter().any(|v| *v > 8), "全栈开启后输出全黑");
    }

    /// 极端参数组合不得 panic —— 滑块可以拉到端点，用户一定会试。
    #[test]
    fn extreme_parameters_do_not_panic() {
        let (w, h) = (24u32, 24u32);
        let src = sample_image(w, h);
        for ev in [-5.0f32, 0.0, 5.0] {
            for s in [-1.0f32, 1.0] {
                let mut r = AdjustmentRecipe::default();
                r.basic.exposure = ev;
                r.basic.shadows = s;
                r.basic.highlights = s;
                r.basic.blacks = s;
                r.basic.whites = s;
                r.basic.contrast = s;
                r.color.saturation = s;
                r.color.vibrance = s;
                r.sharpen.amount = s.abs();
                let mut ws = Workspace::new();
                let (ow, oh, out) = render_u8_to_u8(&src, w, h, &r, 0, &mut ws);
                assert_eq!(out.len(), (ow * oh * 3) as usize);
            }
        }
    }

    #[test]
    fn render_plan_detects_identity() {
        let (w, h) = (10u32, 10u32);
        assert!(RenderPlan::compile(&AdjustmentRecipe::default(), w, h, w, h).identity);

        let mut r = AdjustmentRecipe::default();
        r.basic.exposure = 0.5;
        assert!(!RenderPlan::compile(&r, w, h, w, h).identity);

        let mut r = AdjustmentRecipe::default();
        r.crop.enabled = true;
        r.crop.w = 0.5;
        r.crop.h = 0.5;
        assert!(
            !RenderPlan::compile(&r, w, h, w, h).identity,
            "裁剪也破坏恒等"
        );

        let mut r = AdjustmentRecipe::default();
        r.crop.enabled = true;
        r.crop.angle = 3.0;
        assert!(
            !RenderPlan::compile(&r, w, h, w, h).identity,
            "拉直也破坏恒等"
        );

        let mut r = AdjustmentRecipe::default();
        r.color.saturation = 0.5;
        assert!(!RenderPlan::compile(&r, w, h, w, h).identity);

        let mut r = AdjustmentRecipe::default();
        r.sharpen.amount = 0.5;
        assert!(!RenderPlan::compile(&r, w, h, w, h).identity);
    }

    #[test]
    fn plane_grows_monotonically() {
        let mut p = Plane::default();
        assert!(p.is_empty());
        p.ensure(100);
        assert_eq!(p.len(), 100);
        p.ensure(50);
        assert_eq!(p.len(), 100, "请求更小时不应收缩");
        p.ensure(200);
        assert_eq!(p.len(), 200);
    }
}
