//! FEAT-095：照片归档 / 隐藏标记命令层
//! ===================================================================
//! 职责：归档（archived）/ 隐藏（hidden）标记的设置、取消与查询。
//! 持久化在 `db` 模块（`photo_marks` 表），本文件只是命令层。
//!
//! 设计要点：
//! - **只打标记，不移动文件**：照片仍在原相册目录，EXIF / 评分 / 标签 / 人物关联全部保留；
//! - 标记后照片从原相册视图移出（由 `album::list_album_photos` 过滤 `photo_marks` 实现），
//!   在归档 / 隐藏相册中集中呈现（虚拟相册 `path` 为哨兵值，见 `db::VIRTUAL_PATH_*`）；
//! - 状态互斥：同一张照片同一时刻只能处于 archived / hidden 之一。

use crate::db::{PhotoMarkOutcome, PhotoMarkState, MARK_ARCHIVED, MARK_HIDDEN};

/// 校验状态字符串（只接受 archived / hidden），避免脏数据落库
fn validate_state(state: &str) -> Result<&'static str, String> {
    match state {
        s if s == MARK_ARCHIVED => Ok(MARK_ARCHIVED),
        s if s == MARK_HIDDEN => Ok(MARK_HIDDEN),
        other => Err(format!(
            "未知的照片标记状态: {other}（仅支持 archived / hidden）"
        )),
    }
}

/// 批量归档 / 隐藏 / 取消归档 / 取消隐藏照片
///
/// - `state`: `"archived"` | `"hidden"`
/// - `marked`: `true` = 打标记；`false` = 取消标记
///
/// 多用户隔离：标记按 `user_id` 写入，照片文件本身不动。
#[tauri::command]
pub fn set_photo_marks(
    state: String,
    paths: Vec<String>,
    marked: bool,
    app_state: tauri::State<crate::AppState>,
    session: tauri::State<crate::SessionState>,
) -> Result<PhotoMarkOutcome, String> {
    let _t = log_call!(
        "set_photo_marks",
        &format!("state={state} marked={marked} paths={}", paths.len())
    );
    let user_id = crate::require_user(&session)?;
    let kind = validate_state(&state)?;
    let db = app_state.0.lock().map_err(|e| e.to_string())?;
    let outcome = db
        .set_photo_marks(user_id, kind, &paths, marked)
        .map_err(|e| e.to_string())?;
    let verb = if marked { "标记" } else { "取消标记" };
    crate::logger::log_call_end_with(
        "set_photo_marks",
        _t,
        &format!(
            "OK | {verb} marked={} unmarked={} failed={}",
            outcome.marked,
            outcome.unmarked,
            outcome.failed_paths.len()
        ),
    );
    Ok(outcome)
}

/// 列出某状态（归档 / 隐藏）下的全部照片路径
///
/// 归档 / 隐藏虚拟相册的数据源；按最近标记时间倒序。
#[tauri::command]
pub fn list_marked_photos(
    state: String,
    session: tauri::State<crate::SessionState>,
    app_state: tauri::State<crate::AppState>,
) -> Result<Vec<String>, String> {
    let _t = log_call!("list_marked_photos", &format!("state={state}"));
    let user_id = crate::require_user(&session)?;
    let kind = validate_state(&state)?;
    let db = app_state.0.lock().map_err(|e| e.to_string())?;
    let paths = db
        .list_photo_marks(user_id, kind)
        .map_err(|e| e.to_string())?;
    crate::logger::log_call_end_with(
        "list_marked_photos",
        _t,
        &format!("OK | count={}", paths.len()),
    );
    Ok(paths)
}

/// 批量查询照片的标记状态（右键菜单据此显示「取消归档 / 取消隐藏」）
#[tauri::command]
pub fn get_photo_mark_states(
    paths: Vec<String>,
    session: tauri::State<crate::SessionState>,
    app_state: tauri::State<crate::AppState>,
) -> Result<Vec<PhotoMarkState>, String> {
    let _t = log_call!(
        "get_photo_mark_states",
        &format!("paths={}", paths.len())
    );
    let user_id = crate::require_user(&session)?;
    let db = app_state.0.lock().map_err(|e| e.to_string())?;
    let states = db
        .get_photo_mark_states(user_id, &paths)
        .map_err(|e| e.to_string())?;
    crate::logger::log_call_end_with(
        "get_photo_mark_states",
        _t,
        &format!("OK | count={}", states.len()),
    );
    Ok(states)
}
