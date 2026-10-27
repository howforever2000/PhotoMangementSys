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
mod devpath;
mod devtools;
mod exif_orient;
mod edit;
mod folder;
mod geo_index;
mod logger;
mod model_dl;
mod photo_info;
mod photo_scan;
mod persons;
mod photo_mark;
mod scan_perf;
mod scan_timing;
mod session;
mod studio;
mod test_scan;
mod test_scan_job;
mod textdesc;
mod thumb_migrate;
mod thumbnail;
mod tone;
mod vision;
mod photos;
mod album;
mod vcr_settings;

use std::path::{Path, PathBuf};
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
    verify_current_password,
};
use avatar::commands::{
    clear_user_avatar,
    get_person_avatar,
    get_person_avatars_bulk,
    restore_person_avatar,
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
    list_persons_in_album,
    merge_persons,
    rebuild_person_registry,
    rename_person,
};
use photo_info::commands::{get_photo_info};
use photo_mark::{get_photo_mark_states, list_marked_photos, set_photo_marks};
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
    get_face_tier_info,
    get_vcr_gpu_status,
    get_vcr_threads,
    list_vcr_models,
    set_face_tier,
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


/// 启动自检：数据目录必须**可创建、可写**。
///
/// 为什么值得单独一步（BUG-2026-1026-002）：
///   目录不可写时，SQLite 给出的报错是
///     `attempt to write a readonly database`（ReadOnly / 8）
///   或 `unable to open database file`（CannotOpen / 14）
///   —— 两条都完全看不出"是权限问题"。实测：同一个 exe 放在可写位置启动正常，
///   放在写不进去的位置（被沙箱/安全软件限制、或目录被占）就以这两种错误崩在
///   `Database::open`，日志里只有一句"数据库初始化失败"，排查方向完全靠猜。
///   提前探一次写，能把"权限/占用"和"库损坏"彻底分开。
fn ensure_dir_writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".write_probe");
    std::fs::write(&probe, b"ok")?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// 数据目录解析：`PMS_DATA_DIR` 优先，否则用 Tauri 的 app_data_dir。
