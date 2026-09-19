/// AOP 日志宏：在命令/函数入口记录调用开始，返回计时器
///
/// 用法（放在函数第一行）：
/// ```ignore
/// // crate 内宏，doctest（独立 crate）无法直接编译，标为 ignore 仅作展示
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
mod avatar;
mod category;
mod content;
mod crypto;
mod db;
mod devdata;
mod devtools;
mod folder;
mod geo_index;
mod logger;
mod model_dl;
mod photo_info;
mod photo_scan;
mod persons;
mod session;
mod studio;
mod test_scan;
mod textdesc;
mod thumbnail;
mod tone;
mod vision;
mod photos;
mod album;
mod vcr_settings;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use db::Database;
use tauri::Manager;

// 开发者视角命令（lib.rs 瘦身第一期：自本文件迁出至 devtools.rs，行为零改动；
// 原样 use 引回使 generate_handler 列表与前端 invoke 路径保持不变）
use devtools::{
    app_info, dev_data_paths, dev_db_rows, dev_db_sql, dev_db_tables, dev_reveal_path,
    open_dev_data_window, open_dev_log_window, tail_dev_log,
};
use album::{
    auto_detect_album_location,
    batch_move_album_to_folder,
    batch_set_album_location,
    batch_set_album_tag,
    create_album,
    delete_album,
    delete_albums,
    get_album,
    get_albums,
    import_albums,
    list_album_photos,
    merge_albums,
    move_album,
    rename_album,
    reorder_album,
    search_albums,
    set_cover,
    update_album,
    update_album_tags,
};
use auth::commands::{
    get_current_user,
    login,
    logout,
    register,
    reset_password,
    update_profile,
};
use avatar::commands::{
    clear_user_avatar,
    get_person_avatar,
    get_person_avatars_bulk,
    set_person_avatar_from_photo,
    set_user_avatar,
};
use content::classify_album;
use folder::commands::{
    create_folder,
    delete_folder,
    get_manual_tree,
    reorder_folder,
    update_folder,
};
use model_dl::commands::{
    cancel_model_download,
    get_model_sources,
    list_model_downloads,
    probe_model_sources,
    set_model_sources,
    start_model_download,
};
use persons::commands::{
    delete_person,
    get_person_photos,
    list_person_photos,
    list_persons,
    merge_persons,
    rename_person,
};
use photo_info::commands::{get_photo_info};
use photo_scan::commands::{scan_album_photos, scan_album_photos_local_place, scan_album_photos_with_place};
use photos::{
    clear_recently_deleted,
    delete_photo_files,
    delete_photo_records,
    delete_photo_records_by_paths,
    delete_photos_to_trash,
    export_photos,
    get_photo_ratings,
    get_photo_thumbs,
    get_photo_thumbs_count,
    list_recently_deleted,
    move_photos_to_album,
    open_folder,
    prewarm_thumbs,
    restore_photo_records,
    set_photo_rating,
};
use tone::commands::{scan_album_tones};
use vcr_settings::{
    benchmark_vcr,
    benchmark_vcr_sweep,
    get_vcr_gpu_status,
    get_vcr_threads,
    list_vcr_models,
    set_vcr_gpu,
    set_vcr_model,
    set_vcr_threads,
};


/// 缩略图缓存目录名（位于 app_data_dir 下）
pub(crate) const THUMBS_DIR: &str = "thumbs";

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
pub(crate) fn require_user(session: &tauri::State<SessionState>) -> Result<i64, String> {
    let guard = session.0.lock().map_err(|e| e.to_string())?;
    guard.ok_or_else(|| "请先登录".to_string())
}


/// 获取缩略图缓存目录（app_data_dir/thumbs）
pub(crate) fn thumbs_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {e}"))?;
    Ok(data_dir.join(THUMBS_DIR))
}

/// 应用数据目录（记住登录 token 文件所在）
pub(crate) fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {e}"))
}


