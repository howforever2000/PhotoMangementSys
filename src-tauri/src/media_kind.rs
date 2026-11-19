//! 媒体类型判定 —— **全项目唯一真相点**
//!
//! ## 为什么要独立成模块
//!
//! 改造前，扩展名白名单被复制在三处且互不知晓：
//! `photo_scan.rs::IMAGE_EXTS`、`thumbnail.rs::IMAGE_EXTS`、`vision.rs::IMAGE_EXTS`
//! （`photo_scan.rs` 的注释还写着「为解耦本地复制一份」）。这三份副本一旦漂移，
//! 表现是「某个入口看不到某些文件」，极难排查。
//!
//! 现在统一由本模块给出判定，各模块按需要**筛选**而不是各自维护列表：
//! - 需要「能解码出一张图」的地方（EXIF / 影调 / AI 识别 / 去重）→ [`is_decodable_image`]
//! - 需要「所有可视媒体」的地方（相册浏览 / 统计 / 缩略图）→ [`is_media`]
//!
//! ## 为什么动图要单独成一类
//!
//! gif / 动态 webp 在解码层面**天然只有首帧**（`image` crate 的 `GifDecoder`
//! 与 `image-webp` 的 `read_image` 都只解第一帧），所以缩略图链路不需要任何改造
//! 就能出封面。但「它是不是动图」这个信息必须显式存在，否则：
//! - 无法在网格上给播放角标；
//! - 无法让 AI / 影调跳过（对动图跑 CLIP 只看到首帧，结论不可靠还白耗算力）；
//! - 统计口径无法区分。

use std::path::Path;

/// 静态图片扩展名（不含动图与视频）
pub const STILL_IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "bmp", "heic", "heif", "tif", "tiff"];

/// 动图扩展名（gif 与动态 webp；静态 webp 也归入此类，见 [`kind_of_ext`] 注释）
pub const ANIMATED_EXTS: &[&str] = &["gif", "webp"];

/// 视频扩展名
///
/// 只收 WebView2（Chromium）与 Windows 系统编解码器都常见的容器。
/// **故意不收** `mkv` / `avi` / `3gp`：
/// - Tauri 的 asset 协议按魔数嗅探 MIME，嗅探不出来会退回 `text/html`，
///   浏览器拿到 `text/html` 会直接拒绝播放；
/// - 收进来只会让用户看到「能导入但打不开」的坏体验。
pub const VIDEO_EXTS: &[&str] = &["mp4", "mov", "m4v", "wmv", "webm"];

/// 媒体类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// 静态图片（含 heic/heif）
    Image,
    /// 动图（gif / webp）。webp 可能是静态的，但判「是否真的多帧」需要解码整帧表，
    /// 成本与收益不成正比（见模块文档），故一致性归入此类。
    AnimatedImage,
    /// 视频
    Video,
}

impl MediaKind {
    /// 数据库/前端序列化用的稳定标识（**改动即破坏兼容**，勿随意改）
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::AnimatedImage => "animated",
            MediaKind::Video => "video",
        }
    }

    /// 是否为视频（统计口径、封面策略、播放角标都用它）
    pub fn is_video(self) -> bool {
        matches!(self, MediaKind::Video)
    }
}

/// 从扩展名（小写、不含点）判定媒体类型；不是可视媒体则返回 `None`
pub fn kind_of_ext(ext: &str) -> Option<MediaKind> {
    let e = ext.to_ascii_lowercase();
    if VIDEO_EXTS.contains(&e.as_str()) {
        Some(MediaKind::Video)
    } else if ANIMATED_EXTS.contains(&e.as_str()) {
        Some(MediaKind::AnimatedImage)
    } else if STILL_IMAGE_EXTS.contains(&e.as_str()) {
        Some(MediaKind::Image)
    } else {
        None
    }
}

/// 从路径判定媒体类型；不是可视媒体则返回 `None`
pub fn kind_of(path: impl AsRef<Path>) -> Option<MediaKind> {
    let p = path.as_ref();
    let ext = p.extension()?.to_str()?;
    kind_of_ext(ext)
}

/// 从文件名（或任意含扩展名的字符串）判定媒体类型
pub fn kind_of_name(name: &str) -> Option<MediaKind> {
    let ext = match name.rsplit_once('.') {
        Some((_, e)) => e,
        None => return None,
    };
    kind_of_ext(ext)
}

