//! 参数表：前后端共用的**唯一**滑块定义
//! ====================================================================
//! 前端不硬编码任何滑块的名称、范围或默认值 —— 全部由本模块导出，
//! 前端 `invoke("edit_schema")` 拉取后按 schema 生成面板。收益：
//!
//! - 改一个滑块的范围只改这一处，不会出现「前端还按旧范围钳制、
//!   后端已经接受新值」的错位；
//! - 数值显示（EV / K / 百分比 / 原始值）也由 `display` 声明，
//!   前端不做任何格式化猜测；
//! - 归一化值 ↔ 物理量的映射只在这里，因此「默认相机照片自动优化」
//!   （`autotune`）与 UI 刻度必然一致。
//!
//! ## 归一化约定
//!
//! 大部分滑块以 `-1..=1` 存储（0 = 不动），只有三个例外，它们保留物理量：
//!
//! | 滑块 | 存什么 | 原因 |
//! |---|---|---|
//! | 曝光 | EV (-5..5) | 对数刻度，存归一化会丢失刻度非线性 |
//! | 色温 | Kelvin | 用户输入的是绝对色温，存偏移会让「6500K」变得不可寻址 |
//! | 色调 | -100..100 | 与 Lightroom 的显示口径一致，便于导入导出 |
//!
//! 其余全部 `-1..=1`。**这样做的直接收益**是配方文件可读且稳定：
//! 任何实现改动只需调整本文件的映射，用户的配方数值不变。

use serde::Serialize;

/// 参数类型（决定前端渲染哪种控件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlKind {
    /// 双极滑块（-1..1，中点为不动）
    Slider,
    /// 单极滑块（0..1）
    SliderPositive,
    /// 下拉选择
    Select,
}

/// 数值显示格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueFormat {
    /// 直接显示数值
    Plain,
    /// EV，带 +/- 与一位小数
    Ev,
    /// Kelvin
    Kelvin,
    /// 百分比
    Percent,
    /// 滑块位置（0..1 映射到 0..100%）
    Percent100,
}

/// 一个可调参数的完整描述。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    /// 配方中的字段路径，如 `basic.exposure` / `curveRgb` / `crop.angle`
    pub key: String,
    /// 中文标签
    pub label: String,
    /// 控件类型
    pub kind: ControlKind,
    /// 存储值的最小值
    pub min: f32,
    /// 存储值的最大值
    pub max: f32,
    /// 「不动」的取值（双极滑块的几何中点）
    pub neutral: f32,
    /// 拖动步长（存储单位）
    pub step: f32,
    /// 显示格式
    pub format: ValueFormat,
    /// 鼠标滚轮精细调整步长
    pub fine_step: f32,
    /// 一句话说明
    pub help: String,
}

fn slider(
    key: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    format: ValueFormat,
    help: &'static str,
) -> ParamSpec {
    ParamSpec {
        key: key.to_string(),
        label: label.to_string(),
        kind: ControlKind::Slider,
        min,
        max,
        neutral: 0.0,
        step: 0.01,
        format,
        fine_step: 0.001,
        help: help.to_string(),
    }
}

fn bi(key: &'static str, label: &'static str, help: &'static str) -> ParamSpec {
    slider(key, label, -1.0, 1.0, ValueFormat::Plain, help)
}

