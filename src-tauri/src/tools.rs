//! 创意工坊（FEAT-150）· 图像加工工具
//!
//! 定位：**对已有照片做加工**，不是修图画布。旧的非破坏调色引擎（FEAT-099）
//! 已整体下线（见 README「创意工坊」条目），本模块只保留真的属于
//! 「相册管理」语境的能力：
//!
//! **压缩**：给分享/发送场景做体积瘦身。可指定目标大小（KB）、长边上限、
//! 居中裁切比例。输出**永远是副本**（JPEG），绝不覆盖原图 —— 这一点是硬约束：
//! 压缩是破坏性操作，一旦写回原图就没有回头路。
//!
//! ## 分层（CS1）
//!
//! - **命令层**（`image_compress`）：参数校验、鉴权、`spawn_blocking`、出入口日志，
//!   不含业务算法。
//! - **服务层**（`compress_one`）：编排 解码 → 裁切 → 缩放 → 编码 → 落盘，可单测。
//! - **编码细节**（`encode_under_target` / `crop_rect_for` / `output_path`）：
//!   各自单一职责，便于替换编码策略。
//!
//! ## 为什么输出固定 JPEG
//!
//! 目标场景是「发给别人 / 传上去」，JPEG 兼容性最好。代价是：透明底会压白、
//! EXIF 不随文件走。前者由 `to_rgb_white` 显式铺白（否则透明区会变黑），
//! 后者在 UI 上写明，不做隐式承诺。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{DynamicImage, ImageEncoder};
use serde::{Deserialize, Serialize};

/// 质量阶梯（大 → 小）。命中目标大小的第一档就是结果，不做全量二分 ——
/// 12MP 图单次 JPEG 编码约 150~300ms，二分 7 次 × 批量 20 张 = 半分钟起步，
/// 阶梯通常 1~3 次就命中。
const QUALITY_LADDER: [u8; 10] = [95, 88, 82, 74, 66, 58, 50, 42, 34, 28];
/// 命中后向上试探的档位（用完预算，质量尽可能高）
const QUALITY_REFINE: [u8; 2] = [4, 8];
/// 长边小于该值就不再为凑大小继续缩小（再小就没有观看价值）
const MIN_LONG_EDGE: u32 = 480;
/// 为凑目标大小最多缩小几轮（每轮 0.85）
const MAX_SHRINK_ROUNDS: usize = 6;
const SHRINK_FACTOR: f64 = 0.85;

/* ============================ 数据结构 ============================ */

