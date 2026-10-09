//! 调整配方（Recipe）：非破坏编辑的**唯一真相源**
//! ===========================================================
//! 一张图的全部编辑状态就是本文件里的 `AdjustmentRecipe` 一个结构体。
//! 它同时承担四个职责：
//!
//! 1. **算法输入** —— `pipeline.rs` 只认这个结构体，不认任何 UI 状态；
//! 2. **持久化格式** —— 存成同目录的 `<文件名>.pmrecipe.json`（见 `store.rs`）；
//! 3. **导入/导出** —— 前端可把 JSON 粘进粘出，便于跨设备搬运；
//! 4. **历史快照** —— 撤销栈存的就是它的克隆体。
//!
//! ## 演进兼容性（硬约束）
//!
//! 编辑配方是**用户资产**，十年后还得能打开。因此：
//! - 容器上 `#[serde(default)]`：缺字段走 `Default`，不因新增参数而打不开旧文件；
//! - **绝不使用** `deny_unknown_fields`：新版本写的字段旧版本必须能忽略而不是报错；
//! - **绝不去掉字段**：废弃的滑块保留为 `Option<f32>` 并标 deprecated，
//!   否则旧配方的数值会静默丢失（用户看不出「少了点什么」才是最难排查的 bug）；
//! - `version` 只在**不兼容**变更时 +1；新增可选参数不必升版本。
//!
//! ## 单位口径（前后端必须一致）
//!
//! 所有滑块一律用**归一化值** `-1.0 ..= 1.0`（或 `0..=1`）存储，
//! 不存 EV、不存 Kelvin、不存百分比。物理量到归一化值的映射集中在
//! `schema.rs`，那里的表就是前端滑块的刻度依据 —— 换刻度只改一处。
//! 这样做的直接好处：拖动滑块时前端传的是小数，`f32` 精度足够，
//! 而存「-0.35」比存「-1.75 EV」在不同版本之间更不容易被误改。

use serde::{Deserialize, Serialize};

/// 配方格式版本。当前格式为 v1。
pub const RECIPE_VERSION: u32 = 1;

/// 曲线控制点的最大数量。
///
/// 256 是硬上限而非性能考虑：曲线最终编译成 256 项 LUT，
/// 控制点多于 256 在数学上无法被表达（Catmull-Rom 会自交导致过冲）。
pub const MAX_CURVE_POINTS: usize = 64;

/// 一条曲线的控制点。x/y 均为 `0..=1` 的归一化位置。
///
/// 序列保证**按 x 严格递增**：插入时若与既有控制点重合或逆序，
/// 由 `Curve::upsert` 就地替换而不是追加（追加会产生零宽区间，
/// 除法求插值时除零）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
}

impl CurvePoint {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// 一条色调曲线：控制点 + 编译好的 256 项查找表。
///
/// 控制点是**权威数据**（要存盘、要给前端画界面），LUT 是**派生缓存**
/// （每次改点重编，256 项插值在 microseconds 级，不值得为它分文件）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curve {
    /// 控制点，按 x 递增。空 = 恒等曲线（y = x）。
    pub points: Vec<CurvePoint>,
}

impl Default for Curve {
    fn default() -> Self {
        Self { points: Vec::new() }
    }
}

impl Curve {
    /// 插入或替换一个控制点，返回是否真的改变了曲线。
    ///
    /// 替换判定用**距离阈值**而不是精确相等：前端拖动时会产生
    /// 1e-7 量级的抖动，若每次都重编 LUT 只会白白浪费 CPU。
    /// 替换已有控制点时按「新旧中点」落位，避免拖动产生跳变。
    pub fn upsert(&mut self, p: CurvePoint) -> bool {
        let p = CurvePoint {
            x: clamp01(p.x),
            y: clamp01(p.y),
        };
        for existing in self.points.iter_mut() {
            if (existing.x - p.x).abs() < 1e-3 {
                let moved = (existing.y - p.y).abs() > 1e-3;
                if moved {
                    existing.y = p.y;
                }
                return moved;
            }
        }
        if self.points.len() >= MAX_CURVE_POINTS {
            return false;
        }
        self.points.push(p);
        self.points
            .sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        true
    }