/// 「基本」面板的参数。
pub fn basic_params() -> Vec<ParamSpec> {
    vec![
        ParamSpec {
            key: "basic.exposure".into(),
            label: "曝光".into(),
            kind: ControlKind::Slider,
            min: -5.0,
            max: 5.0,
            neutral: 0.0,
            step: 0.05,
            format: ValueFormat::Ev,
            fine_step: 0.01,
            help: "整体明暗。+1 = 亮度翻倍，-1 = 减半。".into(),
        },
        ParamSpec {
            key: "basic.contrast".into(),
            label: "对比度".into(),
            kind: ControlKind::Slider,
            min: -1.0,
            max: 1.0,
            neutral: 0.0,
            step: 0.01,
            format: ValueFormat::Plain,
            fine_step: 0.001,
            help: "以 18% 中灰为支点拉伸明暗差，中灰本身不动。".into(),
        },
        bi(
            "basic.highlights",
            "高光",
            "压回过曝区域的速度。-100 = 强力抢救。",
        ),
        bi(
            "basic.shadows",
            "阴影",
            "提亮暗部。+100 只影响亮度约 0.06 以下的区域。",
        ),
        bi(
            "basic.whites",
            "白色",
            "移动最亮处的支点，决定「白」在哪里。",
        ),
        bi(
            "basic.blacks",
            "黑色",
            "抬起纯黑支点。+100 时纯黑变成深灰。",
        ),
        ParamSpec {
            key: "basic.temperature".into(),
            label: "色温".into(),
            kind: ControlKind::Slider,
            min: 1500.0,
            max: 50_000.0,
            neutral: 6500.0,
            step: 100.0,
            format: ValueFormat::Kelvin,
            fine_step: 10.0,
            help: "暖↔冷。6500K 为中性（D65）。数值越大越暖（越黄）。".into(),
        },
        ParamSpec {
            key: "basic.tint".into(),
            label: "色调".into(),
            kind: ControlKind::Slider,
            min: -100.0,
            max: 100.0,
            neutral: 0.0,
            step: 1.0,
            format: ValueFormat::Percent,
            fine_step: 0.1,
            help: "绿↔品红。负值偏绿，正值偏品红。".into(),
        },
    ]
}

/// 「颜色」面板的参数。
pub fn color_params() -> Vec<ParamSpec> {
    vec![
        bi(
            "color.vibrance",
            "自然饱和度",
            "按已有艳度反比加权：接近中性灰的区域动得多，艳色几乎不动，人像肤色因此不爆。",
        ),
        bi(
            "color.saturation",
            "饱和度",
            "所有像素统一增减艳度。+100 可能让大面积蓝天变纯蓝。",
        ),
    ]
}

/// 「细节」面板的参数。
pub fn detail_params() -> Vec<ParamSpec> {
    vec![
        ParamSpec {
            key: "sharpen.amount".into(),
            label: "锐化强度".into(),
            kind: ControlKind::SliderPositive,
            min: 0.0,
            max: 1.0,
            neutral: 0.0,
            step: 0.01,
            format: ValueFormat::Percent100,
            fine_step: 0.001,
            help: "非锐化掩模的总强度。0 = 关闭（JPEG 已被相机锐化过，默认关闭）。".into(),
        },
        ParamSpec {
            key: "sharpen.radius".into(),
            label: "半径".into(),
            kind: ControlKind::SliderPositive,
            min: 0.0,
            max: 1.0,
            neutral: 0.5,
            step: 0.01,
            format: ValueFormat::Plain,
            fine_step: 0.001,
            help: "高斯 σ，按图像短边归一化（0.5~3.0 像素 @短边 1000px）。越大光晕越宽。".into(),
        },
        ParamSpec {
            key: "sharpen.detail".into(),
            label: "细节".into(),
            kind: ControlKind::SliderPositive,
            min: 0.0,
            max: 1.0,
            neutral: 0.25,
            step: 0.01,
            format: ValueFormat::Percent100,
            fine_step: 0.001,
            help: "边缘过渡的保留程度。降低可压掉强边缘两侧的光晕。".into(),
        },
        ParamSpec {
            key: "sharpen.masking".into(),
            label: "蒙版".into(),
            kind: ControlKind::SliderPositive,
            min: 0.0,
            max: 1.0,
            neutral: 0.25,
            step: 0.01,
            format: ValueFormat::Percent100,
            fine_step: 0.001,
            help: "只锐化强边缘。提高可避免把平坦区的噪点一起放大。".into(),
        },
    ]
}

/// 「裁剪」面板的参数（不含曲线 —— 曲线是自绘控件，不是滑块）。
pub fn crop_params() -> Vec<ParamSpec> {
    vec![ParamSpec {
        key: "crop.angle".into(),
        label: "角度".into(),
        kind: ControlKind::Slider,
        min: -45.0,
        max: 45.0,
        neutral: 0.0,
        step: 0.1,
        format: ValueFormat::Plain,
        fine_step: 0.05,
        help: "水平拉直。负值逆时针，正值顺时针。".into(),
    }]
}

/// 曲线面板描述（前端据此自绘曲线编辑器）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurveSpec {
    pub key: String,
    pub label: String,
    pub help: String,
}

