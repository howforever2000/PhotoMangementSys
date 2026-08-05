mod db;
mod folder;
mod logger;
mod thumbnail;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use db::{CreateAlbumInput, Database, UpdateAlbumInput};
use tauri::{Emitter, Manager};

/// 缩略图缓存目录名（位于 app_data_dir 下）
const THUMBS_DIR: &str = "thumbs";

/// AOP 日志宏：在命令/函数入口记录调用开始，返回计时器
///
/// 用法（放在函数第一行）：
/// ```rust
/// let _t = log_call!("create_album", "input=...");
/// ```
macro_rules! log_call {
    ($name:expr) => {
        logger::log_call_start($name, "")
    };
    ($name:expr, $desc:expr) => {
        logger::log_call_start($name, $desc)
    };
}

/// 全局应用状态：封装数据库连接
///
/// `rusqlite::Connection` 本身非 `Sync`，需用 `Mutex` 包裹后才能满足
/// `tauri::State` 的 `Send + Sync` 要求，供多个 `#[tauri::command]` 共享。
/// 对应 SpringBoot 中被 `@Autowired` 注入的单例 `DataSource` / `Service`。
pub struct AppState(pub Mutex<Database>);

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
#[tauri::command]
fn create_album(
    input: CreateAlbumInput,
    state: tauri::State<AppState>,
) -> Result<db::Album, String> {
    let _t = log_call!("create_album", &format!("name={}, path={}", input.name, input.path));
    validate_create(&input)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let r = db.create_album(input).map_err(|e| e.to_string());
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

/// 填充相册的统计属性（照片数量、文件夹大小、拍摄时间、默认封面）
///
/// - `photo_count`: 扫描文件夹统计图片数量
/// - `size_bytes`: 文件夹真实占用空间（递归遍历累加所有文件大小）
/// - `shoot_time`: 相册内图片的 EXIF 拍摄时间（YYYY-MM-DD）
/// - `cover_path`: 若没有封面，自动用文件夹内第一张图片的缩略图作为封面
///
/// 拍摄时间从"相册内第一张原图"读取（缩略图是生成的，不含 EXIF）。
/// 这些都是返回给前端的计算属性，不写回数据库（保留用户手动设置封面的能力）。
fn fill_album_stats(
    album: &mut db::Album,
    thumbs_dir: &Path,
) {
    let dir = std::path::Path::new(&album.path);

    // 统计照片数量
    album.photo_count = thumbnail::count_images(dir) as i64;

    // 统计文件夹总大小（真实占用空间，递归累加所有文件）
    album.size_bytes = thumbnail::folder_size(dir);

    // 无封面时自动用第一张图片的缩略图作为封面
    let mut source_path = None;
    if album.cover_path.is_none() {
        if let Ok(res) = thumbnail::ensure_thumbnail(album.id, dir, thumbs_dir) {
            source_path = Some(res.source_path);
            album.cover_path = Some(res.thumb_path);
        }
    }

    // 拍摄时间：从相册内第一张原图读取 EXIF（缩略图是生成的，不含 EXIF）
    // 优先用自动封面时的原图；若已有手动封面，则重新扫描找一张图读
    let exif_source = source_path
        .map(std::path::PathBuf::from)
        .or_else(|| thumbnail::find_first_image(dir).ok());
    if let Some(img) = exif_source {
        album.shoot_time = thumbnail::read_shoot_time(&img);
    }
}

/// 获取相册列表（需求 §4.2 get_albums，按 updated_at 降序）
///
/// 返回时为无封面的相册自动补第一张图缩略图
#[tauri::command]
fn get_albums(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<Vec<db::Album>, String> {
    let _t = log_call!("get_albums");
    let thumbs = thumbs_dir(&app)?;
        let mut albums = {
            let db = state.0.lock().map_err(|e| e.to_string())?;
            db.get_albums().map_err(|e| e.to_string())?
        };
    for a in albums.iter_mut() {
        fill_album_stats(a, &thumbs);
    }
    // 兜底：如果 fill_album_folder 未正确设置 folder_id，从 DB 再查
    {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        for album in albums.iter_mut() {
            if album.folder_id.is_none() {
                if let Ok(Some(fid)) = db.conn().query_row(
                    "SELECT folder_id FROM albums WHERE id = ?1",
                    rusqlite::params![album.id],
                    |r| r.get::<_, Option<i64>>(0),
                ) {
                    album.folder_id = Some(fid);
                }
                if album.folder_id.is_none() {
                    if let Ok(fid) = db.conn().query_row(
                        "SELECT folder_id FROM folder_albums WHERE album_id = ?1 LIMIT 1",
                        rusqlite::params![album.id],
                        |r| r.get::<_, i64>(0),
                    ) {
                        album.folder_id = Some(fid);
                    }
                }
            }
        }
    }
    logger::log_call_end_with("get_albums", _t, &format!("OK | count={}", albums.len()));
    Ok(albums)
}

/// 获取单个相册详情（需求 §4.2 get_album）
#[tauri::command]
fn get_album(
    id: i64,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<db::Album, String> {
    eprintln!("[BACKEND] get_album CALLED: id={}", id);
    let thumbs = thumbs_dir(&app)?;
    let mut album = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(id).map_err(|e| e.to_string())?
    };
    eprintln!("[BACKEND] get_album AFTER DB: id={}, folder_id={:?}", id, album.folder_id);
    // 兜底：如果 fill_album_folder 未正确设置 folder_id，再从 DB 查一次
    if album.folder_id.is_none() {
        eprintln!("[BACKEND] get_album FALLBACK: id={}, folder_id is NONE, querying DB directly", id);
        let db = state.0.lock().map_err(|e| e.to_string())?;
        if let Ok(Some(fid)) = db.conn().query_row(
            "SELECT folder_id FROM albums WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get::<_, Option<i64>>(0),
        ) {
            album.folder_id = Some(fid);
            eprintln!("[BACKEND] get_album FALLBACK albums.folder_id: id={}, fid={}", id, fid);
        }
        // 如果 folder_albums 也有值，用它补充
        if album.folder_id.is_none() {
            if let Ok(fid) = db.conn().query_row(
                "SELECT folder_id FROM folder_albums WHERE album_id = ?1 LIMIT 1",
                rusqlite::params![id],
                |r| r.get::<_, i64>(0),
            ) {
                album.folder_id = Some(fid);
                eprintln!("[BACKEND] get_album FALLBACK folder_albums: id={}, fid={}", id, fid);
            }
        }
    }
    fill_album_stats(&mut album, &thumbs);
    eprintln!("[BACKEND] get_album RETURNING: id={}, folder_id={:?}", id, album.folder_id);
    Ok(album)
}

/// 更新相册信息（需求 §4.2 update_album）
#[tauri::command]
fn update_album(
    input: UpdateAlbumInput,
    state: tauri::State<AppState>,
) -> Result<(), String> {
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
    db.update_album(input).map_err(|e| e.to_string())
}

/// 设置相册标签（覆盖式，最多 5 个）
#[tauri::command]
fn update_album_tags(
    album_id: i64,
    tags: Vec<String>,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.update_album_tags(album_id, tags).map_err(|e| e.to_string())
}

/// 删除相册（需求 §4.2 delete_album，仅删记录不删本地文件）
#[tauri::command]
fn delete_album(id: i64, state: tauri::State<AppState>) -> Result<(), String> {
    let _t = log_call!("delete_album", &format!("id={id}"));
    let r = (|| -> Result<(), String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.delete_album(id).map_err(|e| e.to_string())
    })();
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
/// 返回实际删除数量。
#[tauri::command]
fn delete_albums(ids: Vec<i64>, state: tauri::State<AppState>) -> Result<usize, String> {
    let _t = log_call!("delete_albums", &format!("ids={ids:?}"));
    let r = (|| -> Result<usize, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.delete_albums(&ids).map_err(|e| e.to_string())
    })();
    match &r {
        Ok(n) => logger::log_call_end_with("delete_albums", _t, &format!("OK | deleted={n}")),
        Err(e) => logger::log_call_end_with("delete_albums", _t, &format!("ERR | {e}")),
    }
    r
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
) -> Result<db::Album, String> {
    // 校验图片路径存在
    let img = std::path::Path::new(&image_path);
    if !img.is_file() {
        return Err(format!("图片不存在: {image_path}"));
    }

    let thumbs = thumbs_dir(&app)?;

    // 生成封面缩略图（统一存到缓存目录）
    let cover = thumbnail::generate_cover(id, img, &thumbs)
        .map_err(|e| format!("无法生成封面缩略图: {e}"))?;

    // 更新数据库 cover_path
    {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.update_album(db::UpdateAlbumInput {
            id,
            name: None,
            description: None,
            cover_path: Some(cover),
            location: None,
        })
        .map_err(|e| e.to_string())?;
    }

    // 返回更新后的相册（含封面）
    let mut album = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.get_album(id).map_err(|e| e.to_string())?
    };
    // 此时 cover_path 已设置，fill_album_stats 不会覆盖它
    fill_album_stats(&mut album, &thumbs);
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
#[tauri::command]
fn import_albums(
    root_path: String,
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<ImportResult, String> {
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

        // 检查是否已作为相册存在，存在则跳过
        if let Ok(Some(_)) = db.find_album_by_path(&path_str) {
            result.skipped += 1;
        } else {
            // 创建相册，名称用子文件夹名
            let created = db.create_album(db::CreateAlbumInput {
                name: folder_name.clone(),
                path: path_str,
                description: None,
            });
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
#[tauri::command]
fn create_folder(
    name: String,
    parent_id: Option<i64>,
    state: tauri::State<AppState>,
) -> Result<folder::Folder, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("分组名称不能为空".into());
    }
    let db = state.0.lock().map_err(|e| e.to_string())?;
    folder::create_folder(db.conn(), &name, parent_id).map_err(|e| e.to_string())
}

/// 更新分组（名称/说明/标签，标签最多 5 个）
#[tauri::command]
fn update_folder(
    id: i64,
    name: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
    state: tauri::State<AppState>,
) -> Result<folder::Folder, String> {
    let db = state.0.lock().map_err(|e| e.to_string())?;
    folder::update_folder(
        db.conn(),
        id,
        name.as_deref(),
        description.as_deref(),
        tags,
    )
    .map_err(|e| e.to_string())
}

/// 删除分组
#[tauri::command]
fn delete_folder(id: i64, state: tauri::State<AppState>) -> Result<(), String> {
    let db = state.0.lock().map_err(|e| e.to_string())?;
    folder::delete_folder(db.conn(), id).map_err(|e| e.to_string())
}

/// 获取手动排序结构
#[tauri::command]
fn get_manual_tree(state: tauri::State<AppState>) -> Result<folder::ManualTree, String> {
    let _t = log_call!("get_manual_tree");
    let r = (|| -> Result<folder::ManualTree, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        folder::get_manual_tree(db.conn()).map_err(|e| e.to_string())
    })();
    match &r {
        Ok(t) => logger::log_call_end_with("get_manual_tree", _t, &format!("OK | folders={}", t.folders.len())),
        Err(e) => logger::log_call_end_with("get_manual_tree", _t, &format!("ERR | {e}")),
    }
    r
}

/// 按名称模糊搜索相册（含分组归属路径）
#[tauri::command]
fn search_albums(
    keyword: String,
    state: tauri::State<AppState>,
) -> Result<Vec<db::AlbumSearchResult>, String> {
    let _t = log_call!("search_albums", &format!("keyword={keyword}"));
    let r = (|| -> Result<Vec<db::AlbumSearchResult>, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.search_albums(&keyword).map_err(|e| e.to_string())
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
#[tauri::command]
fn move_album(
    album_id: i64,
    folder_id: Option<i64>,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let _start = logger::log_call_start("move_album", &format!("album_id={album_id}, folder_id={folder_id:?}"));
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let conn = db.conn();

    // 校验文件夹存在
    if let Some(fid) = folder_id {
        let exists: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM folders WHERE id = ?1", rusqlite::params![fid], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err("目标分组不存在".into());
        }
    }

    // 新位置排序（组内末尾）
    let sort_order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM albums
             WHERE folder_id = ?1 OR (folder_id IS NULL AND ?1 IS NULL)",
            rusqlite::params![folder_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // 事务内更新，确保 folder_id 持久化
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    // 1. 更新 albums.folder_id（相册记父节点）
    tx.execute(
        "UPDATE albums SET folder_id = ?1, sort_order = ?2 WHERE id = ?3",
        rusqlite::params![folder_id, sort_order, album_id],
    )
    .map_err(|e| e.to_string())?;
    // 2. 更新 folder_albums 关联表（folders 记录子相册列表）
    //    删除该相册的旧关联
    tx.execute(
        "DELETE FROM folder_albums WHERE album_id = ?1",
        rusqlite::params![album_id],
    )
    .map_err(|e| e.to_string())?;
    //    若移入分组，插入新关联
    if let Some(fid) = folder_id {
        tx.execute(
            "INSERT OR REPLACE INTO folder_albums (folder_id, album_id, sort_order) VALUES (?1, ?2, ?3)",
            rusqlite::params![fid, album_id, sort_order],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    let verify: Option<i64> = conn.query_row(
        "SELECT folder_id FROM albums WHERE id = ?1", rusqlite::params![album_id], |r| r.get(0)).unwrap_or(None);
    logger::log_call_end_with("move_album", _start, &format!("folder_id_in_db={verify:?}"));
    Ok(())
}

/// 相册组内排序
///
/// 将相册移到同一分组内的新位置。`new_index` 为该相册在目标分组（同一分组）中的新下标。
/// 若 `folder_id` 提供且与相册当前分组不同，则先移动到该分组再插入指定位置。
#[tauri::command]
fn reorder_album(
    album_id: i64,
    folder_id: Option<i64>,
    new_index: i64,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    eprintln!("[BACKEND] reorder_album CALLED: album_id={}, folder_id={:?}, new_index={}", album_id, folder_id, new_index);
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let conn = db.conn();

    // 校验文件夹存在
    if let Some(fid) = folder_id {
        let exists: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM folders WHERE id = ?1", rusqlite::params![fid], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err("目标分组不存在".into());
        }
    }

    // 事务：先移到目标分组，再在组内排序
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;

    // 移到目标分组（先放末尾）
    tx.execute(
        "UPDATE albums SET folder_id = ?1, sort_order = 999999 WHERE id = ?2",
        rusqlite::params![folder_id, album_id],
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

    // 目标分组所有相册按当前顺序取出
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

    // 重写 albums.sort_order 和 folder_albums.sort_order
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
    eprintln!("[BACKEND] reorder_album COMMITTED: album_id={}, folder_id={:?}", album_id, folder_id);
    let verify: Option<i64> = conn.query_row(
        "SELECT folder_id FROM albums WHERE id = ?1", rusqlite::params![album_id], |r| r.get(0)).unwrap_or(None);
    eprintln!("[BACKEND] reorder_album VERIFY: album_id={}, folder_id_in_db={:?}", album_id, verify);
    Ok(())
}

/// 调整分组在兄弟中的顺序
#[tauri::command]
fn reorder_folder(folder_id: i64, new_index: i64, state: tauri::State<AppState>) -> Result<(), String> {
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let conn = db.conn();
    let folder = folder::get_folder(conn, folder_id).map_err(|e| e.to_string())?;
    // 兄弟分组排序
    let mut siblings: Vec<(i64, i64)> = {
        let mut stmt = conn
            .prepare("SELECT id, sort_order FROM folders WHERE parent_id IS ?1 ORDER BY sort_order")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![folder.parent_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
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
            // 注册为全局状态，后续命令通过 tauri::State<AppState> 注入使用
            app.manage(AppState(Mutex::new(database)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            create_album,
            get_albums,
            get_album,
            update_album,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