/// 当前时间戳（reorder_folder 内部用）
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}


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
            // panic 落盘：任何线程崩溃（含白屏类 WebView 异常之外的原生崩溃）都可在日志追溯
            logger::install_panic_hook();
            logger::log_info(&format!(
                "==== APP START v{} pid={} data_dir={} ====",
                env!("CARGO_PKG_VERSION"),
                std::process::id(),
                data_dir.display()
            ));
            // 打包版：解析「模型目录」并写入进程环境变量（VCR_MODEL_DIR）
            //   - 随 MSI/NSIS 安装的模型位于 resource_dir/vcr/models；
            //   - MSI 默认装到 Program Files（普通权限进程不可写），而语义子图拆分 /
            //     档位持久化 / 应用内模型下载都要写模型目录 → 该目录不可写时自动在
            //     app_data_dir 下建立硬链接（或复制）副本并改用它，
            //     见 vision::resolve_model_dir；
            //   - clip_model_present()（vision.rs）与 model_dl::models_dir() 均按
            //     VCR_MODEL_DIR 解析（此前硬编码 CARGO_MANIFEST_DIR，打包后指向构建机
            //     源码路径，导致安装版「模型已内置却显示未下载」）。
            {
                let h = app.handle().clone();
                let model_dir = vision::resolve_model_dir(&h);
                logger::log_info(&format!("模型目录: {}", model_dir.display()));
                std::env::set_var("VCR_MODEL_DIR", &model_dir);
            }
            // 数据目录统一（VCR_DATA_DIR）：人物库 persons.db 落 app_data_dir/vcr-data
            //   - 此前相册库在 %APPDATA%、人物库却由编译期常量指向项目目录 python/data，
            //     造成「安装版人物页读到开发库、微服务另写一份」的读写分裂
            //     （BUG-2026-0916-005）；
            //   - persons.rs（人物页直读）与 spawn_server（注入子进程）均按该变量解析；
            //   - 外部已显式设置则不覆盖（便于调试指向别处）。
            if std::env::var("VCR_DATA_DIR").is_err() {
                let vcr_data = data_dir.join("vcr-data");
                logger::log_info(&format!("人物数据目录: {}", vcr_data.display()));
                std::env::set_var("VCR_DATA_DIR", &vcr_data);
            }
            // 开发诊断：PMS_AUTO_OPEN_DEVLOG=1 时启动 4 秒后自动打开日志副窗口，
            // 免点击复现打开链路（测量窗口创建/首帧耗时），日常使用不设置即可
            if std::env::var("PMS_AUTO_OPEN_DEVLOG").as_deref() == Ok("1") {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    let _ = open_dev_log_window(h);
                });
            }
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
            let token = session::read_token_file(&data_dir);
            let restored_user = token.as_ref().and_then(|token| {
                session::validate_remember_session(database.conn(), token)
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
                    // （本地有 token 但没恢复成功 = 会话过期/失效，属正常但值得留痕）
                    if token.is_some() {
                        logger::log_warn(
                            "记住登录未生效（token 过期或会话失效），本次按未登录启动，已清理本地 token",
                        );
                    }
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
            set_user_avatar,
            clear_user_avatar,
            // 相册管理（按用户隔离）
            create_album,
            get_albums,
            get_person_avatars_bulk,
            app_info,
            dev_data_paths,
            dev_db_tables,
            dev_db_rows,
            dev_db_sql,
            dev_reveal_path,
            get_album,
            update_album,
            prewarm_thumbs,
            get_photo_thumbs_count,
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
            delete_photos_to_trash,
            delete_photo_records_by_paths,
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
            set_person_avatar_from_photo,
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
            content::commands::list_content_categories,
            content::commands::list_photos_by_category,
            category::commands::list_categories,
            category::commands::save_category,
            category::commands::delete_category,
            category::commands::preview_category,
            category::commands::rebuild_categories,
            category::commands::list_category_photos,
            category::commands::category_index_stats,
            content::commands::list_photo_locations,
            content::commands::list_photos_by_location,
            content::commands::set_photo_tags,
            content::commands::get_photo_tags,
            content::commands::smart_search,
            content::commands::warmup_semantic_service,
            // FEAT-067：以图搜图 + 描述向量（人物编号不入向量，人物走 faces 精确过滤）
            textdesc::commands::smart_search_by_image,
            textdesc::commands::rebuild_desc_index,
            textdesc::commands::get_desc_person_name,
            textdesc::commands::set_desc_person_name,
            export_photos,
            get_vcr_gpu_status,
            start_model_download,
            list_model_downloads,
            cancel_model_download,
            probe_model_sources,
            get_model_sources,
            set_model_sources,
            set_vcr_gpu,
            get_vcr_threads,
            set_vcr_threads,
            benchmark_vcr_sweep,
            list_vcr_models,
            set_vcr_model,
            benchmark_vcr,
            cancel_scan,
            // 开发者视角（实时日志副窗口）
            tail_dev_log,
            open_dev_log_window,
            open_dev_data_window,
            // 创意工坊（FEAT-063）：Python 微服务生命周期 + 结果落盘
            studio::studio_ensure,
            studio::studio_save_result,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
    // 事件循环正常退出（异常退出走 panic 钩子落盘）
    logger::log_info("==== APP EXIT ====");
}

#[cfg(test)]
mod tests {
    use crate::persons::commands::p_is_under;

    /// BUG-2026-0916-001：归属解析归一化 —— 大小写 / 分隔符 / 前缀边界
    #[test]
    fn p_is_under_normalized() {
        // 基本包含
        assert!(p_is_under(r"D:\Pics\album", r"D:\Pics\album\a.jpg"));
        // 大小写不敏感（NTFS）
        assert!(p_is_under(r"D:\Pics\Album", r"d:\pics\album\a.jpg"));
        // 分隔符混用（正斜杠记录）
        assert!(p_is_under(r"D:\Pics\album", "D:/Pics/album/b/c.jpg"));
        // 相册路径带尾分隔符
        assert!(p_is_under(r"D:\Pics\album\", r"D:\Pics\album\a.jpg"));
        // 前缀边界：D:\Pics\alb 不应匹配 D:\Pics\album\a.jpg
        assert!(!p_is_under(r"D:\Pics\alb", r"D:\Pics\album\a.jpg"));
        // 照片即目录本身（无剩余文件部分）
        assert!(!p_is_under(r"D:\Pics\album", r"D:\Pics\album"));
        assert!(!p_is_under(r"D:\Pics\album", r"D:\Pics\album\"));
        // 目录之外 / 兄弟目录
        assert!(!p_is_under(r"D:\Pics\album", r"D:\Other\a.jpg"));
        assert!(!p_is_under(r"D:\Pics\album", r"D:\Pics\album2\a.jpg"));
    }
}
