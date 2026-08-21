/// AOP 日志宏：在命令/函数入口记录调用开始，返回计时器
///
/// 用法（放在函数第一行）：
/// ```rust
/// let _t = log_call!("create_album", "input=...");
/// ```
macro_rules! log_call {
    ($name:expr) => {
        crate::logger::log_call_start($name, "")
    };
    ($name:expr, $desc:expr) => {
        crate::logger::log_call_start($name, $desc)
    };
}

mod auth;
mod content;
mod db;
mod folder;
mod geo_index;
mod logger;
mod photo_scan;
mod session;
mod test_scan;
mod thumbnail;
mod tone;
mod vision;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use db::{CreateAlbumInput, Database, UpdateAlbumInput};
use tauri::{Emitter, Manager};

/// 缩略图缓存目录名（位于 app_data_dir 下）
const THUMBS_DIR: &str = "thumbs";

/// 全局应用状态：封装数据库连接
///
/// `rusqlite::Connection` 本身非 `Sync`，需用 `Mutex` 包裹后才能满足
/// `tauri::State` 的 `Send + Sync` 要求，供多个 `#[tauri::command]` 共享。
/// 对应 SpringBoot 中被 `@Autowired` 注入的单例 `DataSource` / `Service`。
pub struct AppState(pub Mutex<Database>);

/// 登录会话状态：当前登录用户 id（None 表示未登录）
///
/// 多用户登录的核心状态：注册/登录成功后写入，登出后清空。
/// 所有相册/分组命令通过 `require_user` 读取它，实现相册空间按用户隔离。
pub struct SessionState(pub Mutex<Option<i64>>);

/// 读取当前登录用户 id，未登录返回「请先登录」错误
fn require_user(session: &tauri::State<SessionState>) -> Result<i64, String> {
    let guard = session.0.lock().map_err(|e| e.to_string())?;
    guard.ok_or_else(|| "请先登录".to_string())
}

// =====================================================================
// Tauri 命令层（对应 SpringBoot @RestController）
// =====================================================================
//


// 每个 `#[tauri::command]` 函数等价于一个 Controller 端点：
//   - `tauri::State<AppState>`  →  `@Autowired` 注入的 Service / Repository
//   - 入参（自动从 invoke 的 JS 对象反序列化）→  `@RequestBody`
//   - `Result<T, String>`  →  成功返回 JSON，失败返回错误字符串（前端 reject）
//
// 注册：在下方 `run()` 的 `invoke_handler!` 中列出全部命令名。
// 调用：前端 `invoke('create_album', { input: {...} })`。

// =====================================================================
// 认证命令（多用户登录）
// =====================================================================

/// 注册新用户（需求：账户名、邮箱、手机号、密码、密码确认）
///
/// 校验通过后写入 users 表（密码存 Argon2id 哈希），并自动登录。
#[tauri::command]
fn register(
    input: auth::RegisterInput,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<auth::User, String> {
    let _t = log_call!(
        "register",
        &format!("username={}", input.username)
    );
    let r = (|| -> Result<auth::User, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        let user = auth::register_user(db.conn(), input)?;
        // 注册成功自动登录 + 写入记住登录 token（默认 3 天免密复用）
        remember_login(&db, &app, user.id);
        let mut guard = session.0.lock().map_err(|e| e.to_string())?;
        *guard = Some(user.id);
        Ok(user)
    })();
    match &r {
        Ok(u) => logger::log_call_end_with("register", _t, &format!("OK | id={}", u.id)),
        Err(e) => logger::log_call_end_with("register", _t, &format!("ERR | {e}")),
    }
    r
}

/// 登录（需求：同一 app 多用户登录）
///
/// `account` 支持账户名 / 邮箱 / 手机号 任一 + 密码。
/// 成功后写入会话，后续相册/分组命令均以该用户空间为准。
#[tauri::command]
fn login(
    input: auth::LoginInput,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<auth::User, String> {
    let _t = log_call!("login", "account=***");
    let r = (|| -> Result<auth::User, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        let user = auth::verify_login(db.conn(), &input.account, &input.password)?;
        // 记住登录（默认 3 天免密复用上次用户）
        remember_login(&db, &app, user.id);
        let mut guard = session.0.lock().map_err(|e| e.to_string())?;
        *guard = Some(user.id);
        Ok(user)
    })();
    match &r {
        Ok(u) => logger::log_call_end_with("login", _t, &format!("OK | id={}", u.id)),
        Err(e) => logger::log_call_end_with("login", _t, &format!("ERR | {e}")),
    }
    r
}

/// 退出登录（清空会话）
#[tauri::command]
fn logout(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let _t = log_call!("logout");
    // 清除记住登录 token（DB + 磁盘文件），下次启动需重新登录
    if let Ok(dir) = app_data_dir(&app) {
        if let Some(token) = session::read_token_file(&dir) {
            if let Ok(db) = state.0.lock() {
                let _ = session::clear_remember_session(db.conn(), &token);
            }
        }
        session::clear_token_file(&dir);
    }
    let mut guard = session.0.lock().map_err(|e| e.to_string())?;
    *guard = None;
    logger::log_call_end_with("logout", _t, "OK");
    Ok(())
}

/// 获取当前登录用户（应用启动时恢复会话用），未登录返回 None
#[tauri::command]
fn get_current_user(
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<Option<auth::User>, String> {
    let user_id = {
        let guard = session.0.lock().map_err(|e| e.to_string())?;
        *guard
    };
    match user_id {
        None => Ok(None),
        Some(id) => {
            let db = state.0.lock().map_err(|e| e.to_string())?;
            auth::find_user_by_id(db.conn(), id).map_err(|e| e.to_string())
        }
    }
}

/// 忘记密码重置（需求：填手机号、账户名、邮箱校验通过后重设密码）
///
/// 无需登录即可调用；三者必须匹配同一用户。
#[tauri::command]
fn reset_password(
    input: auth::ResetPasswordInput,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let _t = log_call!("reset_password", "username=***");
    let r = (|| -> Result<(), String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        auth::reset_password(db.conn(), input)
    })();
    match &r {
        Ok(_) => logger::log_call_end_with("reset_password", _t, "OK"),
        Err(e) => logger::log_call_end_with("reset_password", _t, &format!("ERR | {e}")),
    }
    r
}

