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
mod crypto;
mod db;
mod folder;
mod geo_index;
mod logger;
mod photo_info;
mod photo_scan;
mod persons;
mod session;
mod test_scan;
mod thumbnail;
mod tone;
mod vision;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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

/// 扫描任务取消标记（组合扫描/内容识别通用）
///
/// - 后端命令启动扫描时置 `false`，扫描循环每批/每步检查该标记
/// - 前端点击「停止」→ `cancel_scan` 置 `true` → 扫描在下个检查点提前结束
/// - `Arc<AtomicBool>` 保证跨异步任务 / 阻塞线程共享且线程安全
#[derive(Clone, Default)]
pub struct ScanState(pub Arc<AtomicBool>);

/// 请求停止当前扫描：置位取消标记，扫描循环在下一个检查点提前结束
#[tauri::command]
fn cancel_scan(scan: tauri::State<'_, ScanState>) -> Result<(), String> {
    scan.0.store(true, Ordering::SeqCst);
    logger::log_info("scan | 收到停止请求，已置取消标记");
    Ok(())
}

/// 取消标记是否已置位（true = 收到停止请求）
#[allow(dead_code)]
pub fn scan_cancelled(scan: &ScanState) -> bool {
    scan.0.load(Ordering::SeqCst)
}

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

/// 修改当前用户基本信息（邮箱/手机号），需先验证当前密码
#[tauri::command]
fn update_profile(
    input: auth::UpdateProfileInput,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<auth::User, String> {
    let _t = log_call!("update_profile", "id=***");
    let r = (|| -> Result<auth::User, String> {
        let user_id = require_user(&session)?;
        let db = state.0.lock().map_err(|e| e.to_string())?;
        auth::update_profile(db.conn(), user_id, input)
    })();
    match &r {
        Ok(u) => logger::log_call_end_with("update_profile", _t, &format!("OK | id={}", u.id)),
        Err(e) => logger::log_call_end_with("update_profile", _t, &format!("ERR | {e}")),
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

/// FEAT-036：批量填充每个相册的「已入库照片数」（photo_content_scan 中该相册的行数）。
/// 一次分组统计（count_scanned_by_album），避免逐相册 N+1 查询。
/// 多用户隔离：`user_id` 由调用方传入，仅统计当前用户已入库行。
fn fill_scanned_counts(albums: &mut [db::Album], user_id: i64, state: &tauri::State<AppState>) {
    let Ok(db) = state.0.lock() else { return };
    let Ok(map) = db.count_scanned_by_album(user_id) else { return };
    for a in albums.iter_mut() {
        a.scanned_photo_count = map.get(&a.id).copied().unwrap_or(0);
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
    // FEAT-036：批量填充每个相册的已入库照片数（一次 SQL 分组统计，避免 N+1）
    fill_scanned_counts(&mut albums, user_id, &state);
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
    // FEAT-036：填充该相册已入库照片数（单元素切片复用批量逻辑）
    fill_scanned_counts(std::slice::from_mut(&mut album), user_id, &state);
    Ok(album)
}

/// 列出相册文件夹内所有图片的绝对路径（供照片网格浏览）
///
/// 无需先执行内容扫描即可展示照片：轻量 walkdir 收集图片路径。
/// 多用户隔离：仅能列出归属当前用户的相册。
#[tauri::command]
fn list_album_photos(
    album_id: i64,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<Vec<String>, String> {
    let _t = log_call!("list_album_photos", &format!("album_id={album_id}"));
    let user_id = require_user(&session)?;
    let dir = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(album_id, user_id)
            .map_err(|e| e.to_string())?
            .path
    };
    // 过滤已被「记录删除」排除的照片（本地文件保留，但不再出现在网格中）
    let excluded: std::collections::HashSet<String> = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.list_excluded_photos(album_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect()
    };
    let mut count = 0usize;
    let paths: Vec<String> = crate::thumbnail::list_album_images(Path::new(&dir))
        .into_iter()
        .filter(|p| !excluded.contains(p))
        .inspect(|_| count += 1)
        .collect();
    logger::log_call_end_with("list_album_photos", _t, &format!("OK | count={count}"));
    Ok(paths)
}

/// 批量生成/复用照片网格缩略图（供前端分批懒加载）
///
/// - 输入：相册 id + 一批原图路径
/// - 输出：`[(原图路径, 缩略图缓存路径)]`，缓存命中直接复用，未命中生成 256px JPEG
/// - 在阻塞线程执行，避免首次生成占用异步运行时
#[tauri::command]
async fn get_photo_thumbs(
    album_id: i64,
    paths: Vec<String>,
    app: tauri::AppHandle,
    session: tauri::State<'_, SessionState>,
) -> Result<Vec<(String, String)>, String> {
    let _t = log_call!("get_photo_thumbs", &format!("album_id={album_id} paths={}", paths.len()));
    require_user(&session)?;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let requested = paths.len();
    let thumbs = thumbs_dir(&app)?;
    let res = tauri::async_runtime::spawn_blocking(move || {
        crate::thumbnail::ensure_grid_thumbs(album_id, &paths, &thumbs)
    })
    .await
    .map_err(|e| format!("缩略图任务线程失败: {e}"))?;
    logger::log_call_end_with("get_photo_thumbs", _t, &format!("OK | done={} requested={}", res.len(), requested));
    Ok(res)
}


/// 批量导出结果 —— 对应前端 `ExportOutcome`
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExportOutcome {
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub failed_paths: Vec<String>,
    pub dest_dir: String,
}

/// 批量导出照片：把选中的原图复制到目标目录（扁平化、重名自动加序号），可选生成信息清单
///
/// - `paths`：选中照片原图路径列表
/// - `dest_dir`：目标目录（需已通过文件夹对话框选择）
/// - `export_info`：是否同时写入 `导出清单.txt`（含导出时间与原始路径对照）
#[tauri::command]
async fn export_photos(
    paths: Vec<String>,
    dest_dir: String,
    export_info: bool,
    session: tauri::State<'_, SessionState>,
) -> Result<ExportOutcome, String> {
    let _t = log_call!("export_photos", &format!("n={} dest={dest_dir}", paths.len()));
    require_user(&session)?;
    if paths.is_empty() {
        return Ok(ExportOutcome { copied: 0, skipped: 0, failed: 0, failed_paths: vec![], dest_dir });
    }
    std::fs::create_dir_all(&dest_dir).map_err(|e| format!("创建导出目录失败: {e}"))?;

    let mut copied = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut failed_paths: Vec<String> = vec![];
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut manifest = String::new();

    for p in &paths {
        let base = std::path::Path::new(p);
        let Some(fname) = base.file_name().and_then(|s| s.to_str()) else { failed += 1; failed_paths.push(p.clone()); continue };
        let stem = std::path::Path::new(fname).file_stem().and_then(|s| s.to_str()).unwrap_or(fname);
        let ext = std::path::Path::new(fname).extension().and_then(|s| s.to_str()).unwrap_or("");

        // 重名去重：file.jpg → file_1.jpg → file_2.jpg
        let mut target_name = fname.to_string();
        let mut i = 1;
        while used.contains(&target_name) {
            target_name = if ext.is_empty() {
                format!("{stem}_{i}")
            } else {
                format!("{stem}_{i}.{ext}")
            };
            i += 1;
        }
        used.insert(target_name.clone());

        let target = std::path::Path::new(&dest_dir).join(&target_name);
        if target.exists() {
            skipped += 1;
            continue;
        }
        match std::fs::copy(p, &target) {
            Ok(_) => {
                copied += 1;
                manifest.push_str(&format!("{target_name}\t{p}\n"));
            }
            Err(e) => {
                failed += 1;
                failed_paths.push(format!("{p} ({e})"));
            }
        }
    }

    if export_info && copied > 0 {
        use std::io::Write;
        let mut now = "".to_string();
        if let Ok(secs) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            now = format!("{}", secs.as_secs());
        }
        let content = format!("导出时间戳：{now}\n共导出 {copied} 张\n\n原图路径清单：\n{manifest}");
        if let Ok(mut f) = std::fs::File::create(std::path::Path::new(&dest_dir).join("导出清单.txt")) {
            let _ = f.write_all(content.as_bytes());
        }
    }

    logger::log_call_end_with("export_photos", _t, &format!("OK | copied={copied} failed={failed} skipped={skipped}"));
    Ok(ExportOutcome { copied, skipped, failed, failed_paths, dest_dir })
}

/// 照片批量删除结果 —— 对应前端 `PhotoDeleteOutcome`
#[derive(Debug, Clone, serde::Serialize)]
pub struct PhotoDeleteOutcome {
    /// 请求删除的照片数
    pub requested: usize,
    /// 成功处理数（记录模式=排除数；文件模式=实际删掉的文件数）
    pub deleted: usize,
    pub failed: usize,
    pub failed_paths: Vec<String>,
}

/// 最近删除记录条目
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecentlyExcludedItem {
    pub album_id: i64,
    pub path: String,
    pub excluded_at: i64,
    pub album_name: String,
}

/// 批量「相册记录删除」：从该相册网格浏览中移除 + 清除扫描/AI 记录，本地文件保留
///
/// 可通过 restore 命令撤销（排除表回滚）。
#[tauri::command]
fn delete_photo_records(
    album_id: i64,
    paths: Vec<String>,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<PhotoDeleteOutcome, String> {
    let _t = log_call!("delete_photo_records", &format!("album_id={album_id} paths={}", paths.len()));
    let user_id = require_user(&session)?;
    let requested = paths.len();
    let outcome = if paths.is_empty() {
        PhotoDeleteOutcome { requested, deleted: 0, failed: 0, failed_paths: Vec::new() }
    } else {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        // 归属校验在 exclude_album_photos 内（非本人相册 → NotFound）
        let excluded = db.exclude_album_photos(album_id, user_id, &paths).map_err(|e| e.to_string())?;
        let removed = db.delete_content_by_paths(&paths).map_err(|e| e.to_string())?;
        logger::log_call_end_with("delete_photo_records", _t,
            &format!("OK | excluded={excluded} scan_removed={removed}"));
        PhotoDeleteOutcome { requested, deleted: excluded, failed: 0, failed_paths: Vec::new() }
    };
    Ok(outcome)
}

/// 批量「本地文件删除」：删除磁盘照片文件，并级联清理扫描记录、排除表与网格缩略图缓存
///
/// 危险操作：文件不可恢复，前端必须二次确认后才调用。
#[tauri::command]
fn delete_photo_files(
    album_id: i64,
    paths: Vec<String>,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
    app: tauri::AppHandle,
) -> Result<PhotoDeleteOutcome, String> {
    let _t = log_call!("delete_photo_files", &format!("album_id={album_id} paths={}", paths.len()));
    let user_id = require_user(&session)?;
    let requested = paths.len();
    if paths.is_empty() {
        return Ok(PhotoDeleteOutcome { requested, deleted: 0, failed: 0, failed_paths: Vec::new() });
    }
    // 相册归属校验（不直接操作 albums 表，借 exclude 的校验逻辑前置于事务外会写脏数据，
    // 因此先只读查一次归属）
    {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(album_id, user_id).map_err(|e| e.to_string())?;
    }
    // 1. 先算每张原图的缩略图缓存名（指纹依赖原文件存在，必须在删文件前算好）
    let thumb_names: Vec<String> = paths
        .iter()
        .map(|p| thumbnail::grid_thumb_cache_name(album_id, std::path::Path::new(p)))
        .collect();
    // 2. 删磁盘文件，逐张统计成败
    let mut deleted = 0usize;
    let mut failed_paths = Vec::new();
    for p in &paths {
        match std::fs::remove_file(p) {
            Ok(_) => deleted += 1,
            Err(e) => {
                logger::log_info(&format!("[delete_photo_files] 删除失败 path={p} err={e}"));
                failed_paths.push(p.clone());
            }
        }
    }
    // 3. 级联清理：缩略图缓存 + 成功删除文件的扫描记录与排除表（失败项保留原状可重试）
    if let Ok(thumbs) = thumbs_dir(&app) {
        thumbnail::remove_grid_thumb_files(&thumb_names, &thumbs);
    }
    if deleted > 0 {
        let ok_paths: Vec<String> = paths.iter().filter(|p| !failed_paths.contains(p)).cloned().collect();
        let db = state.0.lock().map_err(|e| e.to_string())?;
        let _ = db.exclude_album_photos(album_id, user_id, &ok_paths);
        let _ = db.delete_content_by_paths(&ok_paths);
    }
    let failed = failed_paths.len();
    let outcome = PhotoDeleteOutcome { requested, deleted, failed, failed_paths };
    logger::log_call_end_with("delete_photo_files", _t,
        &format!("OK | deleted={deleted} failed={failed}"));
    Ok(outcome)
}

/// 恢复已「记录删除」的照片（撤销删除）：从 album_photo_excluded 移除对应条目
#[tauri::command]
fn restore_photo_records(
    album_id: i64,
    paths: Vec<String>,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<usize, String> {
    let _t = log_call!("restore_photo_records", &format!("album_id={album_id} paths={}", paths.len()));
    let user_id = require_user(&session)?;
    if paths.is_empty() {
        return Ok(0);
    }
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let restored = db.restore_excluded_photos(album_id, user_id, &paths).map_err(|e| e.to_string())?;
    logger::log_call_end_with("restore_photo_records", _t,
        &format!("OK | restored={restored}"));
    Ok(restored)
}

/// 获取最近删除记录（用户可在此列表中恢复）
#[tauri::command]
fn list_recently_deleted(
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<Vec<RecentlyExcludedItem>, String> {
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.list_recently_excluded(user_id, 200).map_err(|e| e.to_string())
}

/// 清空所有最近删除记录
#[tauri::command]
fn clear_recently_deleted(
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<usize, String> {
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.clear_all_excluded(user_id).map_err(|e| e.to_string())
}

/// 给一批照片打分（rating 0-5，0 清除）。按 (user_id, path) upsert，无需扫描记录即可打分。
#[tauri::command]
fn set_photo_rating(
    paths: Vec<String>,
    rating: i64,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let _t = log_call!("set_photo_rating", &format!("paths={} rating={rating}", paths.len()));
    let user_id = require_user(&session)?;
    let rating = rating.clamp(0, 5);
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.set_photo_rating(user_id, &paths, rating)
        .map_err(|e| e.to_string())?;
    logger::log_call_end_with("set_photo_rating", _t, &format!("OK | n={}", paths.len()));
    Ok(())
}

/// 查询一批照片的打分，返回 [(path, rating)]（未打分的不会出现）。
#[tauri::command]
fn get_photo_ratings(
    paths: Vec<String>,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<Vec<(String, i64)>, String> {
    let _t = log_call!("get_photo_ratings", &format!("paths={}", paths.len()));
    let user_id = require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let r = db.get_photo_ratings(user_id, &paths).map_err(|e| e.to_string())?;
    logger::log_call_end_with("get_photo_ratings", _t, &format!("OK | n={}", r.len()));
    Ok(r)
}

/// 照片移动结果 —— 对应前端 `PhotoMoveOutcome`
#[derive(Debug, Clone, serde::Serialize)]
pub struct PhotoMoveOutcome {
    pub requested: usize,
    pub moved: usize,
    pub failed: usize,
    pub failed_paths: Vec<String>,
    pub target_id: i64,
}

/// 把一批照片移动到另一相册（物理移动文件进入目标相册文件夹，并同步内容/打分记录）。
/// 目标目录重名自动加 _1/_2 序号；成功后清理源相册的缩略图缓存。
#[tauri::command]
fn move_photos_to_album(
    album_id: i64,
    paths: Vec<String>,
    target_album_id: i64,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<PhotoMoveOutcome, String> {
    let _t = log_call!("move_photos_to_album", &format!("album_id={album_id} target={target_album_id} paths={}", paths.len()));
    let user_id = require_user(&session)?;
    if album_id == target_album_id {
        return Err("目标相册不能是当前相册".into());
    }
    let target_path = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(target_album_id, user_id).map_err(|e| e.to_string())?.path
    };
    std::fs::create_dir_all(&target_path).map_err(|e| format!("目标相册文件夹不可用: {e}"))?;
    // 目标目录已存在的文件名（避免覆盖）
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(rd) = std::fs::read_dir(&target_path) {
        for e in rd.flatten() {
            if let Some(name) = e.file_name().to_str() {
                used.insert(name.to_string());
            }
        }
    }
    let requested = paths.len();
    let mut moved = 0usize;
    let mut failed_paths = Vec::new();
    for src in &paths {
        if !std::path::Path::new(src).is_file() {
            failed_paths.push(src.clone());
            continue;
        }
        let base = std::path::Path::new(src);
        let fname = base.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or(fname);
        let ext = base.extension().and_then(|s| s.to_str()).unwrap_or("");
        let mut name = fname.to_string();
        let mut i = 1;
        while used.contains(&name) {
            name = if ext.is_empty() {
                format!("{stem}_{i}")
            } else {
                format!("{stem}_{i}.{ext}")
            };
            i += 1;
        }
        used.insert(name.clone());
        let dest = std::path::Path::new(&target_path).join(&name);
        let ok = std::fs::rename(src, &dest).is_ok()
            || (std::fs::copy(src, &dest).is_ok() && std::fs::remove_file(src).is_ok());
        if !ok {
            failed_paths.push(src.clone());
            continue;
        }
        moved += 1;
        let dest_str = dest.to_string_lossy().to_string();
        let db = state.0.lock().map_err(|e| e.to_string())?;
        let _ = db.move_photo_content_path(user_id, src, &dest_str, target_album_id);
        let _ = db.move_photo_rating_path(user_id, src, &dest_str);
    }
    // 清理源相册中已成功移走照片的缩略图缓存（失败不影响结果）
    if let Ok(thumbs) = thumbs_dir(&app) {
        let names: Vec<String> = paths
            .iter()
            .filter(|p| !failed_paths.contains(p))
            .map(|p| thumbnail::grid_thumb_cache_name(album_id, std::path::Path::new(p)))
            .collect();
        thumbnail::remove_grid_thumb_files(&names, &thumbs);
    }
    let failed = failed_paths.len();
    let out = PhotoMoveOutcome { requested, moved, failed, failed_paths, target_id: target_album_id };
    logger::log_call_end_with("move_photos_to_album", _t, &format!("OK | moved={moved} failed={failed}"));
    Ok(out)
}

/// 人物照片条目 —— 对应前端 `PersonPhotoItem`：
///  - path: 原图绝对路径
///  - thumb: 已算好的网格缩略图缓存路径（生成失败/未识别相册时为 None）
///  - album_id: 照片归属相册（解析失败为 None）
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonPhotoItem {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<i64>,
}

/// 读取某人物出现的全部照片：直接查询已算好的缩略图缓存地址，**不重新运算缩略图**。
/// 未缓存的照片返回 thumb=None，前端回退原图显示。
#[tauri::command]
fn get_person_photos(
    pid: String,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<Vec<PersonPhotoItem>, String> {
    let _t = log_call!("get_person_photos", &format!("pid={pid}"));
    let user_id = require_user(&session)?;
    let paths = crate::persons::list_person_photos(&pid)?;
    // 一次性取出本次用户相册 (id, path) 用于归属解析
    let albums: Vec<(i64, String)> = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_albums(user_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|a| (a.id, a.path))
            .collect()
    };
    let thumbs_dir = thumbs_dir(&app).ok();
    let mut out = Vec::with_capacity(paths.len());
    let mut generated = 0usize;
    for path in paths {
        // 解析归属相册 → 计算缩略图缓存名 → 若存在直接复用
        let resolved: Option<i64> = albums
            .iter()
            .filter(|(_, ap)| {
                p_is_under(ap, &path)
            })
            .max_by_key(|(_, ap)| ap.len())
            .map(|(id, _)| *id);
        let cached = resolved.and_then(|album_id| {
            let thumbs = thumbs_dir.as_ref()?;
            let name = thumbnail::grid_thumb_cache_name(album_id, std::path::Path::new(&path));
            let tp = thumbs.join("grid").join(&name);
            if tp.is_file() {
                Some(tp.to_string_lossy().to_string())
            } else {
                // 缺图 → 调用 ensure_grid_thumb 补齐（256px JPEG 生成后落盘，返回缓存路径），
                // 后续任何场景（PhotoGrid/Timeline/Memories/智能搜索）再访问都直接命中。
                // 补齐失败（原图丢失等）静默兑底 None，前端可回退占位。
                match thumbnail::ensure_grid_thumb(
                    album_id,
                    std::path::Path::new(&path),
                    thumbs,
                ) {
                    Ok(p) => {
                        generated += 1;
                        Some(p)
                    }
                    Err(_) => None,
                }
            }
        });
        out.push(PersonPhotoItem {
            path,
            thumb: cached,
            album_id: resolved,
        });
    }
    let cached_count = out.iter().filter(|i| i.thumb.is_some()).count();
    logger::log_call_end_with(
        "get_person_photos",
        _t,
        &format!(
            "OK | n={} thumb_hit={} generated={generated}",
            out.len(),
            cached_count.saturating_sub(generated),
        ),
    );
    Ok(out)
}

/// 判断照片路径是否位于相册目录之下（目录是祖先，且照片不是目录本身）。
fn p_is_under(dir: &str, photo: &str) -> bool {
    std::path::Path::new(photo)
        .strip_prefix(std::path::Path::new(dir))
        .ok()
        .map(|rel| !rel.as_os_str().is_empty())
        .unwrap_or(false)
}
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

/// 批量预热缩略图结果 —— 对应前端 `PrewarmOutcome`
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrewarmOutcome {
    pub requested: usize,
    pub hit: usize,
    pub generated: usize,
    pub failed: usize,
}

#[tauri::command]
fn prewarm_thumbs(
    album_id: i64,
    paths: Vec<String>,
    app: tauri::AppHandle,
    session: tauri::State<SessionState>,
) -> Result<PrewarmOutcome, String> {
    let _t = log_call!("prewarm_thumbs", &format!("album_id={album_id} paths={}", paths.len()));
    let _user = require_user(&session)?;
    let thumbs_dir = thumbs_dir(&app).map_err(|e| e.to_string())?;
    // 预统计已缓存数量（避免重复 IO）
    let mut hit = 0usize;
    let mut pending: Vec<&String> = Vec::with_capacity(paths.len());
    for p in &paths {
        let name = thumbnail::grid_thumb_cache_name(album_id, std::path::Path::new(p));
        let tp = thumbs_dir.join("grid").join(&name);
        if tp.is_file() {
            hit += 1;
        } else {
            pending.push(p);
        }
    }
    // 批量生成未命中的（ensure_grid_thumb 逐张内部仍逐张判断；性能足够）
    let mut generated = 0usize;
    let mut failed = 0usize;
    for p in &pending {
        match thumbnail::ensure_grid_thumb(album_id, std::path::Path::new(p), &thumbs_dir) {
            Ok(_) => generated += 1,
            Err(_) => failed += 1,
        }
    }
    let out = PrewarmOutcome {
        requested: paths.len(),
        hit,
        generated,
        failed,
    };
    logger::log_call_end_with(
        "prewarm_thumbs",
        _t,
        &format!(
            "OK | hit={} generated={generated} failed={failed}",
            out.hit
        ),
    );
    Ok(out)
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

/// 批量整理结果 —— 对应前端 `BatchAlbumOutcome`
#[derive(Debug, Clone, serde::Serialize)]
pub struct BatchAlbumOutcome {
    pub requested: usize,
    pub ok: usize,
    pub failed: usize,
    pub failed_ids: Vec<i64>,
}

fn _batch_album_outcome(requested: usize, failed_ids: Vec<i64>) -> BatchAlbumOutcome {
    let failed = failed_ids.len();
    BatchAlbumOutcome {
        requested,
        ok: requested.saturating_sub(failed),
        failed,
        failed_ids,
    }
}

/// 批量移动相册到指定分组（`folder_id`=None → 顶级/不分组）
///
/// 复用 `move_album` 的归属逻辑，逐相册执行并汇总成功/失败。多用户隔离。
#[tauri::command]
fn batch_move_album_to_folder(
    album_ids: Vec<i64>,
    folder_id: Option<i64>,
    state: tauri::State<'_, AppState>,
    session: tauri::State<'_, SessionState>,
) -> Result<BatchAlbumOutcome, String> {
    let _t = log_call!("batch_move_album_to_folder", &format!("ids={album_ids:?} folder={folder_id:?}"));
    let _user_id = require_user(&session)?;

    let mut failed_ids = Vec::new();
    for id in &album_ids {
        // 逐个执行（move_album 内部校验归属与目标分组）
        if move_album(*id, folder_id, state.clone(), session.clone()).is_err() {
            failed_ids.push(*id);
        }
    }

    let out = _batch_album_outcome(album_ids.len(), failed_ids);
    logger::log_call_end_with(
        "batch_move_album_to_folder",
        _t,
        &format!("OK | ok={} failed={}", out.ok, out.failed),
    );
    Ok(out)
}

/// 批量设置相册地点（可清空：传空字符串）
#[tauri::command]
fn batch_set_album_location(
    album_ids: Vec<i64>,
    location: String,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<BatchAlbumOutcome, String> {
    let _t = log_call!("batch_set_album_location", &format!("ids={album_ids:?} loc={location}"));
    let user_id = require_user(&session)?;
    let mut failed_ids = Vec::new();
    {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        for id in &album_ids {
            if db.update_album_location(*id, user_id, &location).is_err() {
                failed_ids.push(*id);
            }
        }
    }
    let out = _batch_album_outcome(album_ids.len(), failed_ids);
    logger::log_call_end_with(
        "batch_set_album_location",
        _t,
        &format!("OK | ok={} failed={}", out.ok, out.failed),
    );
    Ok(out)
}

/// 批量加/删相册标签（mode：`add` 追加 / `remove` 移除；最多 5 个）
#[tauri::command]
fn batch_set_album_tag(
    album_ids: Vec<i64>,
    tags: Vec<String>,
    mode: String,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<BatchAlbumOutcome, String> {
    let _t = log_call!("batch_set_album_tag", &format!("ids={album_ids:?} mode={mode} tags={tags:?}"));
    let user_id = require_user(&session)?;
    if mode != "add" && mode != "remove" {
        return Err("mode 仅支持 add / remove".into());
    }
    let clean: Vec<String> = tags
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if mode == "add" && clean.is_empty() {
        return Err("请至少输入一个标签".into());
    }

    let mut failed_ids = Vec::new();
    {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        for id in &album_ids {
            let r = (|| -> Result<(), String> {
                let existing = db.get_album_tag_list(*id, user_id).map_err(|e| e.to_string())?;
                if mode == "add" {
                    let mut merged = existing.clone();
                    for t in &clean {
                        if !merged.iter().any(|x| x == t) {
                            merged.push(t.clone());
                        }
                    }
                    if merged.len() > 5 {
                        return Err("标签数不能超过 5 个".into());
                    }
                    db.update_album_tags(*id, user_id, merged).map_err(|e| e.to_string())
                } else {
                    let filtered: Vec<String> = existing
                        .into_iter()
                        .filter(|x| !clean.contains(x))
                        .collect();
                    db.update_album_tags(*id, user_id, filtered).map_err(|e| e.to_string())
                }
            })();
            if r.is_err() {
                failed_ids.push(*id);
            }
        }
    }
    let out = _batch_album_outcome(album_ids.len(), failed_ids);
    logger::log_call_end_with(
        "batch_set_album_tag",
        _t,
        &format!("OK | mode={mode} ok={} failed={}", out.ok, out.failed),
    );
    Ok(out)
}

/// 批量整理合并结果 —— 对应前端 `MergeAlbumOutcome`
#[derive(Debug, Clone, serde::Serialize)]
pub struct MergeAlbumOutcome {
    /// 请求的源相册数（含最终被跳过的自合并/同目录）
    pub requested: usize,
    /// 成功合并（文件全部移动 + 记录已删）的源相册数
    pub merged: usize,
    pub files_moved: usize,
    pub files_failed: usize,
    /// 因存在移动失败而保留记录的源相册 ID
    pub skipped: Vec<i64>,
    /// 整体出错的源相册 ID
    pub failed_ids: Vec<i64>,
    pub target_id: i64,
}

/// 合并相册：把源相册文件夹中所有照片**物理移动**到目标相册文件夹，
/// 再把源相册记录及关联数据（统计/内容/标签/分组/排除表）一并删除。
///
/// - 重名自动加序号（`_1`、`_2`…）避免覆盖；同卷用 rename，跨卷回退 copy+remove
/// - 仅当源相册所有照片成功移动后才删除其记录；有失败则保留记录并列入 `skipped`
/// - `mode="move"`（默认）：照片**物理移动**进目标相册文件夹；`mode="record"`：仅删除源相册记录、**不移动文件**（文件保留在磁盘原处）
/// - 多用户隔离：仅能合并归属当前用户的相册
#[tauri::command]
fn merge_albums(
    source_ids: Vec<i64>,
    target_id: i64,
    mode: Option<String>,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    session: tauri::State<SessionState>,
) -> Result<MergeAlbumOutcome, String> {
    let mode = mode.as_deref().unwrap_or("move");
    let is_move = mode != "record";
    let _t = log_call!("merge_albums", &format!("source={source_ids:?} target={target_id} mode={mode}"));
    let user_id = require_user(&session)?;

    let target_path = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(target_id, user_id).map_err(|e| e.to_string())?.path
    };
    let db = state.0.lock().map_err(|e| e.to_string())?;

    // 目标目录中已存在的文件名（避免覆盖）—— 仅物理移动模式需要
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    if is_move {
        std::fs::create_dir_all(&target_path).map_err(|e| format!("目标相册文件夹不可用: {e}"))?;
        if let Ok(rd) = std::fs::read_dir(&target_path) {
            for e in rd.flatten() {
                if let Some(name) = e.file_name().to_str() {
                    used.insert(name.to_string());
                }
            }
        }
    }

    let mut merged = 0usize;
    let mut files_moved = 0usize;
    let mut files_failed = 0usize;
    let mut skipped = Vec::new();
    let mut failed_ids = Vec::new();
    let mut removed_ids = Vec::new();
    // 合并来源收集：成功删除源记录后插入 album_merged_sources，供卡片显示历史来源
    // 收集顺序为处理顺序（去重：同一源不会被处理多次）。
    let mut merged_sources_to_record: Vec<(i64, String, String)> = Vec::new();

    for sid in &source_ids {
        if *sid == target_id {
            continue; // 不能合并到自身
        }
        // 先取源相册信息（无论 move/record 都要）；取不到则失败跳过
        let src_album = match db.get_album(*sid, user_id) {
            Ok(a) => a,
            Err(_) => {
                failed_ids.push(*sid);
                continue;
            }
        };
        // 同目录无需移动，但来源仍可记录（语义上 = 标记为合并来源）
        // 这里只在删除前检查路径一致性
        if is_move {
            let src_path = src_album.path.clone();
            if src_path != target_path {
                let src_images = crate::thumbnail::list_album_images(std::path::Path::new(&src_path));
                let mut moved_ok = true;
                for src in &src_images {
                    let base = std::path::Path::new(src);
                    let fname = base.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or(fname);
                    let ext = base.extension().and_then(|s| s.to_str()).unwrap_or("");
                    // 重名去重
                    let mut name = fname.to_string();
                    let mut i = 1;
                    while used.contains(&name) {
                        name = if ext.is_empty() {
                            format!("{stem}_{i}")
                        } else {
                            format!("{stem}_{i}.{ext}")
                        };
                        i += 1;
                    }
                    used.insert(name.clone());
                    let dest = std::path::Path::new(&target_path).join(&name);
                    // 同卷 rename，失败再尝试 copy+remove
                    let ok = std::fs::rename(src, &dest).is_ok()
                        || (std::fs::copy(src, &dest).is_ok() && std::fs::remove_file(src).is_ok());
                    if ok {
                        files_moved += 1;
                    } else {
                        files_failed += 1;
                        moved_ok = false;
                    }
                }
                // 尽力清理已空的原文件夹
                let _ = std::fs::remove_dir(&src_path);
                if !moved_ok {
                    skipped.push(*sid);
                    continue;
            }
            }
        }
        // 收集来源（删除前先记录）
        merged_sources_to_record.push((src_album.id, src_album.name, src_album.path));

        if db.delete_album(*sid, user_id).is_ok() {
            merged += 1;
            removed_ids.push(*sid);
        } else {
            failed_ids.push(*sid);
        }
    }

    // 在事务内把成功合并的源相册信息写入 album_merged_sources（供卡片显示历史来源）。
    // 只对 merged 的源记录；唯一约束 (album_id, source_id) 防重复。
    if !merged_sources_to_record.is_empty() {
        match db.conn().unchecked_transaction() {
            Ok(tx) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let mut had_error = false;
                for (sid, sname, spath) in &merged_sources_to_record {
                    if tx.execute(
                        "INSERT OR IGNORE INTO album_merged_sources
                           (album_id, source_id, source_name, source_path, user_id, merged_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![target_id, sid, sname, spath, user_id, now],
                    ).is_err() {
                        had_error = true;
                        break;
                    }
                }
                if had_error {
                    // 写来源失败不回滚合并：用户已经合并成功了，写来源只是元信息
                    let _ = tx.rollback();
                } else {
                    let _ = tx.commit();
                }
            }
            Err(_) => {
                // 事务创建失败不阻塞合并主流程
            }
        }
    }

    // 清理已删除源相册的缩略图缓存（失败不影响结果）
    if let Ok(thumbs) = thumbs_dir(&app) {
        for id in &removed_ids {
            thumbnail::cleanup_all_album_thumbs(*id, &thumbs);
        }
    }

    drop(db);
    let skipped_count = skipped.len();
    let out = MergeAlbumOutcome {
        requested: source_ids.len(),
        merged,
        files_moved,
        files_failed,
        skipped,
        failed_ids,
        target_id,
    };

    logger::log_call_end_with(
        "merge_albums",
        _t,
        &format!("OK | merged={merged} files_moved={files_moved} files_failed={files_failed} skipped={skipped_count}"),
    );
    Ok(out)
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

/// 读取单张照片信息（分辨率/文件大小/RGB 像素分布直方图，按需实时读，不落库）
///
/// 大图查看器「详细信息」面板专用；解码在阻塞线程执行避免卡异步运行时。
#[tauri::command]
async fn get_photo_info(path: String) -> Result<photo_info::PhotoInfo, String> {
    let _t = log_call!("get_photo_info", &format!("path={path}"));
    let r = tauri::async_runtime::spawn_blocking(move || photo_info::read_photo_info(&path))
        .await
        .map_err(|e| format!("照片信息任务线程失败: {e}"))?;
    match &r {
        Ok(info) => logger::log_call_end_with(
            "get_photo_info",
            _t,
            &format!("OK | {}x{} size={}", info.width, info.height, info.file_size),
        ),
        Err(e) => logger::log_call_end_with("get_photo_info", _t, &format!("ERR | {e}")),
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
    scan: tauri::State<'_, ScanState>,
) -> Result<Vec<vision::VisionResult>, String> {
    let _t = log_call!("classify_album", &format!("path={path}"));
    scan.0.store(false, Ordering::SeqCst);
    let r = vision::classify_album(
        &path,
        batch_size.unwrap_or(8).max(1) as usize,
        &app,
        Some(scan.0.clone()),
    )
    .await;
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

/// 人物注册表：列出全部已标号人物（直读 persons.db，按脸数降序；不依赖微服务）
#[tauri::command]
fn list_persons() -> Result<Vec<persons::PersonEntry>, String> {
    let _t = log_call!("list_persons", "db-direct");
    let r = persons::list_persons();
    match &r {
        Ok(list) => logger::log_call_end_with("list_persons", _t, &format!("OK | n={}", list.len())),
        Err(e) => logger::log_call_end_with("list_persons", _t, &format!("ERR | {e}")),
    }
    r
}

/// 人物注册表：列出某人物出现的全部照片路径（直读 persons.db；供前端展示缩略图）
#[tauri::command]
fn list_person_photos(pid: String) -> Result<Vec<String>, String> {
    let _t = log_call!("list_person_photos", &format!("pid={pid}"));
    let r = persons::list_person_photos(&pid);
    match &r {
        Ok(list) => logger::log_call_end_with("list_person_photos", _t, &format!("OK | n={}", list.len())),
        Err(e) => logger::log_call_end_with("list_person_photos", _t, &format!("ERR | {e}")),
    }
    r
}

/// 人物头像缓存目录（app_data_dir/avatars）
fn avatars_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {e}"))?;
    Ok(data_dir.join("avatars"))
}

/// 获取人物头像（本地优先：磁盘缓存命中直接返回，未命中则从代表脸 bbox 本地裁剪）
///
/// 完全离线可用，不再依赖 Python 微服务。
#[tauri::command]
async fn get_person_avatar(
    pid: String,
    force_refresh: Option<bool>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let _t = log_call!("get_person_avatar", &format!("pid={pid} force={force_refresh:?}"));
    let dir = avatars_dir(&app)?;
    let cache_path = dir.join(format!("avatar_{pid}.jpg"));
    if !force_refresh.unwrap_or(false) && cache_path.is_file() {
        logger::log_call_end_with("get_person_avatar", _t, "OK | cache");
        return Ok(cache_path.to_string_lossy().into_owned());
    }
    // 裁剪解码在阻塞线程执行，避免大图解码占用异步运行时
    let r = tauri::async_runtime::spawn_blocking(move || {
        persons::crop_avatar_local(&pid, &cache_path)?;
        Ok(cache_path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| format!("头像任务线程失败: {e}"))?;
    match &r {
        Ok(_) => logger::log_call_end_with("get_person_avatar", _t, "OK | cropped"),
        Err(e) => logger::log_call_end_with("get_person_avatar", _t, &format!("ERR | {e}")),
    }
    r
}

/// 人物注册表：重命名人物（直写 persons.db）
#[tauri::command]
fn rename_person(pid: String, name: String) -> Result<(), String> {
    let _t = log_call!("rename_person", &format!("pid={pid}"));
    let r = persons::rename_person(&pid, &name);
    match &r {
        Ok(_) => logger::log_call_end_with("rename_person", _t, "OK"),
        Err(e) => logger::log_call_end_with("rename_person", _t, &format!("ERR | {e}")),
    }
    r
}

/// 人物注册表：合并人物（source 并入 target；直写 persons.db，质心加权平均与 Python 逻辑一致）
#[tauri::command]
fn merge_persons(target: String, source: String) -> Result<(), String> {
    let _t = log_call!("merge_persons", &format!("target={target} source={source}"));
    let r = persons::merge_persons(&target, &source);
    match &r {
        Ok(_) => logger::log_call_end_with("merge_persons", _t, "OK"),
        Err(e) => logger::log_call_end_with("merge_persons", _t, &format!("ERR | {e}")),
    }
    r
}

/// 人物注册表：删除人物（直写 persons.db，离线可用；同步清理头像缓存）
#[tauri::command]
fn delete_person(
    pid: String,
    app: tauri::AppHandle,
    session: tauri::State<SessionState>,
) -> Result<(), String> {
    let _t = log_call!("delete_person", &format!("pid={pid}"));
    require_user(&session)?;
    let r = persons::delete_person(&pid);
    if r.is_ok() {
        // 头像缓存文件已无意义，一并清理
        if let Ok(dir) = avatars_dir(&app) {
            let _ = std::fs::remove_file(dir.join(format!("avatar_{pid}.jpg")));
        }
        logger::log_call_end_with("delete_person", _t, "OK");
    } else if let Err(e) = &r {
        logger::log_call_end_with("delete_person", _t, &format!("ERR | {e}"));
    }
    r
}

/// GPU 加速可行性（R3）：确保服务就绪后查询 /gpu，返回是否可用 GPU
#[tauri::command]
async fn get_vcr_gpu_status(app: tauri::AppHandle) -> Result<vision::VcrGpuStatus, String> {
    vision::vcr_gpu_status(&app).await
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
    /// FEAT-034-C：路径已被其他用户占用导致跳过的条目明细
    /// 元素格式：{ folder: "xxx", conflict_album: "已存在相册名" }
    /// 这些项目本质不重复入档（path 全局 UNIQUE），对当前用户是「已存在」友好提示。
    pub skipped_conflicts: Vec<SkippedConflict>,
}

#[derive(Debug, serde::Serialize)]
pub struct SkippedConflict {
    pub folder: String,
    pub conflict_album: String,
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
        skipped_conflicts: Vec::new(),
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

        // FEAT-034-C：检查是否已作为相册存在
        // 1) 先查当前用户：同 user_id 下已存在 → skipped（不重计）
        // 2) 查任一用户：path 全局 UNIQUE 被其他用户占用 → 跳冲突明细（不报错）
        //    这避免了多用户迁移后旧数据被 admin 接管、新用户再批量导入时全部误报
        //    「已被相册 X 使用」错位问题。
        if let Ok(Some(_)) = db.find_album_by_path(&path_str, user_id) {
            result.skipped += 1;
        } else {
            match db.find_any_album_by_path(&path_str) {
                Ok(Some(other)) => {
                    // path 已被其他用户的相册占用（全局 UNIQUE 冲突）。
                    // 视为友好跳过，不计入 errors；保留明细供前端提示。
                    result.skipped += 1;
                    result.skipped_conflicts.push(SkippedConflict {
                        folder: folder_name.clone(),
                        conflict_album: other.name,
                    });
                }
                Ok(None) => {
                    // 真正未占用：创建相册
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
                Err(e) => {
                    // 查询出错 → 仍报告为错误（避免静默丢失）
                    result.errors.push(format!("{folder_name}: {e}"));
                }
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
            // 初始化日志组件（保留 3 天 = 4320 分钟）
            logger::init(&data_dir, 4320);
            // 初始化用户敏感字段加密密钥（必须早于数据库迁移，迁移需用密钥加密历史明文）
            crypto::init(&data_dir).expect("初始化应用加密密钥失败");
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
                    app.manage(ScanState::default());
                }
                None => {
                    // token 缺失/失效 → 清理磁盘文件，按未登录启动
                    session::clear_token_file(&data_dir);
                    app.manage(AppState(Mutex::new(database)));
                    app.manage(SessionState(Mutex::new(None)));
                    app.manage(ScanState::default());
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
            update_profile,
            // 相册管理（按用户隔离）
            create_album,
            get_albums,
            get_album,
            update_album,
            prewarm_thumbs,
            list_album_photos,
            get_photo_thumbs,
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
            batch_move_album_to_folder,
            batch_set_album_location,
            batch_set_album_tag,
            merge_albums,
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
            get_photo_info,
            delete_photo_records,
            delete_photo_files,
            restore_photo_records,
            list_recently_deleted,
            clear_recently_deleted,
            set_photo_rating,
            get_photo_ratings,
            move_photos_to_album,
            classify_album,
            list_persons,
            list_person_photos,
            get_person_photos,
            get_person_avatar,
            rename_person,
            merge_persons,
            delete_person,
            content::commands::scan_album_content,
            content::commands::scan_album_combined,
            content::commands::read_album_content,
            content::commands::search_photo_content,
            content::commands::ensure_photo_scanned,
            content::commands::search_photo_content_with_filters,
            content::commands::list_timeline,
            content::commands::smart_search,
            export_photos,
            get_vcr_gpu_status,
            cancel_scan,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
