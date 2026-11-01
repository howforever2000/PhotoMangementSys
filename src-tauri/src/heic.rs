//! HEIC / HEIF 解码（Windows 系统编解码器通道，FEAT-142）
//!
//! ## 为什么走系统通道而不是内置解码器
//!
//! iPhone 默认存 `.heic`，而本项目所有解码路径都基于 `image` crate —— 它
//! **完全不支持 HEIF 容器**。两条补法：
//!
//! - 内置解码器（`libheif` / pure-rust `heic` crate）：前者要 vcpkg 编 C 库、
//!   还要随包分发一堆 DLL；后者是 **AGPL-3.0 或商业授权**，与本项目
//!   PolyForm-Noncommercial 分发许可冲突。两者都不合适。
//! - Windows 自带的 WIC（Windows Imaging Component）：装了
//!   「HEIF 图像扩展」后系统注册 `Microsoft HEIF Decoder`，WIC 即可解码。
//!   零新增依赖、零分发体积、解码器由系统更新维护。
//!
//! 因此这里只做一件事：**用 WIC 把 HEIF 解成 `image::DynamicImage`**，
//! 上层（缩略图 / 影调 / 编辑 / 预览）一律不改调用方式。
//!
//! ## 降级约定
//!
//! 系统没有 HEIF 解码器时**不能假装能用**：
//! - [`support`] 供前端做能力探针（界面给出「装扩展」的提示）；
//! - [`decode`] 返回带可操作建议的错误串，由扫描报告/日志如实呈现。
//!
//! EXIF 不受此影响：`kamadak-exif` 原生支持 HEIF 容器（`read_from_container`），
//! 所以**即使解不出像素，拍摄时间 / GPS / 设备型号照样能入库**。

use std::path::Path;

/// 支持的 HEIF 扩展名（小写；`.hif` 见于部分国产机导出）
pub const HEIC_EXTS: &[&str] = &["heic", "heif", "hif"];

/// HEIF 家族的 `ftyp` brand（major brand 与 compatible brands 共用同一张表）
///
/// 依据 ISO/IEC 23008-12 与各家实现：`mif1/msf1` 为通用 HEIF，
/// `heic/heix/hevc/hevx/heim/heis/hevm/hevs` 为 HEVC 编码的 HEIC，
/// `mif2` 为 HEIF 第二版。
const HEIF_BRANDS: &[[u8; 4]] = &[
    *b"mif1", *b"mif2", *b"msf1", *b"heic", *b"heix", *b"hevc", *b"hevx", *b"heim", *b"heis",
    *b"hevm", *b"hevs",
];

/// 解码能力探针结果（前端据此决定是否提示「安装 HEIF 图像扩展」）
#[derive(Debug, Clone, serde::Serialize)]
pub struct HeicSupport {
    /// 当前系统能否解码 HEIC
    pub available: bool,
    /// 命中的解码器友好名（如 `Microsoft HEIF Decoder`）
    pub codec: Option<String>,
    /// 不可用时的原因（中文、可直接展示）
    pub reason: Option<String>,
}

/// 是否是 HEIC / HEIF 文件（**按扩展名**，仅用于分流，不判内容）
pub fn is_heif_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| HEIC_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

/// 是否是 HEIF 容器（**按文件头魔数**，不信扩展名）
///
/// 只看 ISO-BMFF 的 `ftyp` box：`[4 字节长度][ftyp][major brand][minor][compat…]`。
/// 命中 major brand 或任一 compatible brand 即视为 HEIF。
pub fn is_heif_bytes(buf: &[u8]) -> bool {
    // 至少要容纳 ftyp 头 + major brand
    if buf.len() < 12 || &buf[4..8] != b"ftyp" {
        return false;
    }
    let brand_ok = |b: &[u8]| HEIF_BRANDS.iter().any(|known| known.as_slice() == b);
    // AVIF 同样基于 ISOBMFF 且常把 `mif1` 写进兼容列表，但编码是 AV1 ——
    // 不能拿去走 HEIC（HEVC）解码路径，否则错误提示会误导用户装错扩展。
    if &buf[8..12] == b"avif" || &buf[8..12] == b"avis" {
        return false;
    }
    if brand_ok(&buf[8..12]) {
        return true;
    }
    // compatible brands 从偏移 16 开始，每 4 字节一个，直到 box 末尾
    let box_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let end = box_len.min(buf.len()).max(16);
    let mut off = 16usize;
    while off + 4 <= end {
        if brand_ok(&buf[off..off + 4]) {
            return true;
        }
        off += 4;
    }
    false
}