/// 输入参数长度校验
///
/// 对应 SpringBoot `@Valid` + Bean Validation，在命令层统一校验
fn validate_create(input: &CreateAlbumInput) -> Result<(), String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("请填写相册名称".into());
    }
    if name.chars().count() > 100 {
        return Err("相册名称不能超过 100 个字符".into());
    }
    if input.path.trim().is_empty() {
        return Err("请选择文件夹".into());
    }
    if let Some(desc) = &input.description {
        if desc.chars().count() > 500 {
            return Err("相册简介不能超过 500 个字符".into());
        }
    }
    Ok(())
}

/// 创建相册（需求 §4.2 create_album）
///
/// 多用户隔离：新相册归属当前登录用户。
#[tauri::command]
fn create_album(
    input: CreateAlbumInput,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<db::Album, String> {
    let _t = log_call!("create_album", &format!("name={}, path={}", input.name, input.path));
    let user_id = require_user(&session)?;
    validate_create(&input)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let r = db.create_album(input, user_id).map_err(|e| e.to_string());
    if let Ok(album) = &r {
        logger::log_call_end_with("create_album", _t, &format!("created id={}", album.id));
    } else {
        logger::log_call_end_with("create_album", _t, "FAILED");
    }
    r
}

/// 获取缩略图缓存目录（app_data_dir/thumbs）
fn thumbs_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {e}"))?;
    Ok(data_dir.join(THUMBS_DIR))
}

/// 应用数据目录（记住登录 token 文件所在）
fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {e}"))
}

/// 登录/注册成功后写入记住登录 token（默认 3 天免密复用上次用户）
///
/// 失败不阻断登录（仅记日志）：记住登录是体验增强，不影响本次会话。
fn remember_login(db: &db::Database, app: &tauri::AppHandle, user_id: i64) {
    if let Ok(dir) = app_data_dir(app) {
        if let Ok(token) = session::create_remember_session(db.conn(), user_id) {
            if let Err(e) = session::write_token_file(&dir, &token) {
                logger::log_error("session", &format!("写入记住登录 token 失败: {e}"));
            }
        }
    }
}

/// 填充相册的统计属性（照片数量、文件夹大小、拍摄时间、默认封面）
///
/// **变更探测 + SQL 复用**（替代 TTL 定时失效）：
/// 1. 读 album_stats 缓存的统计；
/// 2. 轻量统计当前目录递归文件数（`count_files_recursive`，只数不读）；
/// 3. 文件数与缓存一致 → 目录未变，直接用 SQL 里的统计（含 albums.cover_path
///    封面，持久化值，不重新生成）并返回；
/// 4. 不一致 → 仅此相册全量重扫（单次 walkdir 完成全部统计），封面用第一张图
///    生成缩略图并<b>写回 albums.cover_path</b>（SQL 持久化，下次加载直接读）。
///
/// 封面地址持久化到 SQL（albums.cover_path），每次加载直接调用，不依赖
/// cover_source 与缩略图生成链——修复：cover_source 为 NULL/源图缺失时
/// 命中路径封面丢失且永不恢复的 bug。更换封面（set_cover）同样更新 SQL
/// 并清理旧的封面缩略图文件。
///
/// - `photo_count`: 图片数量
/// - `size_bytes`: 文件夹真实占用空间
/// - `shoot_time`: 相册内图片的 EXIF 拍摄时间（YYYY-MM-DD）
/// - `cover_path`: 若没有封面，自动用文件夹内第一张图片的缩略图作为封面（写回 SQL）
fn fill_album_stats(album: &mut db::Album, thumbs_dir: &Path, state: &tauri::State<AppState>) {
    let dir = std::path::Path::new(&album.path);

    // 变更探测：递归文件总数（轻量，只数不读，每相册几 ms）
    let file_count = thumbnail::count_files_recursive(dir);

    // 1. 读取缓存的统计（锁内仅 SQLite 快查）
    let cached = {
        let db = state.0.lock().ok();
        db.and_then(|db| db.get_album_stats(album.id).ok().flatten())
    };
    if let Some(stats) = cached {
        // 2. 文件数一致 → 目录未变，直接用 SQL 里的统计与封面（cover_path 已持久化）
        if stats.file_count == file_count as i64 {
            album.photo_count = stats.photo_count;
            album.size_bytes = stats.size_bytes;
            album.shoot_time = stats.shoot_time.clone();
            // 封面兜底：photo_count>0 但 albums.cover_path 为空（旧库未持久化自动封面
            // /封面异常丢失）→ 落入重扫路径，生成封面并写回 SQL，一次性自愈。
            // 相册无图（photo_count==0）则正常返回（无封面是正确状态）。
            if album.cover_path.is_none() && stats.photo_count > 0 {
                // fall through 到全量重扫
            } else {
                return;
            }
        }
        // 3. 文件数不一致 → 仅此相册全量重扫（fall through）
    }

    // 4. 全量扫描（单次 walkdir 完成全部统计）
    let scan = thumbnail::scan_album_dir(dir);
    album.photo_count = scan.photo_count as i64;
    album.size_bytes = scan.size_bytes;

    // 无封面时自动用第一张图片的缩略图作为封面，并持久化到 SQL（albums.cover_path）
    if album.cover_path.is_none() {
        if let Some(src) = &scan.first_image {
            if let Ok(res) = thumbnail::ensure_thumbnail_from_source(album.id, src, thumbs_dir) {
                album.cover_path = Some(res.thumb_path.clone());
                if let Ok(db) = state.0.lock() {
                    let _ = db.update_album_cover(album.id, album.cover_path.clone());
                }
            }
        }
    }

    // 拍摄时间：从第一张原图读取 EXIF（缩略图是生成的，不含 EXIF）
    album.shoot_time = scan
        .first_image
        .as_ref()
        .and_then(|p| thumbnail::read_shoot_time(p));

    // 3. 写回缓存（记录当前文件数作为下次变更探测信号）
    if let Ok(db) = state.0.lock() {
        let _ = db.upsert_album_stats(
            album.id,
            album.photo_count,
            album.size_bytes,
            album.shoot_time.clone(),
            scan.first_image.map(|p| p.to_string_lossy().into_owned()),
            file_count as i64,
        );
    }
}

