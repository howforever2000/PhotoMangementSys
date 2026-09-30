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


// =====================================================================
// 头像缓存的身份纪律（BUG-2026-1001-001）
//
// 背景：缓存文件命名为 avatar_<pid>.jpg，而 P 编号会在人物库被重建/清空后
// **被新的人物复用**（旧文件留在磁盘上）。旧实现只判断“文件在不在”就直接
// 返回 → 人物卡片显示的是别人（实测 2417 个头像里 1952 个比其人物的创建时间
// 还早，抽查 P582/P169 缓存里的脸与库里代表脸不是同一人）。
//
// 修法：给自动头像配一份**身份签名**（="<该人物脸张数>:<最大 face id>"，
// 人脸增删/合并/重建都会变），读取时校验；对不上就删掉重裁。用户自选头像
// 用 .custom 标记，无条件优先且不被自动重裁覆盖。
// =====================================================================

/// 头像缓存三件套路径：(jpg, 签名, 自选标记)
pub fn avatar_paths(dir: &Path, pid: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    (
        dir.join(format!("avatar_{pid}.jpg")),
        dir.join(format!("avatar_{pid}.sig")),
        dir.join(format!("avatar_{pid}.custom")),
    )
}

/// 从缓存文件名解析 pid（`avatar_P001.jpg` / `.sig` / `.custom` → `P001`）
fn pid_of_avatar_file(name: &str) -> Option<String> {
    let rest = name.strip_prefix("avatar_")?;
    let pid = rest.split('.').next().unwrap_or("");
    if pid.is_empty() {
        None
    } else {
        Some(pid.to_string())
    }
}

