//! 头像图片处理工具（FEAT-045）
//!
//! 中心方裁 + 缩放 + JPEG 落盘，供两类头像复用：
//! - 用户头像（`set_user_avatar`）：256×256
//! - 人物自选头像（`set_person_avatar_from_photo`）：96×96（与自动裁剪一致）
//!
//! JPEG 走 jpeg-decoder 分级降采样快速路径（与 persons.rs 代表脸裁剪同源思路），
//! 避免 6000×4000 大图全尺寸解码卡顿（BUG-2026-0815-003 更换封面卡顿的教训）。

use std::path::Path;

/// 中心方裁：读图 → 取中心正方形 → 缩放 `size×size` → JPEG 落盘到 `dst`
///
/// 输入支持项目已启用的格式（jpg/jpeg/png/webp/gif/bmp）；输出统一 JPEG。
pub fn crop_square(src: &Path, dst: &Path, size: u32) -> Result<(), String> {
    if !src.is_file() {
        return Err(format!("图片不存在: {}", src.display()));
    }
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let is_jpeg = name.ends_with(".jpg") || name.ends_with(".jpeg");

    let cropped = if is_jpeg {
        crop_square_jpeg(src, size)?
    } else {
        let img = image::open(src).map_err(|e| format!("图片解码失败: {e}"))?;
        let side = img.width().min(img.height()).max(1);
        let x = (img.width() - side) / 2;
        let y = (img.height() - side) / 2;
        img.crop_imm(x, y, side, side)
    };

    let out = cropped.resize_exact(size, size, image::imageops::FilterType::Triangle);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建头像目录失败: {e}"))?;
    }
    out.save_with_format(dst, image::ImageFormat::Jpeg)
        .map_err(|e| format!("保存头像失败: {e}"))
}