/// 获取相册列表（需求 §4.2 get_albums，按 updated_at 降序）
///
/// 返回时为无封面的相册自动补第一张图缩略图。
/// 多用户隔离：仅返回当前登录用户的相册。
#[tauri::command]
fn get_albums(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<Vec<db::Album>, String> {
    let _t = log_call!("get_albums");
    let user_id = require_user(&session)?;
    let thumbs = thumbs_dir(&app)?;
        let mut albums = {
            let db = state.0.lock().map_err(|e| e.to_string())?;
            db.get_albums(user_id).map_err(|e| e.to_string())?
        };
    for a in albums.iter_mut() {
        fill_album_stats(a, &thumbs, &state);
    }
    logger::log_call_end_with("get_albums", _t, &format!("OK | count={}", albums.len()));
    Ok(albums)
}

/// 获取单个相册详情（需求 §4.2 get_album）
///
/// 多用户隔离：仅能获取归属当前用户的相册。
#[tauri::command]
fn get_album(
    id: i64,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<db::Album, String> {
    let user_id = require_user(&session)?;
    let thumbs = thumbs_dir(&app)?;
    let mut album = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(id, user_id).map_err(|e| e.to_string())?
    };
    fill_album_stats(&mut album, &thumbs, &state);
    Ok(album)
}

/// 更新相册信息（需求 §4.2 update_album）
///
/// 多用户隔离：仅能更新归属当前用户的相册。
#[tauri::command]
fn update_album(
    input: UpdateAlbumInput,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let user_id = require_user(&session)?;
    if let Some(name) = &input.name {
        let name = name.trim();
        if name.is_empty() {
            return Err("相册名称不能为空".into());
        }
        if name.chars().count() > 100 {
            return Err("相册名称不能超过 100 个字符".into());
        }
    }
    if let Some(desc) = &input.description {
        if desc.chars().count() > 500 {
            return Err("相册简介不能超过 500 个字符".into());
        }
    }
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.update_album(input, user_id).map_err(|e| e.to_string())
}

/// 地点自动识别（FEAT-004 自动化）：扫描相册照片 GPS → 反向地理编码 → 落库
///
/// - `force=false`：相册已有手动地点标签时保留（不覆盖），返回 changed=false
/// - 无 GPS 照片 / 反编码失败 / 相册不存在 → 明确错误
/// - 仅写 location，不刷新 updated_at（避免打乱列表排序）
/// - async + spawn_blocking：含网络请求，同步命令会阻塞主线程卡死 UI
#[derive(serde::Serialize)]
struct LocationDetectResult {
    location: String,
    changed: bool,
    lat: f64,
    lon: f64,
}

#[tauri::command]
async fn auto_detect_album_location(
    album_id: i64,
    force: bool,
    state: tauri::State<'_, AppState>,
    session: tauri::State<'_, SessionState>,
) -> Result<LocationDetectResult, String> {
    let _t = log_call!("auto_detect_album_location", &format!("album_id={album_id} force={force}"));
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let album = db.get_album(album_id, user_id).map_err(|e| e.to_string())?;
    if album.location.is_some() && !force {
        let msg = format!("相册已有地点标签（{}），跳过自动识别；如需覆盖请用 force",
            album.location.as_deref().unwrap_or(""));
        logger::log_call_end_with("auto_detect_album_location", _t, &format!("SKIP | {msg}"));
        return Err(msg);
    }
    let dir = album.path;
    drop(db); // 网络/文件 IO 期间不持有数据库锁
    // 1) GPS 众数坐标（~1km 网格）
    let coord = photo_scan::detect_album_location(&dir)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "相册内照片无 GPS 坐标，无法自动识别地点".to_string())?;
    // 2) 反向地理编码：本地省/市优先（离线秒回），未命中再联网精确查询
    let place = match geo_index::find_region(coord.0, coord.1) {
        Some(p) => p,
        None => photo_scan::reverse_geocode_coord(coord.0, coord.1)
            .ok_or_else(|| "地名解析失败（本地未命中且联网查询失败）".to_string())?,
    };
    // 3) 落库（不动 updated_at）
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.update_album_location(album_id, user_id, &place).map_err(|e| e.to_string())?;
    drop(db);
    logger::log_call_end_with(
        "auto_detect_album_location",
        _t,
        &format!("OK | {place} @ {:.4},{:.4}", coord.0, coord.1),
    );
    Ok(LocationDetectResult { location: place, changed: true, lat: coord.0, lon: coord.1 })
}