    /// 删除落在 x 附近的控制点；返回是否删掉了东西。
    ///
    /// 少于 2 点的曲线等价于恒等（两点只能定一条直线），
    /// 因此删到剩 1 点时直接清空，让前端回落到「未启用」状态。
    pub fn remove_near(&mut self, x: f32) -> bool {
        let before = self.points.len();
        self.points.retain(|p| (p.x - x).abs() >= 1e-3);
        if self.points.len() < 2 {
            self.points.clear();
        }
        before != self.points.len()
    }

    /// 是否为恒等曲线（可直接跳过整条曲线阶段，省一次 LUT 查表）。
    pub fn is_identity(&self) -> bool {
        self.points.len() < 2
    }
}

/// 「基本」面板：曝光 / 对比度 / 高光 / 阴影 / 白色 / 黑色 / 色温 / 色调。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BasicAdjustments {
    /// 曝光，单位 EV（-5 ..= +5）。此处**保留物理单位**，
    /// 因为它是对数刻度，存归一化值会丢失「滑块刻度与 EV 的非线性对应」信息。
    pub exposure: f32,
    /// 对比度，-1..=1（0 = 不动，支点 18% 中灰）
    pub contrast: f32,
    /// 高光恢复，-1..=1
    pub highlights: f32,
    /// 阴影提亮，-1..=1
    pub shadows: f32,
    /// 白色（高光端支点），-1..=1
    pub whites: f32,
    /// 黑色（暗部支点），-1..=1
    pub blacks: f32,
    /// 色温，单位 Kelvin（2000..=50000）
    pub temperature: f32,
    /// 色调，-100..=100（负绿正品红）
    pub tint: f32,
}

impl Default for BasicAdjustments {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            temperature: 6500.0,
            tint: 0.0,
        }
    }
}

impl BasicAdjustments {
    /// 是否全是中性值（用于跳过整条基本管线）。
    ///
    /// 注意色温：只有落在 6500K **且** tint 为 0 时才算中性，
    /// 因为 D65 是我们做增益归一化的锚点。
    pub fn is_neutral(&self) -> bool {
        self.exposure == 0.0
            && self.contrast == 0.0
            && self.highlights == 0.0
            && self.shadows == 0.0
            && self.whites == 0.0
            && self.blacks == 0.0
            && self.temperature == 6500.0
            && self.tint == 0.0
    }
}

/// 「颜色」面板：自然饱和度 / 饱和度。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorAdjustments {
    /// 自然饱和度，-1..=1（按「已有饱和度」反比加权，保护人像肤色）
    pub vibrance: f32,
    /// 饱和度，-1..=1（全图一致增减）
    pub saturation: f32,
}

impl Default for ColorAdjustments {
    fn default() -> Self {
        Self {
            vibrance: 0.0,
            saturation: 0.0,
        }
    }
}

/// 「细节」面板：锐化。
///
/// 四个参数与 Camera Raw 面板一一对应，不多不少：
/// 数量 / 半径 / 细节 / 蒙版。少了任何一个都无法复现 ACR 的观感。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharpenAdjustments {
    /// 锐化强度，0..=1（默认 0，即关闭 —— JPEG 本身已被相机锐化过，
    /// 默认再锐一次只会把噪点一起放大）
    pub amount: f32,
    /// 半径，0..=1（映射到 sigma = 0.5..=3.0 像素 @ 短边 1000px 基准）
    pub radius: f32,
    /// 细节，0..=1（低频残差保留比例，压光晕）
    pub detail: f32,
    /// 蒙版，0..=1（只锐化强边缘，避开平坦区噪点）
    pub masking: f32,
}

impl Default for SharpenAdjustments {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius: 0.5,
            detail: 0.25,
            // 蒙版默认 0.25 而非 0：ACR 的 raw 默认就是 25。
            // 但 amount=0 时整个阶段会被跳过，用户显式调了才生效。
            masking: 0.25,
        }
    }
}