///
/// 为什么留这个开关：受限环境（沙箱 / 只读用户目录 / 便携部署 / 排障）下
/// app_data_dir 可能写不进去，而库本身是好的。给一个显式入口，
/// 就能在不改系统权限的前提下把数据挪到可写目录。默认行为完全不变。
fn resolve_data_dir(app: &tauri::AppHandle) -> PathBuf {
    if let Some(raw) = std::env::var_os("PMS_DATA_DIR") {
        let p = PathBuf::from(raw);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    app.path()
        .app_data_dir()
        .expect("无法获取应用数据目录")
}

/// 数据目录不可写时的统一退出：先落盘日志，再打印到 stderr，最后退出。
/// 不 panic —— panic 的调用栈对用户没有任何信息量。
fn fatal_data_dir(dir: &Path, err: &std::io::Error) -> ! {
    let msg = format!(
        "数据目录不可写：{}\n原因：{err}\n\
         排查：① 该目录是否被安全软件/沙箱/权限限制；② 是否已有实例占用；\
         ③ 可用环境变量 PMS_DATA_DIR 指定一个可写目录后重试。",
        dir.display()
    );
    logger::log_error("startup", &msg);
    eprintln!("[FATAL] {msg}");
    std::process::exit(1)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 数据库文件存放于 Tauri 的 app_data_dir
            // Windows: %APPDATA%/com.photomanagementsys.app/photos.db
            let data_dir = resolve_data_dir(app.handle());
            /* 启动自检（BUG-2026-1026-002）：写不进去就不要往下走。
               放在迁移与 logger 之前 —— 这两步自己也要写盘，先把"能不能写"问清楚，
               后面才不会被一个看不懂的 SQLite 报错误导（见 ensure_dir_writable 注释）。 */
            if let Err(e) = ensure_dir_writable(&data_dir) {
                fatal_data_dir(&data_dir, &e);
            }
            // 一次性迁移：标识符由 com.haoyuan.* 改为中性名后，老用户的
            // photos.db / thumbs / vcr-data / **app.key** 会留在旧目录，
            // 应用照常启动但相册为空、加密字段无法解密（BUG-2026-1014-001）。
            //
            // 必须早于 logger::init 与一切 app_data_dir 消费：否则日志写进新目录、
            // 数据还在旧目录，排障时两边对不上。
            //
            // 指定了 PMS_DATA_DIR 时跳过：那时 data_dir 是用户挑的目录，
            // 它旁边不存在"旧标识符目录"，迁移只会白跑一次文件枚举。
            let migrate_note = if std::env::var_os("PMS_DATA_DIR").is_some() {
                "已指定 PMS_DATA_DIR，跳过旧目录迁移".to_string()
            } else {
                devpath::migrate_legacy_data_dir(
                    data_dir.parent().unwrap_or(&data_dir),
                    &data_dir,
                )
            };
            // 迁移只搬文件，不搬库里的绝对路径：`albums.cover_path` /
            // `photo_thumb_cache.thumb_path` 仍然写着旧标识符目录，目录一改名就全部变成死链
            // （症状：**所有相册封面碎图**，而相册名/张数一切正常，BUG-2026-1021-005）。
            // 必须同样早于数据库被应用打开。
            let path_note = devpath::repair_legacy_data_paths(
                data_dir.parent().unwrap_or(&data_dir),
                &data_dir,
            );
            // 初始化日志组件（保留 3 天 = 4320 分钟）
            logger::init(&data_dir, 4320);
            logger::log_info(&format!("数据目录迁移: {migrate_note}"));
            logger::log_info(&format!("旧路径修复: {path_note}"));
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
            /* 不 panic：启动失败时用户看到的应当是一句能行动的话，而不是调用栈。
               （目录不可写的情况已被上面的 ensure_dir_writable 拦下，走到这里
                 还剩"库被占用 / 库损坏"两类。） */
            let database = match Database::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    let msg = format!(
                        "数据库初始化失败：{e}\n库路径：{}\n\
                         排查：① 是否已有实例占用（任务管理器结束残留进程后重试）；\
                         ② 库文件是否损坏（同目录 *.bak-* 为历史备份，可替换后重试）",
                        db_path.display()
                    );
                    logger::log_error("startup", &msg);
                    eprintln!("[FATAL] {msg}");
                    std::process::exit(1);
                }
            };
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
                    // FEAT-064：扫描分组工具的异步任务状态（进程级，页面切换不丢）
                    app.manage(test_scan_job::ScanJobState::default());
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
                    // FEAT-064：扫描分组工具的异步任务状态（进程级，页面切换不丢）
                    app.manage(test_scan_job::ScanJobState::default());
                }
            }
            // FEAT-099 非破坏图像编辑引擎：编辑会话表（预览平面 + 工作区缓存）
            // 与 app 生命周期同长，打开图片时惰性填充，关闭编辑器时整体释放
            app.manage(edit::commands::EditState::new());
            // BUG-2026-1023-009 收尾：缩略图缓存键空间解耦后的**一次性迁移**
            // （重订键复用旧文件 + 回收孤儿），必须早于用户开始浏览；
            // 放后台线程跑，不阻塞窗口首帧；幂等（settings 标记控制）。
            {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    let Ok(thumbs) = thumbs_dir(&h) else {
                        logger::log_warn("取缩略图目录失败，跳过迁移");
                        return;
                    };
                    let st = h.state::<AppState>();
                    let report = {
                        let db = match st.0.lock() {
                            Ok(db) => db,
                            Err(e) => {
                                logger::log_error(
                                    "thumb_migrate",
                                    &format!("数据库锁中毒，跳过迁移: {e}"),
                                );
                                return;
                            }
                        };
                        thumb_migrate::migrate_if_needed(&db, &thumbs)
                    };
                    if report.ran {
                        logger::log_info(&format!(
                            "[thumb] 缓存迁移已执行：复用 {} 行 / 回收 {:.1} MB",
                            report.migrated_rows,
                            report.freed_bytes as f64 / (1024.0 * 1024.0)
                        ));
                    }
                });
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
            // FEAT-064：扫描分组工具异步任务（退出页面不中断 + 并行扫描）
            test_scan::commands::start_scan_job,
            test_scan::commands::start_resolve_job,
            test_scan::commands::start_organize_job,
            test_scan::commands::get_scan_job,
            test_scan::commands::cancel_scan_job,
            test_scan::commands::clear_scan_job,
            test_scan::commands::reset_scan_job,
            // FEAT-064：文件扫描并行度设置（CPU 拓扑 + 推荐线程数 + 实测校准）
            vcr_settings::get_scan_perf,
            vcr_settings::set_scan_perf,
            vcr_settings::calibrate_scan_threads,
            // P23：批次大小标定（同一批样本交错轮转实测各档 ms/张，写 batch_calibration.json）
            vcr_settings::calibrate_scan_batch,
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
            // FEAT-095：照片归档 / 隐藏标记（虚拟相册数据源）
            set_photo_marks,
            list_marked_photos,
            get_photo_mark_states,
            verify_current_password,
            move_photos_to_album,
            classify_album,
            list_persons,
            list_persons_in_album,
            list_person_photos,
            get_person_photos,
            get_person_avatar,
            set_person_avatar_from_photo,
            restore_person_avatar,
            rename_person,
            merge_persons,
            delete_person,
            rebuild_person_registry,
            content::commands::scan_album_content,
            content::commands::scan_album_combined,
            content::commands::read_album_content,
            content::commands::search_photo_content,
            content::commands::ensure_photo_scanned,
            content::commands::get_photo_content_by_path,
            // FEAT-096：单张 EXIF 后台补全（纯本地 IO，不依赖 AI 服务）
            content::commands::backfill_photo_exif,
            content::commands::search_photo_content_with_filters,
            content::commands::list_timeline,
    content::commands::timeline_index,
    content::commands::timeline_month,
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
            // FEAT-SEM：语义链路状态（分档原因，供搜索页给出准确提示与重试入口）
            content::commands::semantic_status,
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
            get_face_tier_info,
            set_face_tier,
            benchmark_vcr,
            cancel_scan,
            // 开发者视角（实时日志副窗口）
            tail_dev_log,
            open_dev_log_window,
            open_dev_data_window,
            // 创意工坊（FEAT-063）：Python 微服务生命周期 + 结果落盘
            studio::studio_ensure,
            studio::studio_save_result,
            // FEAT-099：非破坏图像编辑引擎（Lightroom 基准调色）
            edit::commands::edit_schema,
            edit::commands::edit_open,
            edit::commands::edit_close,
            edit::commands::edit_close_all,
            edit::commands::edit_render,
            edit::commands::edit_histogram,
            edit::commands::edit_autotune,
            edit::commands::edit_save_recipe,
            edit::commands::edit_load_recipe,
            edit::commands::edit_reset_recipe,
            edit::commands::edit_recipe_to_text,
            edit::commands::edit_recipe_from_text,
            edit::commands::edit_encode,
            edit::commands::edit_output_size,
            // 局部蒙版（FEAT-099 扩展）
            edit::commands::edit_mask_set,
            edit::commands::edit_mask_get,
            edit::commands::edit_mask_clear,
            edit::commands::edit_write_file,
            edit::commands::edit_session_stats,
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
