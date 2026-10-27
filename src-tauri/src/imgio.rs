//! 按**内容**解码图片（不信扩展名）
//!
//! ## 为什么需要这个模块（BUG-2026-1027-004）
//!
//! `image::open` 先按**扩展名**猜格式（`ImageReader::open` 的语义），再解码 ——
//! 遇到「内容与扩展名不一致」的文件会直接解码失败：
//!
//! ```text
//! 图片解码失败: Format error decoding Jpeg: Illegal start bytes:8950
//! ```
//!
//! `8950` 是 PNG 的魔数（`\x89PNG`）。本项目的图库里有相当一批这种文件
//! （微信/截图导出的 `.jpg` 实际是 PNG）：实测 `vcr-data/persons.db` 的
//! 10,565 条人脸中有 **95 条**原图是 PNG，其中一批就叫 `*.jpg`；日志里
//! `get_person_avatar` 的「无法读取图片头信息」与 `get_person_photos` 的
//! 「缩略图补齐失败」都由这类文件引起。
//!
//! `image::load_from_memory` 用魔数嗅探格式，与扩展名无关 —— 所以走内存解码。

use std::path::Path;

/// 读文件并按魔数解码任意受支持的图片格式
///
/// 只用于**非 JPEG 快速路径**与降级兜底：JPEG 源图仍走 `jpeg_decoder` 的
/// DCT 降采样（否则 2000 万像素的大图全解码要 5~14 秒）。
pub fn open_any(path: &Path) -> Result<image::DynamicImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取图片失败: {e}"))?;
    image::load_from_memory(&bytes).map_err(|e| format!("图片解码失败: {e}"))
}