/// 系统 HEIC 解码能力（Windows：枚举 WIC 是否注册了 HEIF 解码器）
pub fn support() -> HeicSupport {
    platform::support()
}

/// 解码一张 HEIC/HEIF 为 `DynamicImage`（RGBA8）
///
/// 失败时返回的错误串包含**可操作建议**：用户看到的不该只是
/// `WINCODEC_ERR_COMPONENTNOTFOUND`。
pub fn decode(path: &Path) -> Result<image::DynamicImage, String> {
    platform::decode(path)
}

/// 只读尺寸（不解码像素，用于「分辨率优先」的保留策略与信息面板）
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    platform::dimensions(path)
}

// ===========================================================================
// Windows：WIC 实现
// ===========================================================================
#[cfg(windows)]
mod platform {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::{Interface, GUID, PCWSTR};
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{
        IWICBitmapCodecInfo, IWICBitmapDecoder, IWICBitmapSource, IWICImagingFactory,
        CLSID_WICImagingFactory, GUID_ContainerFormatHeif, GUID_WICPixelFormat32bppBGRA,
        WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICComponentEnumerateDefault,
        WICDecodeMetadataCacheOnLoad, WICDecoder,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };

    use super::HeicSupport;

    /// 进程内只需成功初始化一次 COM（重复调用返回 RPC_E_CHANGED_MODE 也无需处理）
    fn ensure_com() {
        // SAFETY: 纯 FFI 调用；返回值忽略是刻意为之（已初始化/模式不同都算「可用」）
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    fn wide(path: &Path) -> Vec<u16> {
        OsStr::new(path.as_os_str())
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn factory() -> Result<IWICImagingFactory, String> {
        ensure_com();
        // SAFETY: COM 已初始化；CLSID 为 WIC 官方工厂
        unsafe {
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("WIC 工厂创建失败: {e}"))
        }
    }

    fn open_decoder(path: &Path) -> Result<IWICBitmapDecoder, String> {
        let f = factory()?;
        let w = wide(path);
        // SAFETY: `w` 的生命周期覆盖调用；只读打开
        unsafe {
            f.CreateDecoderFromFilename(
                PCWSTR::from_raw(w.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnLoad,
            )
        }
        .map_err(|e| {
            format!(
                "HEIC 解码失败（本机缺少 HEIF 解码器？请在 Microsoft Store 安装「HEIF 图像扩展」\
                 与「HEVC 视频扩展」后重试）: {e}"
            )
        })
    }

    pub(super) fn support() -> HeicSupport {
        let probe = (|| -> Result<Option<String>, String> {
            let f = factory()?;
            // SAFETY: 枚举注册的 WIC 组件；只读
            unsafe {
                let e = f
                    .CreateComponentEnumerator(WICDecoder.0 as u32, WICComponentEnumerateDefault.0 as u32)
                    .map_err(|e| format!("枚举 WIC 解码器失败: {e}"))?;
                let mut buf: [Option<windows::core::IUnknown>; 1] = [None];
                let mut fetched = 0u32;
                loop {
                    // Next 返回 HRESULT：S_FALSE 表示枚举结束
                    let hr = e.Next(&mut buf, Some(&mut fetched));
                    if hr.is_err() || fetched == 0 {
                        break;
                    }
                    let Some(unk) = buf[0].take() else { break };
                    let Ok(info) = unk.cast::<IWICBitmapCodecInfo>() else { continue };
                    let Ok(fmt) = info.GetContainerFormat() else { continue };
                    if fmt == GUID_ContainerFormatHeif {
                        return Ok(Some(friendly_name(&info)));
                    }
                }
            }
            Ok(None)
        })();

        match probe {
            Ok(Some(codec)) => HeicSupport { available: true, codec: Some(codec), reason: None },
            Ok(None) => HeicSupport {
                available: false,
                codec: None,
                reason: Some(
                    "本机未注册 HEIF 解码器：请在 Microsoft Store 安装「HEIF 图像扩展」\
                     （HEVC 编码的 iPhone 照片还需「HEVC 视频扩展」）后重启应用"
                        .into(),
                ),
            },
            Err(e) => HeicSupport { available: false, codec: None, reason: Some(e) },
        }
    }

    fn friendly_name(info: &IWICBitmapCodecInfo) -> String {
        let mut buf = vec![0u16; 256];
        let mut actual = 0u32;
        // SAFETY: 缓冲区长度与容量一致
        let ok = unsafe { info.GetFriendlyName(&mut buf, &mut actual) }.is_ok();
        if !ok {
            return "WIC HEIF Decoder".into();
        }
        let n = (actual as usize).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        let s = s.trim_matches('\0').trim().to_string();
        if s.is_empty() { "WIC HEIF Decoder".into() } else { s }
    }

    pub(super) fn dimensions(path: &Path) -> Option<(u32, u32)> {
        let decoder = open_decoder(path).ok()?;
        let frame = unsafe { decoder.GetFrame(0) }.ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        unsafe { frame.GetSize(&mut w, &mut h) }.ok()?;
        if w == 0 || h == 0 { None } else { Some((w, h)) }
    }

    pub(super) fn decode(path: &Path) -> Result<image::DynamicImage, String> {
        let decoder = open_decoder(path)?;
        // 容器校验：确认确实走了 HEIF 解码器（避免把别的 ISO-BMFF 当图片）
        let fmt = unsafe { decoder.GetContainerFormat() }.map_err(|e| format!("读取容器格式失败: {e}"))?;
        if fmt != GUID_ContainerFormatHeif {
            return Err("该文件的容器格式不是 HEIF，已拒绝按 HEIC 解码".into());
        }
        let frame = unsafe { decoder.GetFrame(0) }.map_err(|e| format!("读取 HEIC 首帧失败: {e}"))?;
        let (mut w, mut h) = (0u32, 0u32);
        unsafe { frame.GetSize(&mut w, &mut h) }.map_err(|e| format!("读取 HEIC 尺寸失败: {e}"))?;
        if w == 0 || h == 0 {
            return Err("HEIC 尺寸为 0，文件可能已损坏".into());
        }
        let src: IWICBitmapSource = frame.cast().map_err(|e| format!("HEIC 位图源转换失败: {e}"))?;
        let f = factory()?;
        let conv = unsafe { f.CreateFormatConverter() }.map_err(|e| format!("创建像素格式转换器失败: {e}"))?;
        // SAFETY: 全部为 WIC 只读接口调用；BGRA 每像素恒定 4 字节
        unsafe {
            conv.Initialize(
                &src,
                &GUID_WICPixelFormat32bppBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(|e| format!("HEIC 像素格式转换失败: {e}"))?;
        }
        let stride = w.checked_mul(4).ok_or_else(|| "HEIC 尺寸过大".to_string())?;
        let total = (stride as usize)
            .checked_mul(h as usize)
            .ok_or_else(|| "HEIC 尺寸过大".to_string())?;
        let mut buf = vec![0u8; total];
        // SAFETY: 缓冲区按 stride×height 精确分配
        unsafe { conv.CopyPixels(std::ptr::null(), stride, &mut buf) }
            .map_err(|e| format!("HEIC 像素读取失败: {e}"))?;
        // BGRA → RGBA（原地交换 R/B）
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let img = image::RgbaImage::from_raw(w, h, buf).ok_or_else(|| "HEIC 像素缓冲无效".to_string())?;
        Ok(image::DynamicImage::ImageRgba8(img))
    }

    /// `GUID` 未直接使用时抑制告警（供未来扩展容器判断）
    #[allow(dead_code)]
    fn _unused(_g: GUID) {}
}

// ===========================================================================
// 非 Windows：明确报告不可用（不静默失败）
// ===========================================================================
#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use super::HeicSupport;

    pub(super) fn support() -> HeicSupport {
        HeicSupport {
            available: false,
            codec: None,
            reason: Some("当前平台未接入 HEIF 解码器（仅 Windows 支持 HEIC 解码）".into()),
        }
    }

    pub(super) fn dimensions(_path: &Path) -> Option<(u32, u32)> {
        None
    }

    pub(super) fn decode(_path: &Path) -> Result<image::DynamicImage, String> {
        Err("当前平台未接入 HEIF 解码器（仅 Windows 支持 HEIC 解码）".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp_box(major: &[u8; 4], compat: &[&[u8; 4]]) -> Vec<u8> {
        let mut v = Vec::new();
        let len = 16 + compat.len() * 4;
        v.extend_from_slice(&(len as u32).to_be_bytes());
        v.extend_from_slice(b"ftyp");
        v.extend_from_slice(major);
        v.extend_from_slice(&[0, 0, 0, 0]);
        for c in compat {
            v.extend_from_slice(*c);
        }
        v
    }

    #[test]
    fn detects_heic_major_brand() {
        assert!(is_heif_bytes(&ftyp_box(b"heic", &[b"mif1", b"heic"])));
        assert!(is_heif_bytes(&ftyp_box(b"mif1", &[b"mif1", b"miaf"])));
    }

    #[test]
    fn detects_heif_via_compatible_brand() {
        // 国产机/相机常见：major 是 `hevc`，compat 里才出现 `mif1`
        assert!(is_heif_bytes(&ftyp_box(b"hevc", &[b"mif1", b"hevc"])));
    }

    #[test]
    fn rejects_non_heif_isobmff() {
        // avif / mp4 不能误判成 HEIC（avif 常带 mif1 兼容 brand，必须按 major brand 排除）
        assert!(!is_heif_bytes(&ftyp_box(b"mp41", &[b"mp41", b"isom"])));
        assert!(!is_heif_bytes(&ftyp_box(b"avif", &[b"avif", b"mif1"]).as_slice()));
        assert!(!is_heif_bytes(&ftyp_box(b"avis", &[b"avis", b"mif1"]).as_slice()));
        // 太短 / 不是 ftyp
        assert!(!is_heif_bytes(b"\0\0\0\x10ftyp"));
        assert!(!is_heif_bytes(b"\xff\xd8\xff\xe0 JFIF....."));
    }

    #[test]
    fn extension_check_is_case_insensitive() {
        assert!(is_heif_path(Path::new("a.HEIC")));
        assert!(is_heif_path(Path::new("b.heif")));
        assert!(is_heif_path(Path::new("c.HIF")));
        assert!(!is_heif_path(Path::new("d.jpg")));
        assert!(!is_heif_path(Path::new("heic")));
    }

    /// 真实样本验证（需要系统装有 HEIF/HEVC 扩展）：
    /// `PMS_HEIC_SAMPLE=D:\path\x.heic cargo test --lib -- --ignored heic`
    #[test]
    #[ignore = "需要真实 HEIC 样本：设置 PMS_HEIC_SAMPLE"]
    fn decodes_real_heic_sample() {
        let Some(p) = std::env::var_os("PMS_HEIC_SAMPLE") else {
            eprintln!("未设置 PMS_HEIC_SAMPLE，跳过");
            return;
        };
        let path = std::path::PathBuf::from(p);
        let bytes = std::fs::read(&path).expect("读取样本失败");
        assert!(is_heif_bytes(&bytes), "样本应按魔数识别为 HEIF");
        let img = decode(&path).expect("应能解出像素（系统 HEIF 解码器可用）");
        assert!(img.width() > 100 && img.height() > 100, "尺寸异常: {img:?}");
        assert_eq!(dimensions(&path), Some((img.width(), img.height())), "头部尺寸应与解码一致");
        // EXIF 走 kamadak-exif（HEIF 容器原生支持）—— 解不出像素也不该影响元数据。
        // 注意：样本可能本就没写 EXIF（返回 NotFound("HEIF") 属正常，不是解码失败）
        let mut buf = std::io::BufReader::new(std::fs::File::open(&path).unwrap());
        let exif_note = match exif::Reader::new().read_from_container(&mut buf) {
            Ok(exif) => format!("{} 个字段", exif.fields().count()),
            Err(e) => format!("无 EXIF（{e}）"),
        };
        eprintln!(
            "codec={:?} size={}x{} exif={exif_note}",
            support().codec,
            img.width(),
            img.height()
        );
    }
}
