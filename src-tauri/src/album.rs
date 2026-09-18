//! 相册级操作命令（自 lib.rs 迁入，lib.rs 瘦身第三期）
//! ===================================================================
//! 职责：相册 CRUD/重命名/标签/批量操作/合并/删除/导入/搜索/移动/排序/
//! 封面设置/地点自动识别，以及统计回填助手。
//! 业务逻辑仍在 db/ 模块，本文件只是命令层。

use tauri::Emitter;
use crate::db::{CreateAlbumInput, UpdateAlbumInput};
use std::path::Path;


/// 输入参数长度校验
///
/// 对应 SpringBoot `@Valid` + Bean Validation，在命令层统一校验
pub fn validate_create(input: &CreateAlbumInput) -> Result<(), String> {
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
pub fn create_album(
    input: CreateAlbumInput,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<crate::db::Album, String> {
    let _t = log_call!("create_album", &format!("name={}, path={}", input.name, input.path));
    let user_id = crate::require_user(&session)?;
    validate_create(&input)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let r = db.create_album(input, user_id).map_err(|e| e.to_string());
    if let Ok(album) = &r {
        crate::logger::log_call_end_with("create_album", _t, &format!("created id={}", album.id));
    } else {
        crate::logger::log_call_end_with("create_album", _t, "FAILED");
    }
    r
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
pub fn fill_album_stats(album: &mut crate::db::Album, thumbs_dir: &Path, state: &tauri::State<crate::AppState>) {
    let dir = std::path::Path::new(&album.path);

    // 变更探测：递归文件总数（轻量，只数不读，每相册几 ms）
    let file_count = crate::thumbnail::count_files_recursive(dir);

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
    let scan = crate::thumbnail::scan_album_dir(dir);
    album.photo_count = scan.photo_count as i64;
    album.size_bytes = scan.size_bytes;

    // 无封面时自动用第一张图片的缩略图作为封面，并持久化到 SQL（albums.cover_path）
    if album.cover_path.is_none() {
        if let Some(src) = &scan.first_image {
            if let Ok(res) = crate::thumbnail::ensure_thumbnail_from_source(album.id, src, thumbs_dir) {
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
        .and_then(|p| crate::thumbnail::read_shoot_time(p));

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


/// FEAT-036：批量填充每个相册的「已入库照片数」。
/// BUG-2026-0909-001：改为按相册目录子树前缀统计（原按 album_id 分组统计，
/// 父子相册共享照片时行归属互抢，表现为「之前入库的照片变未入库」）。
/// 一次全量路径查询 + Rust 侧前缀匹配，避免逐相册 LIKE N+1。
/// 多用户隔离：`user_id` 由调用方传入，仅统计当前用户已入库行。
pub fn fill_scanned_counts(albums: &mut [crate::db::Album], user_id: i64, state: &tauri::State<crate::AppState>) {
    let Ok(db) = state.0.lock() else { return };
    let pairs: Vec<(i64, String)> = albums.iter().map(|a| (a.id, a.path.clone())).collect();
    let Ok(map) = db.count_scanned_by_prefix(user_id, &pairs) else { return };
    for a in albums.iter_mut() {
        a.scanned_photo_count = map.get(&a.id).copied().unwrap_or(0);
    }
}


/// 获取相册列表（需求 §4.2 get_albums，按 updated_at 降序）
///
/// 返回时为无封面的相册自动补第一张图缩略图。
/// 多用户隔离：仅返回当前登录用户的相册。
#[tauri::command]
pub fn get_albums(
    app: tauri::AppHandle,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<Vec<crate::db::Album>, String> {
    let _t = log_call!("get_albums");
    let user_id = crate::require_user(&session)?;
    let thumbs = crate::thumbs_dir(&app)?;
        let mut albums = {
            let db = state.0.lock().map_err(|e| e.to_string())?;
            db.get_albums(user_id).map_err(|e| e.to_string())?
        };
    for a in albums.iter_mut() {
        fill_album_stats(a, &thumbs, &state);
    }
    // FEAT-036：批量填充每个相册的已入库照片数（一次 SQL 分组统计，避免 N+1）
    fill_scanned_counts(&mut albums, user_id, &state);
    crate::logger::log_call_end_with("get_albums", _t, &format!("OK | count={}", albums.len()));
    Ok(albums)
}


/// 获取单个相册详情（需求 §4.2 get_album）
///
/// 多用户隔离：仅能获取归属当前用户的相册。
#[tauri::command]
pub fn get_album(
    id: i64,
    app: tauri::AppHandle,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<crate::db::Album, String> {
    let user_id = crate::require_user(&session)?;
    let thumbs = crate::thumbs_dir(&app)?;
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
pub fn list_album_photos(
    album_id: i64,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<Vec<String>, String> {
    let _t = log_call!("list_album_photos", &format!("album_id={album_id}"));
    let user_id = crate::require_user(&session)?;
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
    crate::logger::log_call_end_with("list_album_photos", _t, &format!("OK | count={count}"));
    Ok(paths)
}

/// 多用户隔离：仅能更新归属当前用户的相册。
#[tauri::command]
pub fn update_album(
    input: UpdateAlbumInput,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<(), String> {
    let user_id = crate::require_user(&session)?;
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
pub struct LocationDetectResult {
    location: String,
    changed: bool,
    lat: f64,
    lon: f64,
}


#[tauri::command]
pub async fn auto_detect_album_location(
    album_id: i64,
    force: bool,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<LocationDetectResult, String> {
    let _t = log_call!("auto_detect_album_location", &format!("album_id={album_id} force={force}"));
    let user_id = crate::require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let album = db.get_album(album_id, user_id).map_err(|e| e.to_string())?;
    if album.location.is_some() && !force {
        let msg = format!("相册已有地点标签（{}），跳过自动识别；如需覆盖请用 force",
            album.location.as_deref().unwrap_or(""));
        crate::logger::log_call_end_with("auto_detect_album_location", _t, &format!("SKIP | {msg}"));
        return Err(msg);
    }
    let dir = album.path;
    drop(db); // 网络/文件 IO 期间不持有数据库锁
    // 1) GPS 众数坐标（~1km 网格）
    let coord = crate::photo_scan::detect_album_location(&dir)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "相册内照片无 GPS 坐标，无法自动识别地点".to_string())?;
    // 2) 反向地理编码：本地省/市优先（离线秒回），未命中再联网精确查询
    let place = match crate::geo_index::find_region(coord.0, coord.1) {
        Some(p) => p,
        None => crate::photo_scan::reverse_geocode_coord(coord.0, coord.1)
            .ok_or_else(|| "地名解析失败（本地未命中且联网查询失败）".to_string())?,
    };
    // 3) 落库（不动 updated_at）
    let db = state.0.lock().map_err(|e| e.to_string())?;
    db.update_album_location(album_id, user_id, &place).map_err(|e| e.to_string())?;
    drop(db);
    crate::logger::log_call_end_with(
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
pub fn rename_album(
    id: i64,
    new_name: String,
    rename_folder: bool,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<crate::db::Album, String> {
    let _t = log_call!(
        "rename_album",
        &format!("id={id}, new_name={new_name}, rename_folder={rename_folder}")
    );
    let user_id = crate::require_user(&session)?;
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
                crate::logger::log_call_end_with(
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
    crate::logger::log_call_end_with(
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
pub fn update_album_tags(
    album_id: i64,
    tags: Vec<String>,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<(), String> {
    let user_id = crate::require_user(&session)?;
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


pub fn _batch_album_outcome(requested: usize, failed_ids: Vec<i64>) -> BatchAlbumOutcome {
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
pub fn batch_move_album_to_folder(
    album_ids: Vec<i64>,
    folder_id: Option<i64>,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<BatchAlbumOutcome, String> {
    let _t = log_call!("batch_move_album_to_folder", &format!("ids={album_ids:?} folder={folder_id:?}"));
    let _user_id = crate::require_user(&session)?;

    let mut failed_ids = Vec::new();
    for id in &album_ids {
        // 逐个执行（move_album 内部校验归属与目标分组）
        if move_album(*id, folder_id, state.clone(), session.clone()).is_err() {
            failed_ids.push(*id);
        }
    }

    let out = _batch_album_outcome(album_ids.len(), failed_ids);
    crate::logger::log_call_end_with(
        "batch_move_album_to_folder",
        _t,
        &format!("OK | ok={} failed={}", out.ok, out.failed),
    );
    Ok(out)
}


/// 批量设置相册地点（可清空：传空字符串）
#[tauri::command]
pub fn batch_set_album_location(
    album_ids: Vec<i64>,
    location: String,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<BatchAlbumOutcome, String> {
    let _t = log_call!("batch_set_album_location", &format!("ids={album_ids:?} loc={location}"));
    let user_id = crate::require_user(&session)?;
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
    crate::logger::log_call_end_with(
        "batch_set_album_location",
        _t,
        &format!("OK | ok={} failed={}", out.ok, out.failed),
    );
    Ok(out)
}


/// 批量加/删相册标签（mode：`add` 追加 / `remove` 移除；最多 5 个）
#[tauri::command]
pub fn batch_set_album_tag(
    album_ids: Vec<i64>,
    tags: Vec<String>,
    mode: String,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<BatchAlbumOutcome, String> {
    let _t = log_call!("batch_set_album_tag", &format!("ids={album_ids:?} mode={mode} tags={tags:?}"));
    let user_id = crate::require_user(&session)?;
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
    crate::logger::log_call_end_with(
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
pub fn merge_albums(
    source_ids: Vec<i64>,
    target_id: i64,
    mode: Option<String>,
    app: tauri::AppHandle,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<MergeAlbumOutcome, String> {
    let mode = mode.as_deref().unwrap_or("move");
    let is_move = mode != "record";
    let _t = log_call!("merge_albums", &format!("source={source_ids:?} target={target_id} mode={mode}"));
    let user_id = crate::require_user(&session)?;

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
    if let Ok(thumbs) = crate::thumbs_dir(&app) {
        for id in &removed_ids {
            crate::thumbnail::cleanup_all_album_thumbs(*id, &thumbs);
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

    crate::logger::log_call_end_with(
        "merge_albums",
        _t,
        &format!("OK | merged={merged} files_moved={files_moved} files_failed={files_failed} skipped={skipped_count}"),
    );
    Ok(out)
}


/// 删除相册（需求 §4.2 delete_album，仅删记录不删本地文件）
///
/// 删除成功后同时清理该相册的缩略图缓存文件（数据库级联删除见 crate::db::delete_album）。
/// 多用户隔离：仅能删除归属当前登录用户的相册。
#[tauri::command]
pub fn delete_album(
    id: i64,
    app: tauri::AppHandle,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<(), String> {
    let _t = log_call!("delete_album", &format!("id={id}"));
    let user_id = crate::require_user(&session)?;
    let r = (|| -> Result<(), String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.delete_album(id, user_id).map_err(|e| e.to_string())
    })();
    if r.is_ok() {
        // 记录已删除，清理缓存文件（失败不影响删除结果）
        if let Ok(thumbs) = crate::thumbs_dir(&app) {
            crate::thumbnail::cleanup_all_album_thumbs(id, &thumbs);
        }
    }
    match &r {
        Ok(_) => crate::logger::log_call_end_with("delete_album", _t, "OK"),
        Err(e) => crate::logger::log_call_end_with("delete_album", _t, &format!("ERR | {e}")),
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
pub fn delete_albums(
    ids: Vec<i64>,
    app: tauri::AppHandle,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<usize, String> {
    let _t = log_call!("delete_albums", &format!("ids={ids:?}"));
    let user_id = crate::require_user(&session)?;
    let r = (|| -> Result<usize, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.delete_albums(&ids, user_id).map_err(|e| e.to_string())
    })();
    if let Ok(n) = &r {
        if *n > 0 {
            if let Ok(thumbs) = crate::thumbs_dir(&app) {
                for id in &ids {
                    crate::thumbnail::cleanup_all_album_thumbs(*id, &thumbs);
                }
            }
        }
    }
    match &r {
        Ok(n) => crate::logger::log_call_end_with("delete_albums", _t, &format!("OK | deleted={n}")),
        Err(e) => crate::logger::log_call_end_with("delete_albums", _t, &format!("ERR | {e}")),
    }
    r
}


/// 设置相册封面（需求 §2.3 设置封面 / §6.2 图片选择对话框）
///
/// 接收用户选择的图片路径，生成封面缩略图缓存到 thumbs/，
/// 并更新 Album.cover_path。返回更新后的相册。
#[tauri::command]
pub fn set_cover(
    id: i64,
    image_path: String,
    app: tauri::AppHandle,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<crate::db::Album, String> {
    let _t = log_call!("set_cover", &format!("album_id={id}, image_path={image_path}"));
    crate::logger::log_info(&format!("[SET_COVER] 开始更换封面: album_id={id}, 选择图片={image_path}"));
    let user_id = crate::require_user(&session)?;

    // 阶段1：校验图片路径存在（用户选择封面）
    let t1 = std::time::Instant::now();
    let img = std::path::Path::new(&image_path);
    if !img.is_file() {
        let e = format!("图片不存在: {image_path}");
        crate::logger::log_error("set_cover", &e);
        crate::logger::log_call_end_with("set_cover", _t, "FAILED | 图片不存在");
        return Err(e);
    }
    crate::logger::log_info(&format!(
        "[SET_COVER] 阶段1 图片校验通过: {}ms",
        t1.elapsed().as_millis()
    ));

    // 阶段2：生成封面缩略图（统一存到缓存目录）
    let thumbs = crate::thumbs_dir(&app)?;
    let t2 = std::time::Instant::now();
    let cover = match crate::thumbnail::generate_cover(id, img, &thumbs) {
        Ok(c) => {
            crate::logger::log_info(&format!(
                "[SET_COVER] 阶段2 生成封面缩略图: {}ms → {}",
                t2.elapsed().as_millis(),
                c
            ));
            c
        }
        Err(e) => {
            let e = format!("无法生成封面缩略图: {e}");
            crate::logger::log_error("set_cover", &e);
            crate::logger::log_call_end_with("set_cover", _t, "FAILED | 生成缩略图失败");
            return Err(e);
        }
    };

    // 阶段3：更新数据库 cover_path（更换封面）
    let t3 = std::time::Instant::now();
    {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.update_album(
            crate::db::UpdateAlbumInput {
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
    crate::logger::log_info(&format!(
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
    crate::thumbnail::cleanup_album_auto_thumbs(id, &thumbs);
    // 此时 cover_path 已设置，fill_album_stats 不会覆盖它
    fill_album_stats(&mut album, &thumbs, &state);
    crate::logger::log_info(&format!(
        "[SET_COVER] 阶段4 清理旧缩略图+刷新统计: {}ms",
        t4.elapsed().as_millis()
    ));

    crate::logger::log_call_end_with("set_cover", _t, &format!("OK | album_id={id}"));
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
pub struct ImportProgress {
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
pub fn import_albums(
    root_path: String,
    app: tauri::AppHandle,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<ImportResult, String> {
    let user_id = crate::require_user(&session)?;
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
                        crate::db::CreateAlbumInput {
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


/// 按名称模糊搜索相册（含分组归属路径）
///
/// 多用户隔离：仅搜索当前登录用户的相册空间。
#[tauri::command]
pub fn search_albums(
    keyword: String,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<Vec<crate::db::AlbumSearchResult>, String> {
    let _t = log_call!("search_albums", &format!("keyword={keyword}"));
    let user_id = crate::require_user(&session)?;
    let r = (|| -> Result<Vec<crate::db::AlbumSearchResult>, String> {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        db.search_albums(&keyword, user_id).map_err(|e| e.to_string())
    })();
    match &r {
        Ok(list) => crate::logger::log_call_end_with("search_albums", _t, &format!("OK | count={}", list.len())),
        Err(e) => crate::logger::log_call_end_with("search_albums", _t, &format!("ERR | {e}")),
    }
    r
}


/// 移动相册到分组或调整顺序
///
/// - `folder_id` 为 None 表示移到顶级
/// - 相册移到目标分组后排在组内末尾
/// - 多用户隔离：仅能移动归属当前登录用户的相册到自己的分组
#[tauri::command]
pub fn move_album(
    album_id: i64,
    folder_id: Option<i64>,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<(), String> {
    let _start = crate::logger::log_call_start("move_album", &format!("album_id={album_id}, folder_id={folder_id:?}"));
    let user_id = crate::require_user(&session)?;
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
    crate::logger::log_call_end_with("move_album", _start, &format!("album_id={album_id}, folder_id={folder_id:?}"));
    Ok(())
}


/// 相册组内排序
///
/// 将相册移到同一分组内的新位置。`new_index` 为该相册在目标分组（同一分组）中的新下标。
/// 若 `folder_id` 提供且与相册当前分组不同，则先移动到该分组再插入指定位置。
/// 多用户隔离：仅能排序归属当前登录用户的相册与分组。
#[tauri::command]
pub fn reorder_album(
    album_id: i64,
    folder_id: Option<i64>,
    new_index: i64,
    state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<(), String> {
    let user_id = crate::require_user(&session)?;
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