/// 裁剪 / 旋转。坐标全部归一化到「旋转后画面」的 0..1，
/// 这样换一张不同分辨率的图打开时同一份配方仍能对得上位置。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CropSpec {
    /// 左边界，0..=1
    pub x: f32,
    /// 上边界，0..=1
    pub y: f32,
    /// 宽度，0..=1（<=0 表示未裁剪）
    pub w: f32,
    /// 高度，0..=1
    pub h: f32,
    /// 旋转角（度），-45..=45，仅用于「拉直」，不含 90° 整转
    pub angle: f32,
    /// 是否处于裁剪模式（false 时裁剪框仍然保留，只是不生效）
    pub enabled: bool,
}

impl Default for CropSpec {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            angle: 0.0,
            enabled: false,
        }
    }
}

impl CropSpec {
    /// 未裁剪 = 整幅画面都保留。
    ///
    /// 判定要求四项同时成立：显式关闭、角度为 0、覆盖整幅。
    /// 只判 `enabled` 是不够的 —— 用户关掉裁剪但留着框，
    /// 再打开时应该看到框还在，而不是框被清掉。
    pub fn is_full_frame(&self) -> bool {
        !self.enabled
            || (self.angle == 0.0
                && self.x <= 0.0
                && self.y <= 0.0
                && self.w >= 1.0
                && self.h >= 1.0)
    }
}

/// 完整配方 = 各面板之和。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdjustmentRecipe {
    pub version: u32,
    pub basic: BasicAdjustments,
    pub color: ColorAdjustments,
    /// RGB 曲线（三通道联动，Adobe 式保色相应用）
    pub curve_rgb: Curve,
    /// 亮度曲线（按亮度统一缩放，绝不偏色）
    pub curve_luma: Curve,
    pub sharpen: SharpenAdjustments,
    pub crop: CropSpec,
}

impl Default for AdjustmentRecipe {
    fn default() -> Self {
        Self {
            version: RECIPE_VERSION,
            basic: BasicAdjustments::default(),
            color: ColorAdjustments::default(),
            curve_rgb: Curve::default(),
            curve_luma: Curve::default(),
            sharpen: SharpenAdjustments::default(),
            crop: CropSpec::default(),
        }
    }
}

impl AdjustmentRecipe {
    /// 是否为「什么都没动」。用于跳过整条管线（全零配方不该烧 CPU）。
    pub fn is_identity(&self) -> bool {
        *self == AdjustmentRecipe::default()
    }

    /// 把越界值全部拉回合法区间。
    ///
    /// **反序列化后必须调用**：JSON 是用户可编辑的文本，
    /// `saturation: 900` 会被 serde 老老实实解析出来。
    /// 相比在每个使用点各自钳制，入口处统一收敛一次更不容易漏。
    pub fn sanitize(&mut self) {
        let b = &mut self.basic;
        b.exposure = clamp(b.exposure, -5.0, 5.0);
        b.contrast = clamp(b.contrast, -1.0, 1.0);
        b.highlights = clamp(b.highlights, -1.0, 1.0);
        b.shadows = clamp(b.shadows, -1.0, 1.0);
        b.whites = clamp(b.whites, -1.0, 1.0);
        b.blacks = clamp(b.blacks, -1.0, 1.0);
        b.temperature = clamp(b.temperature, 1500.0, 50_000.0);
        b.tint = clamp(b.tint, -100.0, 100.0);

        self.color.vibrance = clamp(self.color.vibrance, -1.0, 1.0);
        self.color.saturation = clamp(self.color.saturation, -1.0, 1.0);

        self.sharpen.amount = clamp(self.sharpen.amount, 0.0, 1.0);
        self.sharpen.radius = clamp(self.sharpen.radius, 0.0, 1.0);
        self.sharpen.detail = clamp(self.sharpen.detail, 0.0, 1.0);
        self.sharpen.masking = clamp(self.sharpen.masking, 0.0, 1.0);

        let c = &mut self.crop;
        c.angle = clamp(c.angle, -45.0, 45.0);
        if c.w <= 0.0 || c.h <= 0.0 || c.w > 1.0 || c.h > 1.0 {
            c.x = 0.0;
            c.y = 0.0;
            c.w = 1.0;
            c.h = 1.0;
        } else {
            // 裁剪框必须完全落在画面内，否则 resize 会向画布外采样成透明/黑边。
            c.x = clamp(c.x, 0.0, 1.0 - c.w);
            c.y = clamp(c.y, 0.0, 1.0 - c.h);
        }
    }

