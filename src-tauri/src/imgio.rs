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
//!
//! ## FEAT-142：HEIC / HEIF 分流
//!
//! `image` crate 不支持 HEIF 容器。这里按魔数识别 HEIF 后转发给 [`crate::heic`]
//! （Windows 上由系统 WIC 的 `Microsoft HEIF Decoder` 解码）。分流放在本模块，
//! 是为了让缩略图 / 影调 / 编辑 / 预览四条链路**只改一个调用点**就能同时支持 HEIC ——
//! 否则每处 `image::open` 都要各自判断一次，迟早漏一处。

use std::path::Path;

/// 读文件并按魔数解码任意受支持的图片格式
///
/// 只用于**非 JPEG 快速路径**与降级兜底：JPEG 源图仍走 `jpeg_decoder` 的
/// DCT 降采样（否则 2000 万像素的大图全解码要 5~14 秒）。
pub fn open_any(path: &Path) -> Result<image::DynamicImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取图片失败: {e}"))?;
    open_memory(path, &bytes)
}

/// 已读入内存的字节 → `DynamicImage`（HEIF 按魔数分流，其余交给 `image`）
///
/// 传入 `path` 只用于 HEIF 分支（WIC 从文件读，不重复占用内存）。
pub fn open_memory(path: &Path, bytes: &[u8]) -> Result<image::DynamicImage, String> {
    // 只认**魔数**，不认扩展名：`.heic` 名字下装着 JPEG 的情况同样要能读（同 BUG-2026-1027-004）
    if crate::heic::is_heif_bytes(bytes) {
        return crate::heic::decode(path);
    }
    image::load_from_memory(bytes).map_err(|e| format!("图片解码失败: {e}"))
}

/// 只读图片尺寸（不解码像素）
///
/// 优先用格式头（JPEG SOF / PNG IHDR / WebP / HEIF ispe），读不到再退回整解码。
/// 供「保留分辨率最高那张」这类判断使用 —— 全库逐张解码只为拿宽高是不可接受的。
pub fn probe_dimensions(path: &Path) -> Option<(u32, u32)> {
    let head = read_head(path, 4096)?;
    if let Some(d) = jpeg_dimensions(&head) {
        return Some(d);
    }
    if let Some(d) = png_dimensions(&head) {
        return Some(d);
    }
    if let Some(d) = webp_dimensions(&head) {
        return Some(d);
    }
    if crate::heic::is_heif_bytes(&head) {
        return crate::heic::dimensions(path);
    }
    // 兜底：GIF/BMP 等交给 image 的头部读取（不整解码）
    image::image_dimensions(path).ok()
}

fn read_head(path: &Path, n: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; n];
    let mut read = 0usize;
    loop {
        match f.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(k) => {
                read += k;
                if read == buf.len() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    buf.truncate(read);
    Some(buf)
}

/// JPEG：扫描 SOFn（0xC0–0xCF，排除 C4/C8/CC）读高宽
fn jpeg_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = b[i + 1];
        // 无长度字段的独立标记
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let seg_len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let is_sof = (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC;
        if is_sof && i + 9 <= b.len() {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            if w > 0 && h > 0 {
                return Some((w, h));
            }
            return None;
        }
        if seg_len < 2 {
            return None;
        }
        i += 2 + seg_len;
    }
    None
}

/// PNG：IHDR 紧跟在 8 字节签名 + 4 字节长度 + "IHDR" 之后
fn png_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
    let h = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
    if w > 0 && h > 0 { Some((w, h)) } else { None }
}

/// WebP：VP8X / VP8 / VP8L 三种块头
fn webp_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 30 || &b[0..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    match &b[12..16] {
        b"VP8X" => {
            let w = 1 + u32::from_le_bytes([b[24], b[25], b[26], 0]);
            let h = 1 + u32::from_le_bytes([b[27], b[28], b[29], 0]);
            Some((w, h))
        }
        b"VP8L" => {
            let bits = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
            let w = (bits & 0x3FFF) + 1;
            let h = ((bits >> 14) & 0x3FFF) + 1;
            Some((w, h))
        }
        b"VP8 " => {
            if b.len() < 30 {
                return None;
            }
            let w = u16::from_le_bytes([b[26], b[27]]) as u32 & 0x3FFF;
            let h = u16::from_le_bytes([b[28], b[29]]) as u32 & 0x3FFF;
            Some((w, h))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工拼一个最小 PNG 头：只需签名 + IHDR，够 `png_dimensions` 判读
    fn png_head(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]);
        v
    }

    /// 手工拼 JPEG 头：SOI + SOF0 段（含高宽）
    fn jpeg_head(w: u16, h: u16) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        v.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]); // APP0，长度 16
        v.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        v.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&[0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
        v
    }

    #[test]
    fn png_dimensions_from_header() {
        assert_eq!(png_dimensions(&png_head(4032, 3024)), Some((4032, 3024)));
    }

    #[test]
    fn jpeg_dimensions_from_header() {
        assert_eq!(jpeg_dimensions(&jpeg_head(6000, 4000)), Some((6000, 4000)));
    }

    #[test]
    fn probe_rejects_garbage() {
        assert_eq!(jpeg_dimensions(b"\xff\xd8\xff\xd9"), None);
        assert_eq!(png_dimensions(b"not a png at all, definitely"), None);
        assert_eq!(webp_dimensions(b"RIFF....WEBPxxxx"), None);
    }
}