/// 重命名相册（可同时重命名绑定的本地文件夹）
///
/// - 先重命名本地文件夹（`rename_folder=true` 时），成功后才更新数据库，
///   失败则报错且数据库不变（保持名称与文件夹一致）
/// - 文件夹不存在/目标已存在/无权限 → 返回明确错误
/// - 文件夹路径变化后清除统计缓存（cover_source 指向旧路径），下次加载重扫
#[tauri::command]
fn rename_album(
    id: i64,
    new_name: String,
    rename_folder: bool,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<db::Album, String> {
    let _t = log_call!(
        "rename_album",
        &format!("id={id}, new_name={new_name}, rename_folder={rename_folder}")
    );
    let user_id = require_user(&session)?;
    let new_name = new_name.trim().to_string();
    if new_name.is_empty() {
        return Err("相册名称不能为空".into());
    }
    if new_name.chars().count() > 100 {
        return Err("相册名称不能超过 100 个字符".into());
    }
    // 读取当前相册
    let current = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(id, user_id).map_err(|e| e.to_string())?
    };
    let mut final_path = current.path.clone();
    // 同步重命名本地文件夹
    if rename_folder {
        let old_path = std::path::Path::new(&current.path);
        let old_name = old_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .ok_or_else(|| "无法解析相册文件夹路径".to_string())?;
        if new_name != old_name {
            let parent = old_path
                .parent()
                .ok_or_else(|| "无法解析相册文件夹上级目录".to_string())?;
            let target = parent.join(&new_name);
            if target.exists() {
                logger::log_call_end_with(
                    "rename_album",
                    _t,
                    &format!("FAILED | 目标文件夹已存在: {}", target.display()),
                );
                return Err(format!("目标文件夹已存在: {}", target.display()));
            }
            std::fs::rename(old_path, &target)
                .map_err(|e| format!("重命名文件夹失败: {e}"))?;
            final_path = target.to_string_lossy().into_owned();
        }
    }
    // 更新数据库（名称 + 路径）
    {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.update_album_name_path(id, user_id, &new_name, &final_path)
            .map_err(|e| e.to_string())?;
        // 路径变化 → 统计缓存失效（cover_source 指向旧路径），下次访问重扫
        if final_path != current.path {
            let _ = db.delete_album_stats(id);
        }
    }
    // 返回更新后的相册
    let album = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(id, user_id).map_err(|e| e.to_string())?
    };
    logger::log_call_end_with(
        "rename_album",
        _t,
        &format!("OK | id={id}, path={final_path}"),
    );
    Ok(album)
}

/// 设置相册标签（覆盖式，最多 5 个）
///
/// 多用户隔离：仅能操作归属当前登录用户的相册。
#[tauri::command]
fn update_album_tags(
    album_id: i64,
    tags: Vec<String>,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.update_album_tags(album_id, user_id, tags).map_err(|e| e.to_string())
}

/// 删除相册（需求 §4.2 delete_album，仅删记录不删本地文件）
///
/// 删除成功后同时清理该相册的缩略图缓存文件（数据库级联删除见 db::delete_album）。
/// 多用户隔离：仅能删除归属当前登录用户的相册。
#[tauri::command]
fn delete_album(
    id: i64,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let _t = log_call!("delete_album", &format!("id={id}"));
    let user_id = require_user(&session)?;
    let r = (|| -> Result<(), String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.delete_album(id, user_id).map_err(|e| e.to_string())
    })();
    if r.is_ok() {
        // 记录已删除，清理缓存文件（失败不影响删除结果）
        if let Ok(thumbs) = thumbs_dir(&app) {
            thumbnail::cleanup_all_album_thumbs(id, &thumbs);
        }
    }
    match &r {
        Ok(_) => logger::log_call_end_with("delete_album", _t, "OK"),
        Err(e) => logger::log_call_end_with("delete_album", _t, &format!("ERR | {e}")),
    }
    r
}

/// 批量删除相册（勾选删除）
///
/// 接收相册 ID 数组，事务内批量删除记录。
/// **仅删除数据库记录，不删除本地照片文件。**
/// 返回实际删除数量。删除成功后清理对应缩略图缓存。
/// 多用户隔离：仅能删除归属当前登录用户的相册。
#[tauri::command]
fn delete_albums(
    ids: Vec<i64>,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<usize, String> {
    let _t = log_call!("delete_albums", &format!("ids={ids:?}"));
    let user_id = require_user(&session)?;
    let r = (|| -> Result<usize, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.delete_albums(&ids, user_id).map_err(|e| e.to_string())
    })();
    if let Ok(n) = &r {
        if *n > 0 {
            if let Ok(thumbs) = thumbs_dir(&app) {
                for id in &ids {
                    thumbnail::cleanup_all_album_thumbs(*id, &thumbs);
                }
            }
        }
    }
    match &r {
        Ok(n) => logger::log_call_end_with("delete_albums", _t, &format!("OK | deleted={n}")),
        Err(e) => logger::log_call_end_with("delete_albums", _t, &format!("ERR | {e}")),
    }
    r
}

/// 扫描相册目录内所有图片的 EXIF 拍摄参数（测试功能，不落库）
///
/// 信息提取逻辑全部在独立模块 `photo_scan` 中，此处仅保留薄命令壳
/// （功能解耦：photo_scan 不依赖数据库，可独立测试）。
#[tauri::command]
fn scan_album_photos(path: String) -> Result<Vec<photo_scan::PhotoExif>, String> {
    let _t = log_call!("scan_album_photos", &format!("path={path}"));
    let r = photo_scan::scan_album_photos(&path);
    match &r {
        Ok(list) => logger::log_call_end_with(
            "scan_album_photos",
            _t,
            &format!("OK | photos={}", list.len()),
        ),
        Err(e) => logger::log_call_end_with("scan_album_photos", _t, &format!("ERR | {e}")),
    }
    r
}