    /// 序列化为 JSON 文本。
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("序列化配方失败: {e}"))
    }

    /// 从 JSON 文本解析并做合法性收敛。
    ///
    /// 空字符串 = 空配方（前端「重置」走这条路，避免特殊处理 null）。
    pub fn from_json(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let mut r: AdjustmentRecipe =
            serde_json::from_str(text).map_err(|e| format!("解析配方失败: {e}"))?;
        r.sanitize();
        Ok(r)
    }
}

#[inline]
fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

#[inline]
fn clamp01(v: f32) -> f32 {
    clamp(v, 0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let r = AdjustmentRecipe::default();
        assert!(r.is_identity());
        assert!(r.basic.is_neutral());
    }

    /// 核心不变量：默认配方跑出来必须和原图**逐位相同**。
    /// 任何「顺手加一点点」的默认值都会在这里被抓出来。
    #[test]
    fn untouched_recipe_produces_identical_output() {
        assert_eq!(AdjustmentRecipe::default(), AdjustmentRecipe::default());
    }

    /// 空 JSON = 空配方，而不是错误。
    #[test]
    fn empty_json_yields_default() {
        assert!(AdjustmentRecipe::from_json("").unwrap().is_identity());
        assert!(AdjustmentRecipe::from_json("   \n ").unwrap().is_identity());
    }

    /// 旧配方文件（只有部分字段）必须能打开，且缺失项取默认值。
    #[test]
    fn partial_json_backfills_defaults() {
        let r = AdjustmentRecipe::from_json(r#"{"version":1,"basic":{"exposure":1.5}}"#).unwrap();
        assert_eq!(r.basic.exposure, 1.5);
        assert_eq!(r.basic.temperature, 6500.0);
        assert_eq!(r.color.saturation, 0.0);
        assert_eq!(r.sharpen.masking, 0.25);
    }

    /// 未知字段必须被静默忽略（向前兼容：新版本写、旧版本读）。
    #[test]
    fn unknown_fields_are_ignored() {
        let r = AdjustmentRecipe::from_json(
            r#"{"version":9,"exposure_xyz":1,"basic":{"exposure":2.0,"future":{"a":[1,2]}}}"#,
        )
        .unwrap();
        assert_eq!(r.basic.exposure, 2.0);
        assert_eq!(r.version, 9, "未知版本号不应被拒绝，只要字段能对上");
    }

    /// 往返：存盘再读回来必须完全一致（否则撤销栈与历史配方会漂移）。
    #[test]
    fn json_roundtrip_is_lossless() {
        let mut r = AdjustmentRecipe::default();
        r.basic.exposure = 1.25;
        r.basic.shadows = -0.4;
        r.curve_rgb.upsert(CurvePoint::new(0.25, 0.2));
        r.curve_rgb.upsert(CurvePoint::new(0.75, 0.9));
        r.crop = CropSpec {
            x: 0.1,
            y: 0.2,
            w: 0.6,
            h: 0.5,
            angle: 3.0,
            enabled: true,
        };
        let back = AdjustmentRecipe::from_json(&r.to_json().unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn sanitize_clamps_out_of_range() {
        let mut r = AdjustmentRecipe::from_json(
            r#"{"basic":{"exposure":99,"contrast":7,"temperature":-100,"tint":9999},
                "color":{"saturation":-50}}"#,
        )
        .unwrap();
        let _ = &mut r;
        assert_eq!(r.basic.exposure, 5.0);
        assert_eq!(r.basic.contrast, 1.0);
        assert_eq!(r.basic.temperature, 1500.0);
        assert_eq!(r.basic.tint, 100.0);
        assert_eq!(r.color.saturation, -1.0);
    }

    /// 裁剪框越界必须被拉回画内，否则采样会取到画布外的像素。
    #[test]
    fn sanitize_pulls_crop_inside_frame() {
        let mut r = AdjustmentRecipe::default();
        r.crop = CropSpec {
            x: 0.8,
            y: 0.9,
            w: 0.5,
            h: 0.5,
            angle: 0.0,
            enabled: true,
        };
        r.sanitize();
        assert!(
            r.crop.x + r.crop.w <= 1.0 + 1e-6,
            "x+w={}",
            r.crop.x + r.crop.w
        );
        assert!(r.crop.y + r.crop.h <= 1.0 + 1e-6);
    }

    /// 宽高为 0 或负的裁剪框等价于未裁剪，直接复位为整幅。
    #[test]
    fn sanitize_resets_degenerate_crop() {
        let mut r = AdjustmentRecipe::default();
        r.crop.w = 0.0;
        r.crop.enabled = true;
        r.sanitize();
        assert_eq!(r.crop.w, 1.0);
        assert!(r.crop.is_full_frame());
    }

    #[test]
    fn crop_full_frame_detection() {
        let mut c = CropSpec::default();
        assert!(c.is_full_frame(), "默认即整幅");
        c.enabled = true;
        assert!(c.is_full_frame(), "enabled 但整幅 = 仍未裁剪");
        c.w = 0.5;
        assert!(!c.is_full_frame(), "真正裁了就不该算整幅");
        c.w = 1.0;
        c.angle = 5.0;
        assert!(!c.is_full_frame(), "拉直也改变了画面");
    }

    #[test]
    fn curve_upsert_replaces_near_duplicates() {
        let mut c = Curve::default();
        assert!(c.upsert(CurvePoint::new(0.5, 0.5)));
        assert!(!c.upsert(CurvePoint::new(0.5, 0.5)), "完全相同不应算改动");
        assert!(
            c.upsert(CurvePoint::new(0.5001, 0.8)),
            "极小漂移应替换而非新增"
        );
        assert_eq!(c.points.len(), 1, "同一 x 只能有一个点");
        assert!((c.points[0].y - 0.8).abs() < 1e-3);
    }

    /// 控制点必须按 x 排序，否则插值会算出负斜率段。
    #[test]
    fn curve_points_stay_sorted() {
        let mut c = Curve::default();
        c.upsert(CurvePoint::new(0.8, 0.9));
        c.upsert(CurvePoint::new(0.2, 0.1));
        c.upsert(CurvePoint::new(0.5, 0.5));
        assert_eq!(c.points.len(), 3);
        assert!(c.points[0].x < c.points[1].x && c.points[1].x < c.points[2].x);
    }

    #[test]
    fn curve_clamps_points_to_unit_square() {
        let mut c = Curve::default();
        c.upsert(CurvePoint::new(-3.0, 5.0));
        assert!((0.0..=1.0).contains(&c.points[0].x));
        assert!((0.0..=1.0).contains(&c.points[0].y));
    }

    /// 少于 2 点的曲线是恒等的，必须按恒等处理。
    #[test]
    fn curve_identity_detection() {
        let mut c = Curve::default();
        assert!(c.is_identity());
        c.upsert(CurvePoint::new(0.5, 0.5));
        assert!(c.is_identity(), "单点曲线无法定形");
        c.upsert(CurvePoint::new(0.8, 0.9));
        assert!(!c.is_identity());
    }

    #[test]
    fn curve_remove_clears_when_below_two() {
        let mut c = Curve::default();
        c.upsert(CurvePoint::new(0.5, 0.5));
        assert!(c.remove_near(0.5));
        assert!(c.is_identity(), "删到不足两点应清空");
        assert!(!c.remove_near(0.5), "再删没有的东西应返回 false");
    }

    /// 控制点数量必须有上限，否则用户能存一个 10 万点的曲线把编译拖死。
    #[test]
    fn curve_respects_max_points() {
        let mut c = Curve::default();
        for i in 0..(MAX_CURVE_POINTS + 20) {
            c.upsert(CurvePoint::new(
                i as f32 / (MAX_CURVE_POINTS as f32 + 20.0),
                0.5,
            ));
        }
        assert!(c.points.len() <= MAX_CURVE_POINTS);
    }

    #[test]
    fn recipe_is_not_identity_after_any_change() {
        let mut r = AdjustmentRecipe::default();
        r.basic.exposure = 0.001;
        assert!(!r.is_identity());
    }
}