/// 是否**静态**图片（严格：不含动图、不含视频）
///
/// 供「只看一张静态图」的链路使用。
///
/// 当前唯一需要「严格静态」的判定发生在测试与人工排查；生产链路的目录遍历统一走
/// [`is_decodable_image`]（动图也能解出首帧，故与图片同列），视频则由
/// [`kind_of`] 分流到首帧提取。保留本函数是为了让「静态 vs 能解码」这个语义差别
/// 有明确落点，未来若影调/EXIF 需要排除动图，改调用它即可。
#[cfg(test)]
pub fn is_still_image(path: impl AsRef<Path>) -> bool {
    kind_of(path) == Some(MediaKind::Image)
}

/// 是否可视媒体（静态图 / 动图 / 视频）
///
/// 供相册浏览、统计、缩略图生成使用。
pub fn is_media(path: impl AsRef<Path>) -> bool {
    kind_of(path).is_some()
}

/// 是否「能按单帧解码」的图片（静态图 + 动图，**不含视频**）
///
/// 供 EXIF / 影调 / AI 识别 / 去重这类「喂给解码器就能拿到像素」的链路使用。
/// 动图在这里算「能解」，因为解码器天然只返回首帧。
pub fn is_decodable_image(path: impl AsRef<Path>) -> bool {
    matches!(
        kind_of(path),
        Some(MediaKind::Image) | Some(MediaKind::AnimatedImage)
    )
}

/// 是否「能按单帧解码」的图片（按文件名判定，等价于 [`is_decodable_image`]）
pub fn is_decodable_image_name(name: &str) -> bool {
    matches!(
        kind_of_name(name),
        Some(MediaKind::Image) | Some(MediaKind::AnimatedImage)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn still_images_classified() {
        for p in ["a.jpg", "a.JPEG", "a.png", "a.heic", "a.heif", "a.bmp", "a.tif"] {
            assert_eq!(kind_of(p), Some(MediaKind::Image), "{p}");
            assert!(is_still_image(p));
        }
    }

    #[test]
    fn animated_classified() {
        for p in ["a.gif", "a.GIF", "a.webp"] {
            assert_eq!(kind_of(p), Some(MediaKind::AnimatedImage), "{p}");
            assert!(!is_still_image(p));
            assert!(is_media(p));
        }
    }

    #[test]
    fn video_classified() {
        for p in ["a.mp4", "a.MOV", "a.m4v", "a.wmv", "a.webm"] {
            assert_eq!(kind_of(p), Some(MediaKind::Video), "{p}");
            assert!(!is_still_image(p));
            assert!(is_media(p));
        }
    }

    /// 视频不能混进「静态图」判定 —— 否则 AI/影调链路会拿到 mp4 去解码
    #[test]
    fn video_is_not_still_image() {
        assert!(!is_still_image("clip.mp4"));
        assert!(!is_still_image("clip.mov"));
    }

    /// 故意不收的容器（MIME 嗅探不出会退回 text/html）
    #[test]
    fn exotic_containers_rejected() {
        for p in ["a.mkv", "a.avi", "a.3gp"] {
            assert_eq!(kind_of(p), None, "{p} 不应被收进来");
            assert!(!is_media(p));
        }
    }

    #[test]
    fn non_media_rejected() {
        for p in ["a.txt", "a.pdf", "a", "a.", "README"] {
            assert_eq!(kind_of(p), None, "{p}");
        }
    }

    /// 路径与文件名两种入口结论一致（大小写、目录名不干扰）
    #[test]
    fn path_and_name_agree() {
        assert_eq!(kind_of("/x/Screenshot_1.PNG"), Some(MediaKind::Image));
        assert_eq!(kind_of_name("Screenshot_1.PNG"), Some(MediaKind::Image));
        // 目录里有 ".mp4" 字样不应影响，只看最后一段扩展名
        assert_eq!(kind_of("/x/mp4/photo.jpg"), Some(MediaKind::Image));
    }

    /// 序列化标识稳定（前端按这几个字符串分支）
    #[test]
    fn as_str_is_stable() {
        assert_eq!(MediaKind::Image.as_str(), "image");
        assert_eq!(MediaKind::AnimatedImage.as_str(), "animated");
        assert_eq!(MediaKind::Video.as_str(), "video");
    }
}