/// 扫描 EXIF + 本地行政区划反查（离线 · 省/市，无网络请求）
///
/// 内嵌民政部口径边界数据（resources/china_geo.json），bbox 预筛 + 射线法点面判断，
/// 万张照片 <1s；未命中（国外/公海）时 place 为 None，由前端显示坐标链接。
/// async：保持与 with_place 同构（内部实际为纯 CPU 计算，spawn_blocking 由命令层承担）。
#[tauri::command]
async fn scan_album_photos_local_place(path: String) -> Result<Vec<photo_scan::PhotoExif>, String> {
    let _t = log_call!("scan_album_photos_local_place", &format!("path={path}"));
    let r = photo_scan::scan_album_photos_with_place_local(&path);
    match &r {
        Ok(list) => logger::log_call_end_with(
            "scan_album_photos_local_place",
            _t,
            &format!("OK | photos={} place={}", list.len(), list.iter().filter(|p| p.place.is_some()).count()),
        ),
        Err(e) => logger::log_call_end_with("scan_album_photos_local_place", _t, &format!("ERR | {e}")),
    }
    r
}

/// 扫描 EXIF + 反向地理编码（联网，BigDataCloud 中文地名）
///
/// 仅对有 GPS 坐标的照片发起请求；扫描速度受网络影响（~200ms/张）。
/// async：含网络请求，同步命令会阻塞主线程。
#[tauri::command]
async fn scan_album_photos_with_place(path: String) -> Result<Vec<photo_scan::PhotoExif>, String> {
    let _t = log_call!("scan_album_photos_with_place", &format!("path={path}"));
    let r = photo_scan::scan_album_photos_with_place(&path);
    match &r {
        Ok(list) => logger::log_call_end_with(
            "scan_album_photos_with_place",
            _t,
            &format!("OK | photos={} place={}", list.len(), list.iter().filter(|p| p.place.is_some()).count()),
        ),
        Err(e) => logger::log_call_end_with("scan_album_photos_with_place", _t, &format!("ERR | {e}")),
    }
    r
}

// ---------------------------------------------------------------------------
// 主页面「图片扫描测试」命令（不落库，仅验证时间/地点识别 + 照片移动）
// ---------------------------------------------------------------------------

/// 扫描相册目录内所有图片的影调分析（灰度直方图 + 影调类型，测试功能，不落库）
/// 影调提取逻辑全部在独立模块 `tone` 中，此处仅保留薄命令壳（功能解耦）。
#[tauri::command]
fn scan_album_tones(path: String) -> Result<Vec<tone::PhotoTone>, String> {
    let _t = log_call!("scan_album_tones", &format!("path={path}"));
    let r = tone::scan_album_tones(&path);
    match &r {
        Ok(list) => logger::log_call_end_with(
            "scan_album_tones",
            _t,
            &format!("OK | photos={}", list.len()),
        ),
        Err(e) => logger::log_call_end_with("scan_album_tones", _t, &format!("ERR | {e}")),
    }
    r
}

/// 视觉内容识别（YOLOv8n-cls，测试功能，不落库）
///
/// 启动/复用独立 Python 微服务，批量识别相册目录内图片的内容，
/// 通过 `classify-progress` 事件实时上报进度。识别逻辑全部在
/// 独立模块 `vision` 中，此处仅保留薄命令壳（功能解耦）。
#[tauri::command]
async fn classify_album(
    path: String,
    batch_size: Option<i64>,
    app: tauri::AppHandle,
) -> Result<Vec<vision::VisionResult>, String> {
    let _t = log_call!("classify_album", &format!("path={path}"));
    let r = vision::classify_album(&path, batch_size.unwrap_or(8).max(1) as usize, &app).await;
    match &r {
        Ok(list) => logger::log_call_end_with(
            "classify_album",
            _t,
            &format!("OK | photos={}", list.len()),
        ),
        Err(e) => logger::log_call_end_with("classify_album", _t, &format!("ERR | {e}")),
    }
    r
}

/// 人物注册表：列出全部已标号人物
#[tauri::command]
async fn list_persons() -> Result<Vec<vision::PersonInfo>, String> {
    vision::list_persons().await
}

/// 人物注册表：重命名人物
#[tauri::command]
async fn rename_person(pid: String, name: String) -> Result<(), String> {
    vision::rename_person(&pid, &name).await
}

/// 人物注册表：合并人物（source 并入 target）
#[tauri::command]
async fn merge_persons(target: String, source: String) -> Result<(), String> {
    vision::merge_persons(&target, &source).await
}

/// 人物注册表：删除人物
#[tauri::command]
async fn delete_person(pid: String) -> Result<(), String> {
    vision::delete_person(&pid).await
}

/// GPU 加速可行性（R3）：确保服务就绪后查询 /gpu，返回是否可用 GPU
#[tauri::command]
async fn get_vcr_gpu_status() -> Result<vision::VcrGpuStatus, String> {
    vision::vcr_gpu_status().await
}

/// 在系统文件管理器中打开文件夹内部
///
/// 使用系统原生命令，比 opener 插件的 `open_path` 在 Windows 上更可靠：
/// - Windows: `explorer <path>` 直接进入目录内容
/// - macOS: `open <path>`
/// - Linux: `xdg-open <path>`
#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    // 校验路径存在且是目录
    let p = std::path::Path::new(&path);
    if !p.is_dir() {
        return Err(format!("路径不存在或不是文件夹: {path}"));
    }

    // 根据平台选择系统打开命令
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(&path); // explorer <路径> 直接进入目录内容
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(&path);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(&path);
        c
    };

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开文件夹: {e}"))
}