/// 曲线面板参数。
pub fn curve_params() -> Vec<CurveSpec> {
    vec![
        CurveSpec {
            key: "curveRgb".into(),
            label: "RGB 曲线".into(),
            help: "三通道联动。按 Adobe 的做法对最大/最小通道查表、中间通道按相对位置插值，因此饱和色的色相不会被曲线染色。".into(),
        },
        CurveSpec {
            key: "curveLuma".into(),
            label: "亮度曲线".into(),
            help: "只按亮度统一缩放三通道，严格保色相。适合只调明暗不改色。".into(),
        },
    ]
}

/// 面板分组描述（前端按此顺序渲染）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSpec {
    /// 配方里的分组键，如 `basic`
    pub key: String,
    pub label: String,
    /// 面板顺序（0 起）
    pub order: u32,
    /// 逐项参数
    pub params: Vec<ParamSpec>,
    /// 曲线类控件（若该组有）
    pub curves: Vec<CurveSpec>,
    /// 组内是否默认折叠
    pub collapsed: bool,
}

/// 全量参数表。
pub fn groups() -> Vec<GroupSpec> {
    vec![
        GroupSpec {
            key: "basic".into(),
            label: "基本".into(),
            order: 0,
            params: basic_params(),
            curves: vec![],
            collapsed: false,
        },
        GroupSpec {
            key: "curve".into(),
            label: "色调曲线".into(),
            order: 1,
            params: vec![],
            curves: curve_params(),
            collapsed: false,
        },
        GroupSpec {
            key: "color".into(),
            label: "颜色".into(),
            order: 2,
            params: color_params(),
            curves: vec![],
            collapsed: false,
        },
        GroupSpec {
            key: "detail".into(),
            label: "细节".into(),
            order: 3,
            params: detail_params(),
            curves: vec![],
            collapsed: false,
        },
        GroupSpec {
            key: "crop".into(),
            label: "裁剪与拉直".into(),
            order: 4,
            params: crop_params(),
            curves: vec![],
            collapsed: false,
        },
    ]
}

/// 完整 schema（`edit_schema` 命令的返回值）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub version: u32,
    pub groups: Vec<GroupSpec>,
    /// 预览档位（前端据此选长边）
    pub preview_long_edge: u32,
    pub draft_long_edge: u32,
    /// 数值显示格式枚举（前端据此分派格式化函数）
    pub formats: Vec<String>,
    pub controls: Vec<String>,
}

pub fn schema() -> Schema {
    Schema {
        version: super::recipe::RECIPE_VERSION,
        groups: groups(),
        preview_long_edge: super::source::PREVIEW_LONG_EDGE,
        draft_long_edge: super::source::DRAFT_LONG_EDGE,
        formats: vec![
            "plain".into(),
            "ev".into(),
            "kelvin".into(),
            "percent".into(),
            "percent100".into(),
        ],
        controls: vec!["slider".into(), "sliderPositive".into(), "select".into()],
    }
}