/// 压缩参数。全部可缺省：缺省 = 只按默认质量 82 转 JPEG，不改尺寸。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompressOptions {
    /// 目标文件大小（KB）。给了就走「凑大小」模式，忽略 `quality`。
    pub target_kb: Option<u32>,
    /// 固定质量（1~95）。`target_kb` 为空时生效。
    pub quality: Option<u8>,
    /// 长边上限（像素）。0 / 缺省 = 不缩放；不会放大。
    pub max_edge: Option<u32>,
    /// 居中裁切比例：`1:1` / `4:3` / `3:4` / `16:9` / `9:16`。空 = 不裁切。
    pub crop_aspect: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressedItem {
    pub src: String,
    pub dst: String,
    pub before: u64,
    pub after: u64,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressFailure {
    pub src: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressOutcome {
    pub items: Vec<CompressedItem>,
    pub failed: Vec<CompressFailure>,
    pub total_before: u64,
    pub total_after: u64,
    pub dest_dir: String,
}

/* ============================ 命令层 ============================ */

/// 批量压缩：`paths` → `dest_dir` 下的 JPEG 副本
///
/// - `destDir` 必须由前端给定（默认 `<首图目录>\_compressed`，用户可改）
/// - 单张失败不影响其它张，逐个记进 `failed`
/// - async + spawn_blocking：解码 + 编码是 CPU/IO 混合重负载，同步命令会卡住 UI
#[tauri::command]
pub async fn image_compress(
    paths: Vec<String>,
    dest_dir: String,
    opts: CompressOptions,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<CompressOutcome, String> {
    let _t = log_call!(
        "image_compress",
        &format!(
            "n={} dest={dest_dir} target_kb={:?} q={:?} max_edge={:?} crop={:?}",
            paths.len(),
            opts.target_kb,
            opts.quality,
            opts.max_edge,
            opts.crop_aspect
        )
    );
    crate::require_user(&session)?;
    if paths.is_empty() {
        return Ok(CompressOutcome {
            items: vec![],
            failed: vec![],
            total_before: 0,
            total_after: 0,
            dest_dir,
        });
    }

    let out = tauri::async_runtime::spawn_blocking(move || -> Result<CompressOutcome, String> {
        let dest = PathBuf::from(&dest_dir);
        std::fs::create_dir_all(&dest).map_err(|e| format!("创建输出目录失败: {e}"))?;
        let aspect = match opts.crop_aspect.as_deref().map(str::trim) {
            None | Some("") | Some("none") => None,
            Some(s) => Some(parse_aspect(s).ok_or_else(|| format!("裁切比例无法识别: {s}"))?),
        };

        let mut used: HashSet<String> = HashSet::new();
        let mut items = Vec::new();
        let mut failed = Vec::new();
        let mut total_before = 0u64;
        let mut total_after = 0u64;

        for src in &paths {
            match compress_one(
                Path::new(src),
                &dest,
                &opts,
                aspect,
                &mut used,
            ) {
                Ok(item) => {
                    total_before += item.before;
                    total_after += item.after;
                    items.push(item);
                }
                Err(reason) => {
                    crate::logger::log_warn(&format!("[tools] 压缩失败 {src}: {reason}"));
                    failed.push(CompressFailure { src: src.clone(), reason });
                }
            }
        }

        crate::logger::log_info(&format!(
            "[tools] 压缩完成：成功 {} / 失败 {}，{} KB → {} KB（省 {:.1}%）",
            items.len(),
            failed.len(),
            total_before / 1024,
            total_after / 1024,
            if total_before == 0 {
                0.0
            } else {
                (1.0 - total_after as f64 / total_before as f64) * 100.0
            }
        ));
        Ok(CompressOutcome {
            items,
            failed,
            total_before,
            total_after,
            dest_dir,
        })
    })
    .await
    .map_err(|e| format!("压缩任务异常: {e}"))??;

    Ok(out)
}

/* ============================ 服务层 ============================ */

/// 压一张：解码 → 裁切 → 缩放 → 编码 → 落盘
pub(crate) fn compress_one(
    src: &Path,
    dest: &Path,
    opts: &CompressOptions,
    aspect: Option<(u32, u32)>,
    used: &mut HashSet<String>,
) -> Result<CompressedItem, String> {
    if !src.is_file() {
        return Err("文件不存在".to_string());
    }
    let before = std::fs::metadata(src)
        .map_err(|e| format!("读取文件信息失败: {e}"))?
        .len();

    let mut img = crate::imgio::open_any(src)?;

    // 1) 居中裁切（先裁再缩：缩完再裁会先浪费一次大图重采样）
    if let Some((aw, ah)) = aspect {
        let (w, h) = (img.width(), img.height());
        if let Some((x, y, cw, ch)) = crop_rect_for(w, h, aw, ah) {
            img = img.crop_imm(x, y, cw, ch);
        }
    }

    // 2) 长边上限（只缩不放）
    if let Some(max_edge) = opts.max_edge.filter(|v| *v > 0) {
        let (w, h) = (img.width(), img.height());
        let long = w.max(h);
        if long > max_edge {
            let scale = max_edge as f64 / long as f64;
            let nw = ((w as f64 * scale).round() as u32).max(1);
            let nh = ((h as f64 * scale).round() as u32).max(1);
            img = img.resize(nw, nh, FilterType::Lanczos3);
        }
    }

    // 3) 编码：给了目标大小就凑，否则固定质量
    let (bytes, quality) = match opts.target_kb.filter(|v| *v > 0) {
        Some(kb) => encode_under_target(&mut img, kb as u64 * 1024)?,
        None => {
            let q = opts.quality.unwrap_or(82).clamp(1, 95);
            let rgb = to_rgb_white(&img);
            (encode_jpeg(&rgb, q)?, q)
        }
    };

    // 4) 落盘：绝不覆盖原图
    let target = output_path(dest, src, used);
    std::fs::write(&target, &bytes).map_err(|e| format!("写入失败: {e}"))?;

    Ok(CompressedItem {
        src: src.to_string_lossy().into_owned(),
        dst: target.to_string_lossy().into_owned(),
        before,
        after: bytes.len() as u64,
        width: img.width(),
        height: img.height(),
        quality,
    })
}

/// 凑目标大小：先按质量阶梯找命中档，命中后再向上试探把预算用满；
/// 阶梯最低档仍超标就逐轮缩小长边（每轮 0.85），直到达标或触底。
fn encode_under_target(img: &mut DynamicImage, target: u64) -> Result<(Vec<u8>, u8), String> {
    let mut best: Option<(Vec<u8>, u8)> = None;

    for round in 0..=MAX_SHRINK_ROUNDS {
        let rgb = to_rgb_white(img);
        let mut hit: Option<(Vec<u8>, u8)> = None;
        for q in QUALITY_LADDER {
            let bytes = encode_jpeg(&rgb, q)?;
            if bytes.len() as u64 <= target {
                hit = Some((bytes, q));
                break;
            }
        }
        match hit {
            Some((bytes, q)) => {
                // 向上试探：预算还有富余就把质量抬回来（每档 +4 / +8）
                let mut best_bytes = bytes;
                let mut best_q = q;
                for step in QUALITY_REFINE {
                    let q2 = q.saturating_add(step);
                    if q2 > 95 {
                        continue;
                    }
                    let b2 = encode_jpeg(&rgb, q2)?;
                    if b2.len() as u64 <= target {
                        best_bytes = b2;
                        best_q = q2;
                    }
                }
                best = Some((best_bytes, best_q));
                break;
            }
            None => {
                if round == MAX_SHRINK_ROUNDS || img.width().max(img.height()) <= MIN_LONG_EDGE {
                    break;
                }
                let (w, h) = (img.width(), img.height());
                let nw = ((w as f64 * SHRINK_FACTOR).round() as u32).max(MIN_LONG_EDGE.min(w));
                let nh = ((h as f64 * SHRINK_FACTOR).round() as u32).max(1);
                *img = img.resize(nw, nh, FilterType::Lanczos3);
            }
        }
    }

    // 触底仍超标：交最低质量的结果，并让调用方在 UI 上如实呈现（不谎报达标）
    match best {
        Some(v) => Ok(v),
        None => {
            let rgb = to_rgb_white(img);
            let q = *QUALITY_LADDER.last().unwrap_or(&28);
            Ok((encode_jpeg(&rgb, q)?, q))
        }
    }
}

fn encode_jpeg(rgb: &image::RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;
    Ok(out)
}

/// 透明底铺白：直接 `to_rgb8` 会把 alpha 丢掉，透明区颜色不确定（常见为黑）
fn to_rgb_white(img: &DynamicImage) -> image::RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = image::RgbImage::new(w, h);
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as u32;
        let blend = |c: u8| (((c as u32) * a + 255 * (255 - a)) / 255) as u8;
        out.put_pixel(x, y, image::Rgb([blend(p[0]), blend(p[1]), blend(p[2])]));
    }
    out
}

/// `1:1` / `4:3` … → `(1,1)` / `(4,3)`；非法返回 None
pub(crate) fn parse_aspect(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once(':')?;
    let w: u32 = a.trim().parse().ok()?;
    let h: u32 = b.trim().parse().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

/// 居中裁切矩形（x, y, w, h）。比例已相符时返回 None（不必裁）。
fn crop_rect_for(w: u32, h: u32, aw: u32, ah: u32) -> Option<(u32, u32, u32, u32)> {
    if w == 0 || h == 0 || aw == 0 || ah == 0 {
        return None;
    }
    // 交叉相乘比较，避免浮点误差：w/h  vs  aw/ah
    let lhs = w as u64 * ah as u64;
    let rhs = h as u64 * aw as u64;
    if lhs == rhs {
        return None;
    }
    if lhs > rhs {
        // 太宽 → 裁两侧
        let cw = ((h as u64 * aw as u64) / ah as u64) as u32;
        let cw = cw.clamp(1, w);
        Some(((w - cw) / 2, 0, cw, h))
    } else {
        // 太高 → 裁上下
        let ch = ((w as u64 * ah as u64) / aw as u64) as u32;
        let ch = ch.clamp(1, h);
        Some((0, (h - ch) / 2, w, ch))
    }
}

/// 输出文件名：同目录同名绝不覆盖原图，重名自动加序号
fn output_path(dest: &Path, src: &Path, used: &mut HashSet<String>) -> PathBuf {
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("image");

    let mut name = format!("{stem}.jpg");
    // 输出目录 == 源目录时，`stem.jpg` 可能正是原图本身（JPEG 源），必须避开
    if dest.join(&name) == src {
        name = format!("{stem}_compressed.jpg");
    }
    let mut i = 1;
    while used.contains(&name) || dest.join(&name).exists() {
        let candidate = if i == 1 && name == format!("{stem}.jpg") {
            format!("{stem}_compressed.jpg")
        } else {
            format!("{stem}_{i}.jpg")
        };
        if candidate == name {
            i += 1;
            continue;
        }
        name = candidate;
        i += 1;
    }
    used.insert(name.clone());
    dest.join(name)
}

/* ============================ 单测 ============================ */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aspect_accepts_common_ratios() {
        assert_eq!(parse_aspect("1:1"), Some((1, 1)));
        assert_eq!(parse_aspect(" 16 : 9 "), Some((16, 9)));
    }

    #[test]
    fn parse_aspect_rejects_garbage() {
        assert_eq!(parse_aspect(""), None);
        assert_eq!(parse_aspect("1"), None);
        assert_eq!(parse_aspect("0:1"), None);
        assert_eq!(parse_aspect("a:b"), None);
    }

    #[test]
    fn crop_square_from_wide_image_keeps_height() {
        // 4000×3000 裁 1:1 → 3000×3000，水平居中
        assert_eq!(crop_rect_for(4000, 3000, 1, 1), Some((500, 0, 3000, 3000)));
    }

    #[test]
    fn crop_wide_from_tall_image_keeps_width() {
        // 3000×4000 裁 16:9 → 3000×1687，垂直居中
        let (x, y, w, h) = crop_rect_for(3000, 4000, 16, 9).unwrap();
        assert_eq!((x, w), (0, 3000));
        assert_eq!(h, 1687);
        assert_eq!(y, (4000 - 1687) / 2);
    }

    #[test]
    fn crop_is_noop_when_ratio_already_matches() {
        assert_eq!(crop_rect_for(1600, 900, 16, 9), None);
    }

    #[test]
    fn output_never_overwrites_source_in_same_dir() {
        let dir = std::path::PathBuf::from("C:/pics");
        let src = dir.join("a.jpg");
        let mut used = HashSet::new();
        let out = output_path(&dir, &src, &mut used);
        assert_eq!(out, dir.join("a_compressed.jpg"));
    }

    #[test]
    fn output_dedupes_repeated_names() {
        let dest = std::path::PathBuf::from("C:/out");
        let src = std::path::PathBuf::from("C:/pics/a.png");
        let mut used = HashSet::new();
        assert_eq!(output_path(&dest, &src, &mut used), dest.join("a.jpg"));
        assert_eq!(
            output_path(&dest, &src, &mut used),
            dest.join("a_compressed.jpg")
        );
        assert_eq!(output_path(&dest, &src, &mut used), dest.join("a_2.jpg"));
    }

    /// 端到端：真图 → 裁切 → 缩长边 → 凑目标大小 → 落盘副本。
    ///
    /// 这条比单测某个函数重要："指定大小"是用户唯一能验收的承诺，
    /// 整条链路（解码 / 裁切 / 缩放 / 阶梯搜质量 / 写文件）任一处退步都会在这里暴露。
    #[test]
    fn compress_one_hits_target_size_and_keeps_source() {
        let root = std::env::temp_dir().join(format!("pms_compress_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // 造一张带噪点的 1200×900 PNG（纯色图压出来小得不真实，噪点更接近实拍）
        let mut src_img = image::RgbImage::new(1200, 900);
        for (x, y, px) in src_img.enumerate_pixels_mut() {
            let n = ((x * 37 + y * 91) % 256) as u8;
            *px = image::Rgb([n, n.wrapping_mul(3), 255u8.wrapping_sub(n)]);
        }
        let src = root.join("shot.png");
        src_img.save(&src).unwrap();
        let src_len = std::fs::metadata(&src).unwrap().len();

        let opts = CompressOptions {
            target_kb: Some(40),
            quality: None,
            max_edge: Some(500),
            crop_aspect: Some("1:1".to_string()),
        };
        let mut used = HashSet::new();
        let item = compress_one(&src, &root, &opts, Some((1, 1)), &mut used).unwrap();

        assert!(item.after <= 40 * 1024, "没凑到目标大小：{} 字节", item.after);
        assert!(item.after < src_len, "压缩后反而变大");
        // 先裁 1:1 再缩长边 → 500×500
        assert_eq!((item.width, item.height), (500, 500));
        assert!(std::path::Path::new(&item.dst).is_file(), "产物没落盘");
        assert!(item.dst.ends_with(".jpg"));
        // 原图必须原封不动（压缩是破坏性操作，写回原图就没有回头路）
        assert_eq!(std::fs::metadata(&src).unwrap().len(), src_len);

        let _ = std::fs::remove_dir_all(&root);
    }
}