/// 设置相册封面（需求 §2.3 设置封面 / §6.2 图片选择对话框）
///
/// 接收用户选择的图片路径，生成封面缩略图缓存到 thumbs/，
/// 并更新 Album.cover_path。返回更新后的相册。
#[tauri::command]
fn set_cover(
    id: i64,
    image_path: String,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<db::Album, String> {
    let _t = log_call!("set_cover", &format!("album_id={id}, image_path={image_path}"));
    logger::log_info(&format!("[SET_COVER] 开始更换封面: album_id={id}, 选择图片={image_path}"));
    let user_id = require_user(&session)?;

    // 阶段1：校验图片路径存在（用户选择封面）
    let t1 = std::time::Instant::now();
    let img = std::path::Path::new(&image_path);
    if !img.is_file() {
        let e = format!("图片不存在: {image_path}");
        logger::log_error("set_cover", &e);
        logger::log_call_end_with("set_cover", _t, "FAILED | 图片不存在");
        return Err(e);
    }
    logger::log_info(&format!(
        "[SET_COVER] 阶段1 图片校验通过: {}ms",
        t1.elapsed().as_millis()
    ));

    // 阶段2：生成封面缩略图（统一存到缓存目录）
    let thumbs = thumbs_dir(&app)?;
    let t2 = std::time::Instant::now();
    let cover = match thumbnail::generate_cover(id, img, &thumbs) {
        Ok(c) => {
            logger::log_info(&format!(
                "[SET_COVER] 阶段2 生成封面缩略图: {}ms → {}",
                t2.elapsed().as_millis(),
                c
            ));
            c
        }
        Err(e) => {
            let e = format!("无法生成封面缩略图: {e}");
            logger::log_error("set_cover", &e);
            logger::log_call_end_with("set_cover", _t, "FAILED | 生成缩略图失败");
            return Err(e);
        }
    };

    // 阶段3：更新数据库 cover_path（更换封面）
    let t3 = std::time::Instant::now();
    {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.update_album(
            db::UpdateAlbumInput {
                id,
                name: None,
                description: None,
                cover_path: Some(cover),
                location: None,
            },
            user_id,
        )
        .map_err(|e| e.to_string())?;
    }
    logger::log_info(&format!(
        "[SET_COVER] 阶段3 更新数据库 cover_path: {}ms",
        t3.elapsed().as_millis()
    ));

    // 阶段4：读取更新后的相册 + 清理残留自动缩略图 + 刷新统计
    let t4 = std::time::Instant::now();
    let mut album = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(id, user_id).map_err(|e| e.to_string())?
    };
    // 手动封面已确定，清理可能残留的自动缩略图缓存，避免孤儿文件
    thumbnail::cleanup_album_auto_thumbs(id, &thumbs);
    // 此时 cover_path 已设置，fill_album_stats 不会覆盖它
    fill_album_stats(&mut album, &thumbs, &state);
    logger::log_info(&format!(
        "[SET_COVER] 阶段4 清理旧缩略图+刷新统计: {}ms",
        t4.elapsed().as_millis()
    ));

    logger::log_call_end_with("set_cover", _t, &format!("OK | album_id={id}"));
    Ok(album)
}

/// 批量导入结果
#[derive(Debug, serde::Serialize)]
pub struct ImportResult {
    /// 成功导入的相册数量
    pub imported: usize,
    /// 因已存在而跳过的相册数量
    pub skipped: usize,
    /// 创建失败的文件夹及原因
    pub errors: Vec<String>,
}

/// 批量导入进度事件载荷
#[derive(Debug, Clone, serde::Serialize)]
struct ImportProgress {
    /// 当前处理到第几个
    pub current: usize,
    /// 子文件夹总数
    pub total: usize,
    /// 已成功导入数
    pub imported: usize,
    /// 当前处理的文件夹名
    pub current_name: String,
}

/// 批量导入相册
///
/// 选择一个大文件夹，遍历其**直接子文件夹**，
/// 每个子文件夹默认作为独立相册（相册名 = 子文件夹名）。
/// 已作为相册存在的文件夹自动跳过，避免重复。
/// 处理过程中通过 `import-progress` 事件实时上报进度，供前端进度条显示。
/// 多用户隔离：导入的相册归属当前登录用户。
#[tauri::command]
fn import_albums(
    root_path: String,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<ImportResult, String> {
    let user_id = require_user(&session)?;
    let root = std::path::Path::new(&root_path);
    if !root.is_dir() {
        return Err(format!("路径不存在或不是文件夹: {root_path}"));
    }

    let mut result = ImportResult {
        imported: 0,
        skipped: 0,
        errors: Vec::new(),
    };

    let db = state.0.lock().map_err(|e| e.to_string())?;

    // 收集所有子文件夹（先统计总数，用于进度计算）
    let folders: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(root)
        .map_err(|e| format!("读取文件夹失败: {e}"))?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name, entry.path())
        })
        .collect();

    let total = folders.len();
    let mut processed = 0usize;

    for (folder_name, path) in folders {
        let path_str = path.to_string_lossy().into_owned();

        // 检查是否已作为相册存在（当前用户空间内），存在则跳过
        if let Ok(Some(_)) = db.find_album_by_path(&path_str, user_id) {
            result.skipped += 1;
        } else {
            // 创建相册，名称用子文件夹名（归属当前用户）
            let created = db.create_album(
                db::CreateAlbumInput {
                    name: folder_name.clone(),
                    path: path_str,
                    description: None,
                },
                user_id,
            );
            match created {
                Ok(_) => result.imported += 1,
                Err(e) => result.errors.push(format!("{folder_name}: {e}")),
            }
        }

        processed += 1;
        // 上报进度事件
        let _ = app.emit(
            "import-progress",
            ImportProgress {
                current: processed,
                total,
                imported: result.imported,
                current_name: folder_name,
            },
        );
    }

    Ok(result)
}

// =====================================================================
// 手动排序分组命令（文件夹）
// =====================================================================