/// JPEG 快速路径：按目标 `size` 选最大可用降采样档（1/8、1/4、1/2、1/1），
/// 解码后中心方裁坐标在实际解码尺寸上直接计算（无需换算回原图）。
fn crop_square_jpeg(src: &Path, size: u32) -> Result<image::DynamicImage, String> {
    // 先读头拿原尺寸
    let probe = std::fs::File::open(src).map_err(|e| format!("打开图片失败: {e}"))?;
    let mut head = jpeg_decoder::Decoder::new(std::io::BufReader::new(probe));
    let _ = head.read_info();
    let info = head.info().ok_or("无法读取图片头信息")?;
    let (w0, h0) = (info.width as u32, info.height as u32);

    // 分级选档：降采样后短边仍 ≥ size 的最大档（jpeg-decoder 仅支持 2 的幂档位）
    let mut chosen = 1u32;
    for &d in &[8u32, 4, 2] {
        if (w0.min(h0)) as f64 / d as f64 >= size as f64 {
            chosen = d;
            break;
        }
    }
    let tw = ((w0 + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;
    let th = ((h0 + chosen - 1) / chosen).clamp(1, u16::MAX as u32) as u16;

    let file2 = std::fs::File::open(src).map_err(|e| format!("打开图片失败: {e}"))?;
    let mut dec = jpeg_decoder::Decoder::new(std::io::BufReader::new(file2));
    let _ = dec.scale(tw, th);
    let pixels = dec.decode().map_err(|e| format!("JPEG 解码失败: {e:?}"))?;
    let info2 = dec.info().ok_or("无法读取解码信息")?;
    let aw = (info2.width as u32).max(1);
    let ah = (info2.height as u32).max(1);
    let side = aw.min(ah);
    let x = (aw - side) / 2;
    let y = (ah - side) / 2;
    Ok(image::DynamicImage::ImageRgb8(
        image::RgbImage::from_raw(aw, ah, pixels).ok_or("像素数据长度不符")?,
    )
    .crop_imm(x, y, side, side))
}


pub mod commands {
use std::path::PathBuf;
use tauri::Manager;

// =====================================================================
// 以下命令自 lib.rs 迁入（lib.rs 瘦身）：用户/人物头像命令层
// =====================================================================


/// 人物头像缓存目录（app_data_dir/avatars）
pub fn avatars_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {e}"))?;
    Ok(data_dir.join("avatars"))
}


/// FEAT-045：设置当前用户头像
///
/// 输入本地图片绝对路径（前端 plugin-dialog 选择）→ 中心方裁 256×256 JPEG →
/// `app_data/avatars/user_{id}.jpg` → 写库返回更新后的用户。
/// 图片解码在阻塞线程执行，避免大图占用异步运行时。
#[tauri::command]
pub async fn set_user_avatar(
    source_path: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<crate::auth::User, String> {
    let _t = log_call!("set_user_avatar", &format!("source={source_path}"));
    let user_id = crate::require_user(&session)?;
    if !std::path::Path::new(&source_path).is_file() {
        crate::logger::log_call_end_with("set_user_avatar", _t, "ERR | 图片不存在");
        return Err("所选图片不存在".into());
    }
    let dir = avatars_dir(&app)?;
    let avatar_path = dir.join(format!("user_{user_id}.jpg"));
    let avatar_path_str = avatar_path.to_string_lossy().into_owned();
    let src = source_path;
    let cropped = tauri::async_runtime::spawn_blocking(move || {
        crate::avatar::crop_square(
            std::path::Path::new(&src),
            std::path::Path::new(&avatar_path_str),
            256,
        )
    })
    .await
    .map_err(|e| format!("头像任务线程失败: {e}"))?;
    if let Err(e) = cropped {
        crate::logger::log_call_end_with("set_user_avatar", _t, &format!("ERR | {e}"));
        return Err(e);
    }
    // 覆盖写同一路径：前端展示需带时间戳参数破 webview 图片缓存
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let r = crate::auth::update_user_avatar(db.conn(), user_id, Some(avatar_path.to_string_lossy().into_owned()));
    match &r {
        Ok(u) => crate::logger::log_call_end_with("set_user_avatar", _t, &format!("OK | id={}", u.id)),
        Err(e) => crate::logger::log_call_end_with("set_user_avatar", _t, &format!("ERR | {e}")),
    }
    r
}


/// FEAT-045：移除当前用户头像（删文件 + 库内置空），返回更新后的用户
#[tauri::command]
pub async fn clear_user_avatar(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<crate::auth::User, String> {
    let _t = log_call!("clear_user_avatar", "");
    let user_id = crate::require_user(&session)?;
    if let Ok(dir) = avatars_dir(&app) {
        let _ = std::fs::remove_file(dir.join(format!("user_{user_id}.jpg")));
    }
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let r = crate::auth::update_user_avatar(db.conn(), user_id, None);
    match &r {
        Ok(u) => crate::logger::log_call_end_with("clear_user_avatar", _t, &format!("OK | id={}", u.id)),
        Err(e) => crate::logger::log_call_end_with("clear_user_avatar", _t, &format!("ERR | {e}")),
    }
    r
}


/// 获取人物头像（本地优先：磁盘缓存命中直接返回，未命中则从代表脸 bbox 本地裁剪）
///
/// 完全离线可用，不再依赖 Python 微服务。
#[tauri::command]
pub async fn get_person_avatar(
    pid: String,
    force_refresh: Option<bool>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let dir = avatars_dir(&app)?;
    let cache_path = dir.join(format!("avatar_{pid}.jpg"));
    // 缓存命中不写日志（BUG-2026-0910-002）：画廊一次拉几百个头像，此前每次命中都写
    // CALL+RET 两行，累计 15 万行噪音把 app.log 刷到 15MB+，日志副窗口也被洪水冲垮；
    // 只保留真正值得看的事件（现场裁剪/出错）。
    if !force_refresh.unwrap_or(false) && cache_path.is_file() {
        return Ok(cache_path.to_string_lossy().into_owned());
    }
    let _t = log_call!("get_person_avatar", &format!("pid={pid} force={force_refresh:?}"));
    // 裁剪解码在阻塞线程执行，避免大图解码占用异步运行时
    let r = tauri::async_runtime::spawn_blocking(move || {
        crate::persons::crop_avatar_local(&pid, &cache_path)?;
        Ok(cache_path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| format!("头像任务线程失败: {e}"))?;
    match &r {
        Ok(_) => crate::logger::log_call_end_with("get_person_avatar", _t, "OK | cropped"),
        Err(e) => crate::logger::log_call_end_with("get_person_avatar", _t, &format!("ERR | {e}")),
    }
    r
}


/// 批量取人物头像路径（仅查本地缓存：命中返回路径，未命中返回 null）
///
/// BUG-2026-0910-002 根治：画廊人物已有 ~900 个，逐个 invoke = 每次 900 次 IPC
/// 往返 + 峰值 3600 行日志/秒（日志副窗口被洪水冲垮、主窗口被风暴拖慢）。
/// 改为单次批量：后端本地逐个 stat 缓存文件，一次往返全部带回、只写 1 行日志。
/// 未命中（首次出现的人物，需现场裁剪）返回 null，前端占位，点击该人物时
/// 再走 get_person_avatar 按需裁剪。
#[tauri::command]
pub async fn get_person_avatars_bulk(
    pids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<Vec<Option<String>>, String> {
    let _t = log_call!("get_person_avatars_bulk", &format!("count={}", pids.len()));
    let dir = avatars_dir(&app)?;
    let r = tauri::async_runtime::spawn_blocking(move || {
        pids.iter()
            .map(|pid| {
                let p = dir.join(format!("avatar_{pid}.jpg"));
                if p.is_file() {
                    Some(p.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| format!("批量头像任务线程失败: {e}"))?;
    let hit = r.iter().filter(|x| x.is_some()).count();
    crate::logger::log_call_end_with(
        "get_person_avatars_bulk",
        _t,
        &format!("OK | hit={hit} miss={}", r.len() - hit),
    );
    Ok(r)
}


/// FEAT-047：人物自选头像 —— 用户在人物照片弹窗指定一张照片作为头像封面
///
/// 中心方裁 96×96 JPEG 覆盖 `avatars/avatar_{pid}.jpg`（get_person_avatar 的
/// is_file() 缓存命中自选结果，优先于代表脸自动裁剪）。解码在阻塞线程执行。
#[tauri::command]
pub async fn set_person_avatar_from_photo(
    pid: String,
    photo_path: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let _t = log_call!("set_person_avatar_from_photo", &format!("pid={pid}"));
    if !std::path::Path::new(&photo_path).is_file() {
        crate::logger::log_call_end_with("set_person_avatar_from_photo", _t, "ERR | 原图不存在");
        return Err("所选照片不存在".into());
    }
    let cache_str = avatars_dir(&app)?
        .join(format!("avatar_{pid}.jpg"))
        .to_string_lossy()
        .into_owned();
    let src = photo_path;
    let dst = cache_str.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        crate::persons::set_avatar_from_photo(
            std::path::Path::new(&src),
            std::path::Path::new(&dst),
        )
    })
    .await
    .map_err(|e| format!("头像任务线程失败: {e}"))?;
    match &r {
        Ok(_) => crate::logger::log_call_end_with("set_person_avatar_from_photo", _t, "OK | custom"),
        Err(e) => crate::logger::log_call_end_with("set_person_avatar_from_photo", _t, &format!("ERR | {e}")),
    }
    r?;
    Ok(cache_str)
}

}

#[cfg(test)]
mod tests {
    use super::*;

    /// 非 JPEG（PNG）中心方裁 + JPEG 快速路径均应产出 size×size JPEG
    #[test]
    fn crop_square_png_and_jpeg() {
        let tmp = std::env::temp_dir().join(format!("avatar_crop_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // PNG：非正方形 160×80 → 中心方裁 64×64
        let png = tmp.join("in.png");
        image::RgbImage::from_fn(160, 80, |x, _| {
            if x < 80 {
                image::Rgb([255u8, 0, 0])
            } else {
                image::Rgb([0, 0, 255u8])
            }
        })
        .save(&png)
        .unwrap();
        let dst_png = tmp.join("out.png.jpg");
        crop_square(&png, &dst_png, 64).unwrap();
        let img = image::open(&dst_png).unwrap();
        assert_eq!((img.width(), img.height()), (64, 64));

        // JPEG：1600×800 走快速路径 → 96×96
        let jpg = tmp.join("in.jpg");
        image::RgbImage::from_fn(1600, 800, |_, _| image::Rgb([0u8, 128, 0]))
            .save(&jpg)
            .unwrap();
        let dst_jpg = tmp.join("out.jpg");
        crop_square(&jpg, &dst_jpg, 96).unwrap();
        let img2 = image::open(&dst_jpg).unwrap();
        assert_eq!((img2.width(), img2.height()), (96, 96));

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 源文件不存在应报错而非 panic
    #[test]
    fn crop_square_missing_source_errors() {
        let r = crop_square(
            Path::new("/nonexistent/avatar_src.jpg"),
            Path::new("/tmp/avatar_dst.jpg"),
            96,
        );
        assert!(r.is_err());
    }
}
