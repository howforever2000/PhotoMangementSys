//! 视频首帧提取（Windows Media Foundation `IMFSourceReader`）
//!
//! ## 为什么是 Media Foundation，而不是 Shell 缩略图
//!
//! 最初实现走的是 `IShellItemImageFactory::GetImage`（资源管理器画缩略图那一套），
//! 实测在本机**不可用**，且失败方式很有欺骗性：
//!
//! | flags | 结果 |
//! |---|---|
//! | `SIIGBF_THUMBNAILONLY` | `0x8004B200`（无可用缩略图） |
//! | 去掉 `THUMBNAILONLY` | 返回 512×512，但内容是**通用视频文件图标**（胶片框 + 播放三角），不是首帧 |
//!
//! 也就是说「拿得到图」不等于「拿到的是首帧」——若不做内容识别，这里会静默
//! 把一墙播放按钮当成视频封面。所以 Shell 通道被否决。
//!
//! 实测本机 `mfplat.dll` / `mfreadwrite.dll` / `mp4sdecd.dll` 齐全，
//! 于是改用 MF：`IMFSourceReader` 直接解第一个视频样本，取 RGB32 帧。
//!
//! 否决的第三条路是 ffmpeg sidecar：要分发二进制、写 `bundle.externalBin`、
//! 还要处理编解码许可，与 `heic.rs` 已确立的「不引入 C 库 / 避开 AGPL」取舍冲突。
//!
//! ## 与 HEIC 通道的关系
//!
//! `heic.rs` 走 WIC（`IWICImagingFactory`），只认**图像**编解码器；视频容器必须走 MF。
//! 两者不可合并。
//!
//! ## 失败语义
//!
//! 缺解码器（如未装 HEVC 扩展）、文件损坏、文件已删等一律返回 `Err` 可读信息，
//! 由调用方降级为「无封面占位」，不 panic、不阻塞扫描。

use std::path::Path;

#[cfg(test)]
use crate::media_kind::MediaKind;

/// 生成的封面长边（给网格留余量；最终还会被缩略图链路缩到 256）
const POSTER_EDGE: u32 = 512;

/// 把本地路径转成 MF 接受的 `file:///` URL
///
/// 为什么不能直接传裸路径：实测 `MFCreateSourceReaderFromURL` 收到 Windows 裸路径
/// 时会返回 `0xC00D36C4`（字节流类型不支持）——它把这个串当成了未知协议而非文件路径。
/// 中文 / 空格目录还要求逐段百分号编码，否则解析在非 ASCII 处就断了。
pub fn path_to_file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut encoded = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || "/:._-~".contains(c) {
            encoded.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                encoded.push_str(&format!("%{b:02X}"));
            }
        }
    }
    format!("file:///{encoded}")
}

/// 提取视频首帧，返回 RGBA 图（失败返回可读错误）
#[cfg(windows)]
pub fn extract_first_frame(path: &Path) -> Result<image::RgbaImage, String> {
    if !path.is_file() {
        return Err(format!("文件不存在：{}", path.display()));
    }
    extract_first_frame_url(&path_to_file_url(path))
}

/// 非 Windows 平台：无 Media Foundation，明确不支持
#[cfg(not(windows))]
pub fn extract_first_frame(_path: &Path) -> Result<image::RgbaImage, String> {
    Err("当前平台不支持视频首帧提取".into())
}

/// 从 `file:///` URL 提取首帧（拆出来便于单测两种 URL 写法）
#[cfg(windows)]
pub fn extract_first_frame_url(url: &str) -> Result<image::RgbaImage, String> {
    extract_first_frame_from_url(url)
}

