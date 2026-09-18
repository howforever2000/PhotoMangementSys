//! 照片级操作命令（自 lib.rs 迁入，lib.rs 瘦身第二期）
//! ===================================================================
//! 职责：缩略图批量/预热/覆盖率、导出、删除（记录/文件/回收站）、
//! 最近删除恢复、评分、跨相册移动、open_folder 系统打开。
//! 业务逻辑仍在 db/ 与 thumbnail/ 模块，本文件只是命令层。



/// 批量生成/复用照片网格缩略图（供前端分批懒加载）
///
/// - 输入：相册 id + 一批原图路径
/// - 输出：`[(原图路径, 缩略图缓存路径)]`，表命中 0 IO 复用，未命中现场生成 256px JPEG
/// - FEAT-044 三步模式：主流程短锁查表 → `spawn_blocking` 生成（纯文件系统）→
///   主流程短锁写表，避免 `MutexGuard` 跨线程生命周期问题
/// - 链路覆盖：
///   1. 相册管理预览（AlbumDetail/PhotoGrid）：照片未入库时靠本命令懒加载 + 写表，
///      下次同 `photo_hash` 查表直接命中，缩略图及时呈现；
///   2. 智慧相册（时间线/回忆/智能搜索）：数据源已入库且扫描预热过，未命中仅在
///      文件被修改 / 缓存丢失时出现，现场生成属自愈行为（幂等，不重生成已有项）。
#[tauri::command]
pub async fn get_photo_thumbs(
    album_id: i64,
    paths: Vec<String>,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<Vec<(String, String)>, String> {
    let _t = log_call!("get_photo_thumbs", &format!("album_id={album_id} paths={}", paths.len()));
    let user_id = crate::require_user(&session)?;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let requested = paths.len();
    let thumbs_dir = crate::thumbs_dir(&app).map_err(|e| e.to_string())?;
    use std::collections::HashMap;
    use std::sync::Arc;

    // 1. 主流程加锁查表（短锁）
    let hit_map: HashMap<String, String> = {
        let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
        let hashes: Vec<String> = paths
            .iter()
            .filter_map(|p| {
                let path = std::path::Path::new(p);
                let (len, mtime) = std::fs::metadata(path).ok().map(|md| {
                    (
                        md.len(),
                        md.modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_nanos())
                            .unwrap_or(0),
                    )
                })?;
                Some(crate::thumbnail::thumb_photo_hash(path, len, mtime))
            })
            .collect();
        match db.lookup_thumb_caches(&hashes) {
            Ok(hits) => hits
                .into_iter()
                .filter(|h| {
                    !h.thumb_path.is_empty()
                        && std::path::Path::new(&h.thumb_path).is_file()
                })
                .map(|h| (h.photo_hash, h.thumb_path))
                .collect(),
            Err(_) => HashMap::new(),
        }
    };

    let hit_from_table = hit_map.len();

    // 2. spawn_blocking：未命中项现场生成（纯文件系统，不持锁）。
    //    磁盘上已有指纹命名缩略图但表未记录时，ensure_grid_thumb 只回路径不重生成。
    #[derive(Default)]
    struct GenBuf(std::sync::Mutex<Vec<(String, String, u64, u128)>>);
    let gen_buf = Arc::new(GenBuf::default());
    let gen_buf_for_cb = gen_buf.clone();
    let hit_for_cb = hit_map;
    let paths_for_blocking = paths.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        crate::thumbnail::ensure_grid_thumbs_with_lookup(
            album_id,
            &paths_for_blocking,
            &thumbs_dir,
            &move |hashes| -> HashMap<String, String> {
                let mut out = HashMap::new();
                for h in hashes {
                    if let Some(t) = hit_for_cb.get(h) {
                        out.insert(h.clone(), t.clone());
                    }
                }
                out
            },
            move |generated| {
                if let Ok(mut g) = gen_buf_for_cb.0.lock() {
                    g.extend(generated.iter().cloned());
                }
            },
        )
    })
    .await
    .map_err(|e| format!("缩略图懒加载任务失败: {e}"))?;

    // 3. 主流程短锁写表：新生成项 upsert（photo_hash 主键幂等，下次 0 IO 命中）
    {
        let items = gen_buf.0.lock().map_err(|e| e.to_string())?;
        if !items.is_empty() {
            let db = state.0.lock().map_err(|e| e.to_string())?;
            let recs: Vec<crate::db::ThumbCacheRecord> = items
                .iter()
                .map(|(src, thumb, len, mtime)| {
                    let path = std::path::Path::new(src.as_str());
                    let hash = crate::thumbnail::thumb_photo_hash(path, *len, *mtime);
                    crate::db::ThumbCacheRecord {
                        photo_hash: hash,
                        source_path: src.clone(),
                        thumb_path: thumb.clone(),
                        album_id: Some(album_id),
                        user_id,
                        size_bytes: *len,
                        mtime_ns: *mtime,
                    }
                })
                .collect();
            if let Err(e) = db.upsert_thumb_caches(&recs) {
                crate::logger::log_error("thumb_cache", &format!("lazy-load upsert failed: {e:?}"));
            }
        }
    }

    // 统计：成功返回数 - 表命中数 = 本次新生成数
    let generated = res.len().saturating_sub(hit_from_table);
    crate::logger::log_call_end_with(
        "get_photo_thumbs",
        _t,
        &format!("OK | hit={hit_from_table} generated={generated} requested={requested}"),
    );
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
pub async fn export_photos(
    paths: Vec<String>,
    dest_dir: String,
    export_info: bool,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<ExportOutcome, String> {
    let _t = log_call!("export_photos", &format!("n={} dest={dest_dir}", paths.len()));
    crate::require_user(&session)?;
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

    crate::logger::log_call_end_with("export_photos", _t, &format!("OK | copied={copied} failed={failed} skipped={skipped}"));
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
pub fn delete_photo_records(
    album_id: i64,
    paths: Vec<String>,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<PhotoDeleteOutcome, String> {
    let _t = log_call!("delete_photo_records", &format!("album_id={album_id} paths={}", paths.len()));
    let user_id = crate::require_user(&session)?;
    let requested = paths.len();
    let outcome = if paths.is_empty() {
        PhotoDeleteOutcome { requested, deleted: 0, failed: 0, failed_paths: Vec::new() }
    } else {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        // 归属校验在 exclude_album_photos 内（非本人相册 → NotFound）
        let excluded = db.exclude_album_photos(album_id, user_id, &paths).map_err(|e| e.to_string())?;
        let removed = db.delete_content_by_paths(&paths).map_err(|e| e.to_string())?;
        crate::logger::log_call_end_with("delete_photo_records", _t,
            &format!("OK | excluded={excluded} scan_removed={removed}"));
        PhotoDeleteOutcome { requested, deleted: excluded, failed: 0, failed_paths: Vec::new() }
    };
    Ok(outcome)
}


/// 批量「本地文件删除」：删除磁盘照片文件，并级联清理扫描记录、排除表与网格缩略图缓存
///
/// 危险操作：文件不可恢复，前端必须二次确认后才调用。
#[tauri::command]
pub fn delete_photo_files(
    album_id: i64,
    paths: Vec<String>,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
    app: tauri::AppHandle,
) -> Result<PhotoDeleteOutcome, String> {
    let _t = log_call!("delete_photo_files", &format!("album_id={album_id} paths={}", paths.len()));
    let user_id = crate::require_user(&session)?;
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
        .map(|p| crate::thumbnail::grid_thumb_cache_name(album_id, std::path::Path::new(p)))
        .collect();
    // 2. 删磁盘文件，逐张统计成败
    let mut deleted = 0usize;
    let mut failed_paths = Vec::new();
    for p in &paths {
        match std::fs::remove_file(p) {
            Ok(_) => deleted += 1,
            Err(e) => {
                crate::logger::log_info(&format!("[delete_photo_files] 删除失败 path={p} err={e}"));
                failed_paths.push(p.clone());
            }
        }
    }
    // 3. 级联清理：缩略图缓存 + 成功删除文件的扫描记录与排除表（失败项保留原状可重试）
    if let Ok(thumbs) = crate::thumbs_dir(&app) {
        crate::thumbnail::remove_grid_thumb_files(&thumb_names, &thumbs);
    }
    if deleted > 0 {
        let ok_paths: Vec<String> = paths.iter().filter(|p| !failed_paths.contains(p)).cloned().collect();
        let db = state.0.lock().map_err(|e| e.to_string())?;
        let _ = db.exclude_album_photos(album_id, user_id, &ok_paths);
        let _ = db.delete_content_by_paths(&ok_paths);
    }
    let failed = failed_paths.len();
    let outcome = PhotoDeleteOutcome { requested, deleted, failed, failed_paths };
    crate::logger::log_call_end_with("delete_photo_files", _t,
        &format!("OK | deleted={deleted} failed={failed}"));
    Ok(outcome)
}


/// FEAT-050：按原图路径级联清理缩略图缓存（表行 + 磁盘文件）
///
/// 磁盘文件覆盖 flat JPG / WebP 两套命名；photo_thumb_cache 表内 thumb_path
/// 指向的文件也一并删除。注意：文件名指纹依赖原图可读，
/// 必须在原图被删除 / 移入回收站**之前**调用。
pub fn cleanup_thumb_caches_for_paths(db: &crate::db::Database, app: &tauri::AppHandle, paths: &[String]) {
    // 1. photo_thumb_cache 表行 + 表内记录的缩略图文件
    let table_thumb_files = db.list_thumb_paths_by_sources(paths).unwrap_or_default();
    let _ = db.delete_thumb_caches_by_paths(paths);
    // 2. 网格缩略图磁盘文件（归属未知时用 album_0 无归属命名空间）
    let album_map = db.album_ids_by_paths(paths).unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for p in paths {
        let album_id = album_map.get(p).copied().flatten().unwrap_or(0);
        names.extend(crate::thumbnail::grid_thumb_cache_names_all(
            album_id,
            std::path::Path::new(p),
        ));
    }
    if let Ok(thumbs) = crate::thumbs_dir(app) {
        crate::thumbnail::remove_grid_thumb_files(&names, &thumbs);
        for f in &table_thumb_files {
            let _ = std::fs::remove_file(f);
        }
    }
}


/// FEAT-050：批量「磁盘删除（回收站）」：移入系统回收站 + 级联清扫描记录与缩略图缓存
///
/// 与 delete_photo_files（永久删除不可恢复）的区别：文件可在回收站找回。
/// 前端必须二次确认后才调用。无需相册 id（分类/地点视图照片可能无归属）。
#[tauri::command]
pub fn delete_photos_to_trash(
    paths: Vec<String>,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
    app: tauri::AppHandle,
) -> Result<PhotoDeleteOutcome, String> {
    let _t = log_call!("delete_photos_to_trash", &format!("paths={}", paths.len()));
    let _user_id = crate::require_user(&session)?;
    let requested = paths.len();
    if paths.is_empty() {
        return Ok(PhotoDeleteOutcome { requested, deleted: 0, failed: 0, failed_paths: Vec::new() });
    }
    // 1. 原图仍可读 → 先清理缩略图缓存（指纹依赖文件存在）
    {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        crate::photos::cleanup_thumb_caches_for_paths(&db, &app, &paths);
    }
    // 2. 逐张移入系统回收站
    let mut deleted = 0usize;
    let mut failed_paths = Vec::new();
    for p in &paths {
        match trash::delete(std::path::Path::new(p)) {
            Ok(_) => deleted += 1,
            Err(e) => {
                crate::logger::log_info(&format!("[delete_photos_to_trash] 回收站删除失败 path={p} err={e}"));
                failed_paths.push(p.clone());
            }
        }
    }
    // 3. 级联清扫描记录（仅成功项；失败项保留原状可重试）
    let failed = failed_paths.len();
    if deleted > 0 {
        let ok_paths: Vec<String> = paths.iter().filter(|p| !failed_paths.contains(p)).cloned().collect();
        let db = state.0.lock().map_err(|e| e.to_string())?;
        let _ = db.delete_content_by_paths(&ok_paths);
    }
    let outcome = PhotoDeleteOutcome { requested, deleted, failed, failed_paths };
    crate::logger::log_call_end_with(
        "delete_photos_to_trash",
        _t,
        &format!("OK | deleted={deleted} failed={failed}"),
    );
    Ok(outcome)
}


/// FEAT-050：批量「本地记录删除」（无相册版）：清扫描记录 + 缩略图缓存，本地文件保留
///
/// 分类/地点等跨相册视图使用（delete_photo_records 需相册 id 且写排除表，
/// 不适用无归属照片）。前端必须二次确认后才调用。
#[tauri::command]
pub fn delete_photo_records_by_paths(
    paths: Vec<String>,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
    app: tauri::AppHandle,
) -> Result<PhotoDeleteOutcome, String> {
    let _t = log_call!("delete_photo_records_by_paths", &format!("paths={}", paths.len()));
    let _user_id = crate::require_user(&session)?;
    let requested = paths.len();
    if paths.is_empty() {
        return Ok(PhotoDeleteOutcome { requested, deleted: 0, failed: 0, failed_paths: Vec::new() });
    }
    let db = state.0.lock().map_err(|e| e.to_string())?;
    crate::photos::cleanup_thumb_caches_for_paths(&db, &app, &paths);
    let deleted = db.delete_content_by_paths(&paths).map_err(|e| e.to_string())?;
    crate::logger::log_call_end_with(
        "delete_photo_records_by_paths",
        _t,
        &format!("OK | scan_removed={deleted}"),
    );
    Ok(PhotoDeleteOutcome { requested, deleted, failed: 0, failed_paths: Vec::new() })
}


/// 恢复已「记录删除」的照片（撤销删除）：从 album_photo_excluded 移除对应条目
#[tauri::command]
pub fn restore_photo_records(
    album_id: i64,
    paths: Vec<String>,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<usize, String> {
    let _t = log_call!("restore_photo_records", &format!("album_id={album_id} paths={}", paths.len()));
    let user_id = crate::require_user(&session)?;
    if paths.is_empty() {
        return Ok(0);
    }
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let restored = db.restore_excluded_photos(album_id, user_id, &paths).map_err(|e| e.to_string())?;
    crate::logger::log_call_end_with("restore_photo_records", _t,
        &format!("OK | restored={restored}"));
    Ok(restored)
}


/// 获取最近删除记录（用户可在此列表中恢复）
#[tauri::command]
pub fn list_recently_deleted(
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<Vec<RecentlyExcludedItem>, String> {
    let user_id = crate::require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.list_recently_excluded(user_id, 200).map_err(|e| e.to_string())
}


/// 清空所有最近删除记录
#[tauri::command]
pub fn clear_recently_deleted(
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<usize, String> {
    let user_id = crate::require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.clear_all_excluded(user_id).map_err(|e| e.to_string())
}


/// 给一批照片打分（rating 0-5，0 清除）。按 (user_id, path) upsert，无需扫描记录即可打分。
#[tauri::command]
pub fn set_photo_rating(
    paths: Vec<String>,
    rating: i64,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<(), String> {
    let _t = log_call!("set_photo_rating", &format!("paths={} rating={rating}", paths.len()));
    let user_id = crate::require_user(&session)?;
    let rating = rating.clamp(0, 5);
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.set_photo_rating(user_id, &paths, rating)
        .map_err(|e| e.to_string())?;
    crate::logger::log_call_end_with("set_photo_rating", _t, &format!("OK | n={}", paths.len()));
    Ok(())
}


/// 查询一批照片的打分，返回 [(path, rating)]（未打分的不会出现）。
#[tauri::command]
pub fn get_photo_ratings(
    paths: Vec<String>,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<Vec<(String, i64)>, String> {
    let _t = log_call!("get_photo_ratings", &format!("paths={}", paths.len()));
    let user_id = crate::require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let r = db.get_photo_ratings(user_id, &paths).map_err(|e| e.to_string())?;
    crate::logger::log_call_end_with("get_photo_ratings", _t, &format!("OK | n={}", r.len()));
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
pub fn move_photos_to_album(
    album_id: i64,
    paths: Vec<String>,
    target_album_id: i64,
    app: tauri::AppHandle,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<PhotoMoveOutcome, String> {
    let _t = log_call!("move_photos_to_album", &format!("album_id={album_id} target={target_album_id} paths={}", paths.len()));
    let user_id = crate::require_user(&session)?;
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
    if let Ok(thumbs) = crate::thumbs_dir(&app) {
        let names: Vec<String> = paths
            .iter()
            .filter(|p| !failed_paths.contains(p))
            .map(|p| crate::thumbnail::grid_thumb_cache_name(album_id, std::path::Path::new(p)))
            .collect();
        crate::thumbnail::remove_grid_thumb_files(&names, &thumbs);
    }
    let failed = failed_paths.len();
    let out = PhotoMoveOutcome { requested, moved, failed, failed_paths, target_id: target_album_id };
    crate::logger::log_call_end_with("move_photos_to_album", _t, &format!("OK | moved={moved} failed={failed}"));
    Ok(out)
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
pub async fn prewarm_thumbs(
    album_id: i64,
    paths: Vec<String>,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<PrewarmOutcome, String> {
    let _t = log_call!("prewarm_thumbs", &format!("album_id={album_id} paths={}", paths.len()));
    let user_id = crate::require_user(&session)?;
    if paths.is_empty() {
        return Ok(PrewarmOutcome { requested: 0, hit: 0, generated: 0, failed: 0 });
    }
    let thumbs_dir = crate::thumbs_dir(&app).map_err(|e| e.to_string())?;
    // FEAT-044：走表命中 → hit；未命中走文件系统生成 → generated
    //
    // 拆为查（短锁）→ 生成（纯文件系统）→ 写表（短锁）三步
    // 避免 MutexGuard 跨 spawn_blocking 边界。
    use std::collections::HashMap;
    use std::sync::Arc;

    let hit_map_outer: Arc<HashMap<String, String>> = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        let hashes: Vec<String> = paths
            .iter()
            .filter_map(|p| {
                let path = std::path::Path::new(p);
                let (len, mtime) = std::fs::metadata(path).ok().map(|md| {
                    (
                        md.len(),
                        md.modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_nanos())
                            .unwrap_or(0),
                    )
                })?;
                Some(crate::thumbnail::thumb_photo_hash(path, len, mtime))
            })
            .collect();
        match db.lookup_thumb_caches(&hashes) {
            Ok(hits) => Arc::new(
                hits.into_iter()
                    .filter(|h| {
                        !h.thumb_path.is_empty()
                            && std::path::Path::new(&h.thumb_path).is_file()
                    })
                    .map(|h| (h.photo_hash, h.thumb_path))
                    .collect(),
            ),
            Err(_) => Arc::new(HashMap::new()),
        }
    };

    // 表命中原图 path（避免在生成项中重复）
    let _hit_source_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    // **不依赖 source_path 标记**：表命中且文件存在 → hit；生成返回 OK → generated。
    let hit_from_table = hit_map_outer.len();

    #[derive(Default)]
    struct GenBuf(std::sync::Mutex<Vec<(String, String, u64, u128)>>);
    let gen_buf = Arc::new(GenBuf::default());
    let gen_buf_for_cb = gen_buf.clone();
    let hit_for_cb = hit_map_outer.clone();
    let paths_for_blocking = paths.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        crate::thumbnail::ensure_grid_thumbs_with_lookup(
            album_id,
            &paths_for_blocking,
            &thumbs_dir,
            &move |hashes| -> HashMap<String, String> {
                let mut out = HashMap::new();
                for h in hashes {
                    if let Some(t) = hit_for_cb.get(h) {
                        out.insert(h.clone(), t.clone());
                    }
                }
                out
            },
            move |generated| {
                if let Ok(mut g) = gen_buf_for_cb.0.lock() {
                    g.extend(generated.iter().cloned());
                }
            },
        )
    })
    .await
    .map_err(|e| format!("缩略图预热任务失败: {e}"))?;

    // 写表（新生成项）
    {
        let items = gen_buf.0.lock().map_err(|e| e.to_string())?;
        if !items.is_empty() {
            let db = state.0.lock().map_err(|e| e.to_string())?;
            let recs: Vec<crate::db::ThumbCacheRecord> = items
                .iter()
                .map(|(src, thumb, len, mtime)| {
                    let path = std::path::Path::new(src.as_str());
                    let hash = crate::thumbnail::thumb_photo_hash(path, *len, *mtime);
                    crate::db::ThumbCacheRecord {
                        photo_hash: hash,
                        source_path: src.clone(),
                        thumb_path: thumb.clone(),
                        album_id: Some(album_id),
                        user_id,
                        size_bytes: *len,
                        mtime_ns: *mtime,
                    }
                })
                .collect();
            if let Err(e) = db.upsert_thumb_caches(&recs) {
                crate::logger::log_error("thumb_cache", &format!("prewarm upsert failed: {e:?}"));
            }
        }
    }

    // 统计：成功的 (源 → 缩略图) 数 - 表命中数 = 本次生成数；未成功 = failed
    let generated = res.len().saturating_sub(hit_from_table);
    let failed = paths.len().saturating_sub(res.len());
    let out = PrewarmOutcome {
        requested: paths.len(),
        hit: hit_from_table, // 表命中且文件存在的项数
        generated,
        failed,
    };
    crate::logger::log_call_end_with(
        "prewarm_thumbs",
        _t,
        &format!(
            "OK | hit={} generated={generated} failed={failed}",
            out.hit
        ),
    );
    Ok(out)
}


/// FEAT-044（补充）：缩略图缓存覆盖率统计
///
/// 返回当前用户的：
/// - `cached`: `photo_thumb_cache` 中行数
/// - `total_scanned`: `photo_content_scan` 中行数（已入库总数）
/// - `unthumbered`: 精确统计「已入库但无缩略图缓存行」（LEFT JOIN 按 photo_hash 对齐）
///
/// 前端场景：
/// - 智慧相册首屏：拿 `unthumbered` 判断是否提示「N 张照片未扫描入库」。
/// - 个人中心/性能面板：拿 cached / total_scanned 算覆盖率。
/// - **`unthumbered` 为 JOIN 精确口径**：懒加载（`get_photo_thumbs` 未命中现场生成）
///   也会写 `photo_thumb_cache`，未入库照片同样占行，旧口径
///   `total_scanned - cached` 已失真，不再使用。预热进行中会有中间态，
///   不要用于限制调取流程。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ThumbCoverage {
    pub cached: i64,
    pub total_scanned: i64,
    pub unthumbered: i64,
}


#[tauri::command]
pub fn get_photo_thumbs_count(
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<ThumbCoverage, String> {
    let _t = log_call!("get_photo_thumbs_count", "");
    let user_id = crate::require_user(&session)?;
    let r = (|| -> Result<ThumbCoverage, String> {
        let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
        let cached = db
            .count_thumb_caches(user_id)
            .map_err(|e| format!("{:?}", e))?;
        // photo_content_scan 中的行数（已入库总数）。
        let total_scanned: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM photo_content_scan WHERE user_id = ?1",
                rusqlite::params![user_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("{:?}", e))?;
        // FEAT-044（I4）：LEFT JOIN 精确统计「已入库但无缩略图缓存行」。
        // 懒加载写表后未入库照片也占 photo_thumb_cache 行，
        // 旧口径 total_scanned - cached 会失真，已废弃。
        let unthumbered = db
            .count_scanned_without_thumb(user_id)
            .map_err(|e| format!("{:?}", e))?;
        Ok(ThumbCoverage {
            cached,
            total_scanned,
            unthumbered,
        })
    })();
    if let Ok(ref c) = r {
        crate::logger::log_call_end_with(
            "get_photo_thumbs_count",
            _t,
            &format!("OK | cached={} total={} unthumbered={}", c.cached, c.total_scanned, c.unthumbered),
        );
    } else if let Err(ref e) = r {
        crate::logger::log_call_end_with("get_photo_thumbs_count", _t, &format!("ERR | {e}"));
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
pub fn open_folder(path: String) -> Result<(), String> {
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