/// 创建分组（文件夹）
///
/// 多用户隔离：分组归属当前登录用户。
#[tauri::command]
fn create_folder(
    name: String,
    parent_id: Option<i64>,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<folder::Folder, String> {
    let user_id = require_user(&session)?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("分组名称不能为空".into());
    }
    let db = state.0.lock().map_err(|e| e.to_string())?;
    folder::create_folder(db.conn(), user_id, &name, parent_id).map_err(|e| e.to_string())
}

/// 更新分组（名称/说明/标签，标签最多 5 个）
///
/// 多用户隔离：仅能操作归属当前登录用户的分组。
#[tauri::command]
fn update_folder(
    id: i64,
    name: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<folder::Folder, String> {
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    folder::update_folder(
        db.conn(),
        user_id,
        id,
        name.as_deref(),
        description.as_deref(),
        tags,
    )
    .map_err(|e| e.to_string())
}

/// 删除分组
///
/// 多用户隔离：仅能删除归属当前登录用户的分组。
#[tauri::command]
fn delete_folder(
    id: i64,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    folder::delete_folder(db.conn(), user_id, id).map_err(|e| e.to_string())
}

/// 获取手动排序结构
///
/// 多用户隔离：仅返回当前登录用户的分组与相册。
#[tauri::command]
fn get_manual_tree(
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<folder::ManualTree, String> {
    let _t = log_call!("get_manual_tree");
    let user_id = require_user(&session)?;
    let r = (|| -> Result<folder::ManualTree, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        folder::get_manual_tree(db.conn(), user_id).map_err(|e| e.to_string())
    })();
    match &r {
        Ok(t) => logger::log_call_end_with("get_manual_tree", _t, &format!("OK | folders={}", t.folders.len())),
        Err(e) => logger::log_call_end_with("get_manual_tree", _t, &format!("ERR | {e}")),
    }
    r
}

/// 按名称模糊搜索相册（含分组归属路径）
///
/// 多用户隔离：仅搜索当前登录用户的相册空间。
#[tauri::command]
fn search_albums(
    keyword: String,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<Vec<db::AlbumSearchResult>, String> {
    let _t = log_call!("search_albums", &format!("keyword={keyword}"));
    let user_id = require_user(&session)?;
    let r = (|| -> Result<Vec<db::AlbumSearchResult>, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.search_albums(&keyword, user_id).map_err(|e| e.to_string())
    })();
    match &r {
        Ok(list) => logger::log_call_end_with("search_albums", _t, &format!("OK | count={}", list.len())),
        Err(e) => logger::log_call_end_with("search_albums", _t, &format!("ERR | {e}")),
    }
    r
}

/// 移动相册到分组或调整顺序
///
/// - `folder_id` 为 None 表示移到顶级
/// - 相册移到目标分组后排在组内末尾
/// - 多用户隔离：仅能移动归属当前登录用户的相册到自己的分组
#[tauri::command]
fn move_album(
    album_id: i64,
    folder_id: Option<i64>,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let _start = logger::log_call_start("move_album", &format!("album_id={album_id}, folder_id={folder_id:?}"));
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let conn = db.conn();

    // 校验相册归属当前用户（他人相册等同不存在）
    let album_owned: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM albums WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![album_id, user_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !album_owned {
        return Err("相册不存在".into());
    }

    // 校验文件夹存在（归属当前用户）
    if let Some(fid) = folder_id {
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM folders WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![fid, user_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err("目标分组不存在".into());
        }
    }

    // 新位置排序：组内末尾（唯一事实源 folder_albums；移出到顶级时无排序 UI，归 0）
    let sort_order: i64 = match folder_id {
        Some(fid) => conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM folder_albums WHERE folder_id = ?1",
                rusqlite::params![fid],
                |r| r.get(0),
            )
            .unwrap_or(0),
        None => 0,
    };

    // 事务内更新，确保 folder_id 持久化（albums.folder_id/sort_order 为冗余缓存列，同事务同步）
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    // 1. 冗余列：albums.folder_id（读取不依赖它，仅保持数据完整）
    tx.execute(
        "UPDATE albums SET folder_id = ?1, sort_order = ?2 WHERE id = ?3 AND user_id = ?4",
        rusqlite::params![folder_id, sort_order, album_id, user_id],
    )
    .map_err(|e| e.to_string())?;
    // 2. 事实源：folder_albums 关联表（先删旧关联，再插入新关联）
    tx.execute(
        "DELETE FROM folder_albums WHERE album_id = ?1",
        rusqlite::params![album_id],
    )
    .map_err(|e| e.to_string())?;
    if let Some(fid) = folder_id {
        tx.execute(
            "INSERT OR REPLACE INTO folder_albums (folder_id, album_id, sort_order) VALUES (?1, ?2, ?3)",
            rusqlite::params![fid, album_id, sort_order],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    logger::log_call_end_with("move_album", _start, &format!("album_id={album_id}, folder_id={folder_id:?}"));
    Ok(())
}

/// 相册组内排序
///
/// 将相册移到同一分组内的新位置。`new_index` 为该相册在目标分组（同一分组）中的新下标。
/// 若 `folder_id` 提供且与相册当前分组不同，则先移动到该分组再插入指定位置。
/// 多用户隔离：仅能排序归属当前登录用户的相册与分组。
#[tauri::command]
fn reorder_album(
    album_id: i64,
    folder_id: Option<i64>,
    new_index: i64,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let conn = db.conn();

    // 校验相册归属当前用户（他人相册等同不存在）
    let album_owned: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM albums WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![album_id, user_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !album_owned {
        return Err("相册不存在".into());
    }

    // 校验文件夹存在（归属当前用户）
    if let Some(fid) = folder_id {
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM folders WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![fid, user_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err("目标分组不存在".into());
        }
    }

    // 事务：先移到目标分组，再在组内排序
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;

    // 移到目标分组（先放末尾）
    tx.execute(
        "UPDATE albums SET folder_id = ?1, sort_order = 999999 WHERE id = ?2 AND user_id = ?3",
        rusqlite::params![folder_id, album_id, user_id],
    )
    .map_err(|e| e.to_string())?;

    // 更新 folder_albums 关联表（删除旧关联）
    tx.execute(
        "DELETE FROM folder_albums WHERE album_id = ?1",
        rusqlite::params![album_id],
    )
    .map_err(|e| e.to_string())?;
    // 若移入分组，插入临时关联（末尾）
    if let Some(fid) = folder_id {
        tx.execute(
            "INSERT OR REPLACE INTO folder_albums (folder_id, album_id, sort_order) VALUES (?1, ?2, 999999)",
            rusqlite::params![fid, album_id],
        )
        .map_err(|e| e.to_string())?;
    }

    // 目标分组所有相册按当前顺序取出（唯一事实源 folder_albums）
    let mut items: Vec<i64> = {
        let mut stmt = tx
            .prepare(
                "SELECT album_id FROM folder_albums WHERE folder_id = ?1 ORDER BY sort_order, id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![folder_id], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };

    // 移除相册自身，插入到 new_index
    items.retain(|&id| id != album_id);
    let new_index = new_index.max(0).min(items.len() as i64);
    items.insert(new_index as usize, album_id);

    // 重写 folder_albums.sort_order（事实源）+ albums.sort_order（冗余缓存列）
    for (i, &id) in items.iter().enumerate() {
        tx.execute(
            "UPDATE albums SET sort_order = ?1 WHERE id = ?2",
            rusqlite::params![i as i64, id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE folder_albums SET sort_order = ?1 WHERE album_id = ?2 AND folder_id = ?3",
            rusqlite::params![i as i64, id, folder_id],
        )
        .map_err(|e| e.to_string())?;
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// 调整分组在兄弟中的顺序
///
/// 多用户隔离：仅能调整归属当前登录用户的分组顺序。
#[tauri::command]
fn reorder_folder(
    folder_id: i64,
    new_index: i64,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let conn = db.conn();
    let folder = folder::get_folder(conn, user_id, folder_id).map_err(|e| e.to_string())?;
    // 兄弟分组排序（仅当前用户的分组）
    let mut siblings: Vec<(i64, i64)> = {
        let mut stmt = conn
            .prepare("SELECT id, sort_order FROM folders WHERE parent_id IS ?1 AND user_id = ?2 ORDER BY sort_order")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![folder.parent_id, user_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };
    // 找到该文件夹位置并移除
    if let Some(idx) = siblings.iter().position(|(id, _)| *id == folder_id) {
        siblings.remove(idx);
    }
    let new_index = new_index.max(0).min(siblings.len() as i64);
    siblings.insert(new_index as usize, (folder_id, 0));
    for (i, (id, _)) in siblings.iter().enumerate() {
        conn.execute(
            "UPDATE folders SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![i as i64, now(), id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 当前时间戳（reorder_folder 内部用）
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// =====================================================================
// 应用启动
// =====================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 数据库文件存放于 Tauri 的 app_data_dir
            // Windows: %APPDATA%/com.haoyuan.photo-management-sys/photos.db
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("无法获取应用数据目录");
            // 初始化日志组件（保留 60 分钟，可调节）
            logger::init(&data_dir, 60);
            let db_path = data_dir.join("photos.db");
            logger::log_info("数据库初始化中...");
            let database = Database::open(&db_path).expect("数据库初始化失败");
            logger::log_info("数据库初始化完成");
            // 记住登录表（R2）：登录时写入 3 天 token，启动时恢复免密登录
            if let Err(e) = session::init_schema(database.conn()) {
                logger::log_error("session", &format!("记住登录表初始化失败: {e}"));
            }
            // 恢复上次登录（默认 3 天免密复用上次用户）；失败则清 token
            let restored_user = session::read_token_file(&data_dir)
                .and_then(|token| {
                    session::validate_remember_session(database.conn(), &token)
                        .map_err(|e| {
                            logger::log_error("session", &format!("记住登录校验失败: {e}"));
                        })
                        .ok()
                        .flatten()
                });
            match restored_user {
                Some(uid) => {
                    logger::log_info(&format!("已恢复上次登录用户 id={uid}（记住登录 3 天）"));
                    app.manage(AppState(Mutex::new(database)));
                    app.manage(SessionState(Mutex::new(Some(uid))));
                }
                None => {
                    // token 缺失/失效 → 清理磁盘文件，按未登录启动
                    session::clear_token_file(&data_dir);
                    app.manage(AppState(Mutex::new(database)));
                    app.manage(SessionState(Mutex::new(None)));
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 认证（多用户注册/登录/登出/忘记密码）
            register,
            login,
            logout,
            get_current_user,
            reset_password,
            // 相册管理（按用户隔离）
            create_album,
            get_albums,
            get_album,
            update_album,
            auto_detect_album_location,
            rename_album,
            update_album_tags,
            delete_album,
            delete_albums,
            open_folder,
            set_cover,
            import_albums,
            create_folder,
            update_folder,
            delete_folder,
            get_manual_tree,
            move_album,
            reorder_album,
            reorder_folder,
            search_albums,
            scan_album_photos,
            scan_album_photos_with_place,
            scan_album_photos_local_place,
            test_scan::commands::scan_test_photos,
            test_scan::commands::resolve_test_places,
            test_scan::commands::organize_test_photos,
            scan_album_tones,
            classify_album,
            list_persons,
            rename_person,
            merge_persons,
            delete_person,
            content::commands::scan_album_content,
            content::commands::scan_album_combined,
            content::commands::read_album_content,
            content::commands::search_photo_content,
            content::commands::search_photo_content_with_filters,
            get_vcr_gpu_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