/// Media Foundation 实现：`IMFSourceReader` 读第一帧并转 RGB32
#[cfg(windows)]
pub fn extract_first_frame_from_url(url: &str) -> Result<image::RgbaImage, String> {
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::{
        IMFAttributes, IMFMediaType, IMFSample, IMFSourceReader, MFShutdown, MFStartup,
        MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
        MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION, MFVideoFormat_RGB32, MFMediaType_Video,
        MFCreateAttributes, MFCreateMediaType, MFCreateSourceReaderFromURL, MFSTARTUP_NOSOCKET,
    };
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };

    let wide: HSTRING = url.into();
    if wide.is_empty() {
        return Err("路径为空".into());
    }

    // SAFETY: 所有 COM/MF 对象都是本函数内的局部 RAII 句柄，成对创建与释放；
    // MFStartup/MFShutdown 与 CoInitializeEx/CoUninitialize 严格配对，
    // 用 `need_*` 标志处理「调用方已初始化」的情况（Tauri 主线程通常已初始化过 COM）。
    unsafe {
        let co_hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let need_co_uninit = co_hr.is_ok();
        let mf_hr = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET);
        let need_mf_shutdown = mf_hr.is_ok();
        if mf_hr.is_err() {
            if need_co_uninit {
                CoUninitialize();
            }
            return Err(format!("Media Foundation 初始化失败：{mf_hr:?}"));
        }

        let result = (|| -> Result<image::RgbaImage, String> {
            // 打开源：URL 形式对本地文件同样适用（MF 认裸路径，中文/空格目录实测可用）
            let mut attrs_out: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs_out, 1)
                .map_err(|e| format!("创建属性失败：{e}"))?;
            let attrs = attrs_out.ok_or_else(|| "创建属性失败：空指针".to_string())?;
            // 开启视频处理：让 MF 在解码后替我们做色彩转换/缩放，
            // 免得自己去协商 NV12 → RGB32（这是本实现最省事的关键）。
            attrs
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(|e| format!("开启视频处理失败：{e}"))?;
            let reader: IMFSourceReader =
                MFCreateSourceReaderFromURL(&wide, &attrs).map_err(|e| {
                    format!("无法打开视频（可能缺少该格式的解码器）：{e}")
                })?;

            // 只要 RGB32：显式设置输出类型，避免拿到 NV12 后自己转
            let out_type: IMFMediaType =
                MFCreateMediaType().map_err(|e| format!("创建媒体类型失败：{e}"))?;
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| format!("设置主类型失败：{e}"))?;
            out_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|e| format!("设置 RGB32 失败：{e}"))?;
            reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    None,
                    &out_type,
                )
                .map_err(|e| format!("该视频无法输出 RGB32：{e}"))?;

            // 读第一个样本。返回 (stream, flags, timestamp, sample)
            let mut sample: Option<IMFSample> = None;
            let mut actual_stream = 0u32;
            let mut flags = 0u32;
            let mut ts = 0i64;
            reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    Some(&mut actual_stream),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(|e| format!("读取首帧失败：{e}"))?;
            let sample = sample.ok_or_else(|| "视频没有可读的视频帧".to_string())?;

            let media_type = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .map_err(|e| format!("读取帧格式失败：{e}"))?;
            let packed = media_type
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|e| format!("读取帧尺寸失败：{e}"))?;
            let mut w = (packed >> 32) as u32;
            let mut h = (packed & 0xFFFF_FFFF) as u32;
            if w == 0 || h == 0 {
                return Err("视频帧尺寸无效".into());
            }

            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| format!("获取帧缓冲失败：{e}"))?;
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur_len = 0u32;
            buffer
                .Lock(&mut data, Some(&mut max_len), Some(&mut cur_len))
                .map_err(|e| format!("锁定帧缓冲失败：{e}"))?;
            // RGB32 每像素 4 字节；连续缓冲的 cur_len 即有效载荷长度
            let need = (w as usize) * (h as usize) * 4;
            let copy_len = need.min(cur_len as usize);
            let slice = std::slice::from_raw_parts(data, copy_len);
            let mut buf = vec![0u8; need];
            buf[..copy_len].copy_from_slice(slice);
            let _ = buffer.Unlock();

            // MF 的 RGB32 实际字节序是 BGRA → 交换 R/B
            for px in buf.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            // 缩到 POSTER_EDGE 以内（保持比例），避免大视频占用过多内存
            let img = image::RgbaImage::from_raw(w, h, buf)
                .ok_or_else(|| "帧缓冲尺寸不匹配".to_string())?;
            if w > POSTER_EDGE || h > POSTER_EDGE {
                let sx = POSTER_EDGE as f64 / w as f64;
                let sy = POSTER_EDGE as f64 / h as f64;
                let s = sx.min(sy);
                w = ((w as f64 * s) as u32).max(1);
                h = ((h as f64 * s) as u32).max(1);
                Ok(image::imageops::resize(
                    &img,
                    w,
                    h,
                    image::imageops::FilterType::Triangle,
                ))
            } else {
                Ok(img)
            }
        })();

        if need_mf_shutdown {
            let _ = MFShutdown();
        }
        if need_co_uninit {
            CoUninitialize();
        }
        result
    }
}

/// 按媒体类型取「可当封面的一帧」
///
/// - 图片 / 动图：走既有 `imgio` 解码（动图天然只解首帧）
/// - 视频：走 [`extract_first_frame`]
///
/// 注意：生产链路的 `thumbnail::save_thumbnail` **没有**走本函数，而是直接调
/// `extract_first_frame`——因为它在同一个分支里要复用已有的 JPEG 降采样快速路径，
/// 套一层会多一次 `DynamicImage` 装箱。本函数保留给测试与未来的独立封面入口。
#[cfg(test)]
pub fn open_cover_frame(path: &Path, kind: MediaKind) -> Result<image::DynamicImage, String> {
    if kind.is_video() {
        extract_first_frame(path).map(image::DynamicImage::ImageRgba8)
    } else {
        crate::imgio::open_any(path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不存在的文件要给出可读错误而不是 panic
    #[test]
    fn missing_file_is_readable_error() {
        let r = extract_first_frame(Path::new("D:/definitely/not/here.mp4"));
        assert!(r.is_err());
    }

    /// 图片走常规解码路径，不惊动视频通道
    #[test]
    fn image_kind_uses_imgio() {
        let r = open_cover_frame(Path::new("D:/definitely/not/here.jpg"), MediaKind::Image);
        assert!(r.is_err());
    }

    /// 把任意文件当视频喂进去不能 panic（真实视频由集成测试覆盖）
    #[test]
    fn non_video_input_does_not_panic() {
        let tmp = std::env::temp_dir().join("pms_not_a_video.mp4");
        std::fs::write(&tmp, b"this is definitely not a video").unwrap();
        let r = extract_first_frame(&tmp);
        assert!(r.is_err(), "非视频内容必须报错");
        let _ = std::fs::remove_file(&tmp);
    }
}