/// 删掉某个人物的全部头像缓存文件（jpg/签名/自选标记），返回删掉的文件数
pub fn remove_avatar_cache(dir: &Path, pid: &str) -> u32 {
    let (jpg, sig, custom) = avatar_paths(dir, pid);
    let mut n = 0;
    for p in [jpg, sig, custom] {
        if std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    n
}

/// 清掉全部人物头像缓存（重建人物库用）；用户头像 `user_*.jpg` 保留
pub fn clear_all_avatar_cache(dir: &Path) -> u32 {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("avatar_") && std::fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// 缓存是否仍可信：自选头像恒真；自动头像要求签名与**当前**人脸集一致
fn cache_valid(dir: &Path, pid: &str, cur_sig: Option<&str>) -> bool {
    let (_jpg, sig, custom) = avatar_paths(dir, pid);
    if custom.is_file() {
        return true;
    }
    match (std::fs::read_to_string(&sig), cur_sig) {
        (Ok(written), Some(cur)) => written.trim() == cur,
        // 无签名（历史遗留）或当前无签名（人物已无脸）→ 一律不可信，重裁
        _ => false,
    }
}

/// 裁剪成功后落签名（供下次校验）；写失败不报错（下次会重裁一遍，属可接受代价）
fn write_sig(dir: &Path, pid: &str) {
    let (_jpg, sig, _custom) = avatar_paths(dir, pid);
    if let Ok(Some(s)) = crate::persons::face_sig(pid) {
        let _ = std::fs::write(sig, s);
    }
}

/// 头像刷新的前端事件载荷（批量后台重裁时逐个人物上报）
#[derive(Clone, serde::Serialize)]
pub struct AvatarRefreshed {
    pub pid: String,
    pub path: String,
}


pub mod commands {
use std::path::PathBuf;
use tauri::Manager;

// 头像缓存身份校验/清理助手（定义在本文件外层模块，见“头像缓存的身份纪律”段）
use super::{
    avatar_paths, cache_valid, pid_of_avatar_file, remove_avatar_cache, write_sig,
    AvatarRefreshed,
};

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
/// **缓存必须带身份校验**（BUG-2026-1001-001）：文件存在但签名与当前人脸集不一致
/// （历史遗留 / 人物库重建后 P 编号复用 / 合并过）→ 删旧重裁，避免封面显示别人。
#[tauri::command]
pub async fn get_person_avatar(
    pid: String,
    force_refresh: Option<bool>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let dir = avatars_dir(&app)?;
    let (cache_path, _sig, custom_path) = avatar_paths(&dir, &pid);
    // 缓存命中不写日志（BUG-2026-0910-002）：画廊一次拉几百个头像，此前每次命中都写
    // CALL+RET 两行，累计 15 万行噪音把 app.log 刷到 15MB+，日志副窗口也被洪水冲垮；
    // 只保留真正值得看的事件（现场裁剪/出错）。
    if !force_refresh.unwrap_or(false) && cache_path.is_file() {
        // 自选头像：用户指定优先，身份变化也不覆盖
        if custom_path.is_file() {
            return Ok(cache_path.to_string_lossy().into_owned());
        }
        let cur = crate::persons::face_sig(&pid)?;
        if cache_valid(&dir, &pid, cur.as_deref()) {
            return Ok(cache_path.to_string_lossy().into_owned());
        }
        // 签名失配：旧图不可信，删掉重裁（下面走正常裁剪路径）
        remove_avatar_cache(&dir, &pid);
    }
    let _t = log_call!("get_person_avatar", &format!("pid={pid} force={force_refresh:?}"));
    // 裁剪解码在阻塞线程执行，避免大图解码占用异步运行时
    let dir2 = dir.clone();
    let pid_work = pid.clone();
    let pid_sig = pid.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        crate::persons::crop_avatar_local(&pid_work, &cache_path)?;
        Ok(cache_path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| format!("头像任务线程失败: {e}"))?;
    if r.is_ok() {
        write_sig(&dir2, &pid_sig);
    }
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
///
/// BUG-2026-1001-001：命中还必须**身份签名一致**（见 avatar_paths 注释）。
/// 签名失配的旧头像不能返回（会显示别人），改由后台任务逐个重裁并 emit
/// `person-avatar-refreshed`，前端渐进替换 —— 既不拿错图糊弄，也不让一次画廊
/// 加载同步重裁上千张（那是分钟级卡顿）。孤儿缓存（人物已不存在）顺手清掉。
#[tauri::command]
pub async fn get_person_avatars_bulk(
    pids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<Vec<Option<String>>, String> {
    let _t = log_call!("get_person_avatars_bulk", &format!("count={}", pids.len()));
    let dir = avatars_dir(&app)?;
    let dir2 = dir.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        // 一次拿全量「人物 → 人脸集签名」，避免为每个 pid 单独查库
        let all = crate::persons::persons_with_sig().unwrap_or_default();
        let sigs: std::collections::HashMap<String, Option<String>> = all.into_iter().collect();
        // 孤儿清理：目录里存在但人物表里没有的 pid → 删除（P 编号复用的历史残渣）
        let mut orphan = 0u32;
        if let Ok(rd) = std::fs::read_dir(&dir2) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(pid) = pid_of_avatar_file(&name) {
                    if !sigs.contains_key(&pid) && std::fs::remove_file(e.path()).is_ok() {
                        orphan += 1;
                    }
                }
            }
        }
        let mut out: Vec<Option<String>> = Vec::with_capacity(pids.len());
        let mut stale: Vec<String> = Vec::new();
        for pid in &pids {
            let (jpg, _sig, custom) = avatar_paths(&dir2, pid);
            let cur = sigs.get(pid).and_then(|s| s.as_deref());
            let ok = jpg.is_file() && (custom.is_file() || cache_valid(&dir2, pid, cur));
            if ok {
                out.push(Some(jpg.to_string_lossy().into_owned()));
            } else {
                out.push(None);
                if sigs.contains_key(pid) {
                    // 只在人物确实存在（有脸可裁）时排队后台重裁
                    stale.push(pid.clone());
                }
            }
        }
        (out, stale, orphan)
    })
    .await
    .map_err(|e| format!("批量头像任务线程失败: {e}"))?;
    let (out, stale, orphan) = r;
    let hit = out.iter().filter(|x| x.is_some()).count();
    crate::logger::log_call_end_with(
        "get_person_avatars_bulk",
        _t,
        &format!(
            "OK | hit={hit} miss={} stale={} orphan={orphan}",
            out.len() - hit,
            stale.len()
        ),
    );
    // 后台重裁失配头像（不阻塞本次返回）
    if !stale.is_empty() {
        let dir3 = dir.clone();
        let app3 = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let t0 = std::time::Instant::now();
            let mut fixed = 0usize;
            for pid in &stale {
                let (jpg, _sig, _custom) = avatar_paths(&dir3, pid);
                remove_avatar_cache(&dir3, pid);
                if crate::persons::crop_avatar_local(pid, &jpg).is_ok() {
                    write_sig(&dir3, pid);
                    fixed += 1;
                    let _ = tauri::Emitter::emit(
                        &app3,
                        "person-avatar-refreshed",
                        AvatarRefreshed {
                            pid: pid.clone(),
                            path: jpg.to_string_lossy().into_owned(),
                        },
                    );
                }
            }
            crate::logger::log_info(&format!(
                "[avatar] 后台重裁完成：{fixed}/{} 张，{}ms",
                stale.len(),
                t0.elapsed().as_millis()
            ));
        });
    }
    Ok(out)
}