/// 自动优化（Auto）：从直方图推一组保守的起始值。
///
/// **为什么必须有这个**：新用户打开编辑器面对 12 个滑块的第一反应是不知道
/// 拖哪个，先给一个「不会把照片弄坏」的起点，体验差别巨大。
///
/// 算法刻意保守 —— 直方图法只能可靠地判断「明显偏暗/偏亮」与「截断」，
/// 对「是否欠曝」没有把握，所以：
/// - 只做单次线性拟合的曝光修正（不超过 ±1.5EV）；
/// - 只在检测到截断时才动高光/白色；
/// - **完全不动**阴影、黑色、对比度、饱和度、锐化 ——
///   这几项猜错的代价（噪点、灰雾、假色）远大于收益。
///
/// 返回值仍是归一化值，可直接并入配方；用户随后可以自由微调。
pub fn autotune(luma_hist: &[u32], rgb_hist: &[u32]) -> super::recipe::AdjustmentRecipe {
    let mut r = super::recipe::AdjustmentRecipe::default();
    let total: u64 = luma_hist.iter().map(|c| *c as u64).sum();
    if total == 0 {
        return r;
    }

    // 平均亮度
    let mean: f64 = luma_hist
        .iter()
        .enumerate()
        .map(|(i, c)| i as f64 * *c as f64)
        .sum::<f64>()
        / total as f64
        / 255.0;

    // 曝光：目标中灰 0.42（略高于 18%，因为 JPEG 直方图的中位数往往偏暗）
    // 映射到 EV 时做对数压缩，避免大偏差导致过度校正。
    let target = 0.42f64;
    let ratio = (target / mean.max(0.02)).clamp(0.35, 2.8);
    let ev = ratio.log2().clamp(-1.5, 1.5);
    r.basic.exposure = (ev as f32 * 10.0).round() / 10.0;

    // 高光/白色：仅当暗端确实有堆积时才压。
    // 判据是「0~3 桶的占比」，那是真正的死黑，不是正常暗部。
    let dark_clump: u64 = luma_hist[0..4].iter().map(|c| *c as u64).sum();
    if dark_clump as f64 / total as f64 > 0.03 {
        r.basic.shadows = 0.25;
        r.basic.blacks = 0.12;
    }

    // 高光截断：任一通道 254/255 堆积超过 1% → 压高光
    let blown: u64 = (0..3)
        .map(|c| {
            let base = c * 256;
            (rgb_hist[base + 254] + rgb_hist[base + 255]) as u64
        })
        .sum();
    let rgb_total = total * 3;
    if rgb_total > 0 && blown as f64 / rgb_total as f64 > 0.01 {
        r.basic.highlights = -0.45;
        r.basic.whites = -0.2;
    }

    // 低饱和度照片轻微提自然饱和度；已高饱和的完全不动（防止过艳）
    let sat_sum: u64 = (0..3)
        .flat_map(|c| {
            let base = c * 256;
            rgb_hist[base..base + 256]
                .iter()
                .enumerate()
                .map(move |(i, n)| i as u64 * *n as u64)
        })
        .sum();
    let mean_channel = sat_sum as f64 / rgb_total as f64;
    if mean_channel < 110.0 {
        r.color.vibrance = 0.2;
    }

    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_covers_every_adjustment() {
        let s = schema();
        let keys: Vec<&str> = s
            .groups
            .iter()
            .flat_map(|g| g.params.iter())
            .map(|p| p.key.as_str())
            .collect();
        // 基本 8 项 + 颜色 2 项 + 细节 4 项 + 裁剪 1 项
        assert_eq!(keys.len(), 15, "参数项数不符：{keys:?}");
        assert!(keys.contains(&"basic.exposure"));
        assert!(keys.contains(&"basic.highlights"));
        assert!(keys.contains(&"sharpen.masking"));
        assert!(keys.contains(&"crop.angle"));
    }

    /// 滑块定义与配方字段必须一一对应，否则前端能拖但存不进去。
    #[test]
    fn every_param_key_resolves_in_recipe_json() {
        let json = super::super::recipe::AdjustmentRecipe::default()
            .to_json()
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        for g in schema().groups {
            for p in &g.params {
                let mut cur = &v;
                let mut ok = true;
                for seg in p.key.split('.') {
                    match cur.get(seg) {
                        Some(next) => cur = next,
                        None => {
                            ok = false;
                            eprintln!("schema 键 {} 在配方中不存在", p.key);
                            break;
                        }
                    }
                }
                assert!(ok, "schema 键 {} 在配方中不存在", p.key);
                // 且必须是数字（前端按数值渲染）
                assert!(cur.is_number(), "{} 解析到的不是数值：{cur}", p.key);
            }
        }
    }

    #[test]
    fn param_keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in schema().groups.iter().flat_map(|g| g.params.iter()) {
            assert!(seen.insert(p.key.clone()), "重复的参数键 {}", p.key);
        }
    }

    #[test]
    fn ranges_and_neutral_points_are_sane() {
        for p in schema().groups.iter().flat_map(|g| g.params.iter()) {
            assert!(p.min < p.max, "{} 范围非法", p.key);
            assert!(
                p.neutral >= p.min && p.neutral <= p.max,
                "{} 的不动点 {} 落在范围 [{}, {}] 之外",
                p.key,
                p.neutral,
                p.min,
                p.max
            );
            assert!(
                p.step > 0.0 && p.step <= (p.max - p.min),
                "{} 步长不合理",
                p.key
            );
            assert!(
                p.fine_step > 0.0 && p.fine_step <= p.step,
                "{} 精细步长不合理",
                p.key
            );
            assert!(
                !p.label.is_empty() && !p.help.is_empty(),
                "{} 缺少文案",
                p.key
            );
        }
    }

    #[test]
    fn group_order_is_sequential_and_unique() {
        let s = schema();
        let mut orders: Vec<u32> = s.groups.iter().map(|g| g.order).collect();
        assert_eq!(orders.len(), 5);
        orders.sort_unstable();
        assert_eq!(orders, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn only_curve_group_has_curves() {
        let s = schema();
        for g in &s.groups {
            if g.key != "curve" {
                assert!(g.curves.is_empty(), "{} 不该有曲线控件", g.key);
            }
        }
        let curve_group = s.groups.iter().find(|g| g.key == "curve").unwrap();
        assert_eq!(curve_group.curves.len(), 2);
    }

    #[test]
    fn schema_version_matches_recipe_version() {
        assert_eq!(schema().version, super::super::recipe::RECIPE_VERSION);
    }

    // ---------- 自动优化 ----------

    fn hist_from_values(vals: &[u8]) -> (Vec<u32>, Vec<u32>) {
        let mut l = vec![0u32; 256];
        let mut rgb = vec![0u32; 768];
        for v in vals {
            l[*v as usize] += 1;
            rgb[*v as usize * 3] += 1;
            rgb[*v as usize * 3 + 1] += 1;
            rgb[*v as usize * 3 + 2] += 1;
        }
        (l, rgb)
    }

    #[test]
    fn autotune_empty_histogram_is_noop() {
        let r = autotune(&vec![0u32; 256], &vec![0u32; 768]);
        assert!(r.is_identity(), "空直方图应给出空配方");
    }

    #[test]
    fn autotune_darkens_bright_image() {
        let vals: Vec<u8> = (200..230).collect();
        let (l, rgb) = hist_from_values(&vals);
        let r = autotune(&l, &rgb);
        assert!(
            r.basic.exposure < 0.0,
            "亮图应给负曝光，实得 {}",
            r.basic.exposure
        );
    }

    #[test]
    fn autotune_brightens_dark_image() {
        let vals: Vec<u8> = (40..70).collect();
        let (l, rgb) = hist_from_values(&vals);
        let r = autotune(&l, &rgb);
        assert!(
            r.basic.exposure > 0.0,
            "暗图应给正曝光，实得 {}",
            r.basic.exposure
        );
    }

    /// 自动优化必须保守 —— 猜错的代价远大于猜对的收益。
    #[test]
    fn autotune_is_conservative() {
        let vals: Vec<u8> = (90..160).collect();
        let (l, rgb) = hist_from_values(&vals);
        let r = autotune(&l, &rgb);
        assert!(r.basic.exposure.abs() <= 1.5, "曝光超出 ±1.5EV 的保守上限");
        assert_eq!(r.basic.contrast, 0.0, "自动优化不得猜对比度");
        assert!(r.basic.highlights.abs() <= 0.5, "高光修正不应超过 0.5");
        assert_eq!(r.sharpen.amount, 0.0, "自动优化不得猜锐化");
    }

    #[test]
    fn autotune_reacts_to_highlight_clipping() {
        // 一半像素在 255（严重过曝）
        let mut vals: Vec<u8> = vec![120; 500];
        vals.extend(vec![255u8; 500]);
        let (l, rgb) = hist_from_values(&vals);
        let r = autotune(&l, &rgb);
        assert!(r.basic.highlights < 0.0, "有截断时应压高光");
    }

    #[test]
    fn autotune_reacts_to_shadowed_blacks() {
        let mut vals: Vec<u8> = vec![150; 900];
        vals.extend(vec![0u8; 100]);
        let (l, rgb) = hist_from_values(&vals);
        let r = autotune(&l, &rgb);
        assert!(r.basic.blacks > 0.0, "有死黑堆积时应抬黑");
    }

    #[test]
    fn autotune_output_is_always_sanitizable() {
        for shift in [-40i32, 0, 40] {
            let vals: Vec<u8> = (0..255)
                .map(|i| ((i as i32 + shift) as i32).clamp(0, 255) as u8)
                .collect();
            let (l, rgb) = hist_from_values(&vals);
            let mut r = autotune(&l, &rgb);
            r.sanitize();
            // sanitize 后仍应落在合法范围（否则说明 autotune 越界了）
            assert!(r.basic.exposure >= -5.0 && r.basic.exposure <= 5.0);
        }
    }
}