/// FEAT-047：人物自选头像 —— 用户在人物照片弹窗指定一张照片作为头像封面
///
/// 中心方裁 96×96 JPEG 覆盖 `avatars/avatar_{pid}.jpg`（get_person_avatar 的
/// is_file() 缓存命中自选结果，优先于自动裁剪）。解码在阻塞线程执行。
/// 同时落 `.custom` 标记：自选头像永不因身份签名失配被自动重裁覆盖（用户选择优先）。
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
    let dir = avatars_dir(&app)?;
    let (cache_path, sig_path, custom_path) = avatar_paths(&dir, &pid);
    let cache_str = cache_path.to_string_lossy().into_owned();
    let src = photo_path.clone();
    let dst = cache_path.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        crate::persons::set_avatar_from_photo(
            std::path::Path::new(&src),
            std::path::Path::new(&dst),
        )
    })
    .await
    .map_err(|e| format!("头像任务线程失败: {e}"))?;
    if r.is_ok() {
        // 标记为自选（内容存源图路径，便于排查“这个头像哪来的”）；旧签名作废
        let _ = std::fs::write(&custom_path, &photo_path);
        let _ = std::fs::remove_file(&sig_path);
    }
    match &r {
        Ok(_) => crate::logger::log_call_end_with("set_person_avatar_from_photo", _t, "OK | custom"),
        Err(e) => crate::logger::log_call_end_with("set_person_avatar_from_photo", _t, &format!("ERR | {e}")),
    }
    r?;
    Ok(cache_str)
}


/// 恢复自动头像：抹掉自选标记 + 按当前代表脸重裁（右键菜单用）
///
/// 与 set_person_avatar_from_photo 成对：用户挑错了/想回到“最像本人的脸”时，
/// 一步回到自动选脸结果。无脸人物返回错误（前端 toast 提示）。
#[tauri::command]
pub async fn restore_person_avatar(
    pid: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let _t = log_call!("restore_person_avatar", &format!("pid={pid}"));
    let dir = avatars_dir(&app)?;
    let (cache_path, _sig, custom_path) = avatar_paths(&dir, &pid);
    let _ = std::fs::remove_file(&custom_path);
    let _ = std::fs::remove_file(&cache_path);
    let dir2 = dir.clone();
    let pid2 = pid.clone();
    let dst = cache_path.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        crate::persons::crop_avatar_local(&pid2, &dst)
    })
    .await
    .map_err(|e| format!("头像任务线程失败: {e}"))?;
    if r.is_ok() {
        write_sig(&dir2, &pid);
    }
    match &r {
        Ok(_) => crate::logger::log_call_end_with("restore_person_avatar", _t, "OK | auto"),
        Err(e) => crate::logger::log_call_end_with("restore_person_avatar", _t, &format!("ERR | {e}")),
    }
    r?;
    Ok(cache_path.to_string_lossy().into_owned())
}

}

#[cfg(test)]
mod tests {
    use super::*;

    /// 头像缓存文件名 → pid 解析（孤儿清理/签名校验都靠它）
    #[test]
    fn test_pid_of_avatar_file() {
        assert_eq!(pid_of_avatar_file("avatar_P001.jpg").as_deref(), Some("P001"));
        assert_eq!(pid_of_avatar_file("avatar_P1484.sig").as_deref(), Some("P1484"));
        assert_eq!(pid_of_avatar_file("avatar_P1.custom").as_deref(), Some("P1"));
        assert_eq!(pid_of_avatar_file("user_1.jpg"), None);
        assert_eq!(pid_of_avatar_file("avatar_.jpg"), None);
    }

    /// 缓存可信判据：自选标记恒真；否则签名必须一致（不一致 = 可能是别人的脸）
    #[test]
    fn test_avatar_cache_validity() {
        let dir = std::env::temp_dir().join(format!("avatar_sig_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_jpg, sig, custom) = avatar_paths(&dir, "P007");

        // 无签名文件（历史遗留头像）→ 不可信 → 必须重裁
        assert!(!cache_valid(&dir, "P007", Some("3:9")));
        // 签名一致 → 可信
        std::fs::write(&sig, "3:9").unwrap();
        assert!(cache_valid(&dir, "P007", Some("3:9")));
        // 签名不一致（人脸集变了 / P 编号被复用）→ 不可信
        assert!(!cache_valid(&dir, "P007", Some("4:10")));
        // 当前无人脸签名 → 不可信（宁可不显示也不显示错的人）
        assert!(!cache_valid(&dir, "P007", None));
        // 自选标记 → 无条件可信（用户选择优先，身份变化也不覆盖）
        std::fs::write(&custom, "D:\\some\\photo.jpg").unwrap();
        assert!(cache_valid(&dir, "P007", Some("999:1")));

        remove_avatar_cache(&dir, "P007");
        assert!(!sig.exists() && !custom.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

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
