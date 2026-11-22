//! 入库读数与对账（页面「已入库」的唯一口径）
//!
//! ══════════════════════════════════════════════════════════════════════════
//! 为什么需要独立命令（BUG：25k 张库显示「已入库 60585」）
//! ──────────────────────────────────────────────────────────────────────────
//! 旧口径是「每个相册各数一遍自己目录前缀下的扫描行，再加起来」：
//! - 父子相册 / 同一文件夹重复导入时，同一张照片会被**每个包含它的相册各算一次**；
//! - `photo_content_scan` 的主键是 `路径+大小+mtime` 的哈希，路径写法漂移或 mtime
//!   变化就会为同一张照片再插一行，而旧行不会自动消失；
//! - 文件被移走 / 在系统里删掉后，扫描行同样不会回收。
//!
//! 三者叠加，Σ 就会超过真实照片数（用户截图：25384 张照片 / 60585 已入库），
//! 而「重复清理」按路径删行后数字又回落 —— 那正是行数在变的证据。
//!
//! 本命令给页面一个**按路径去重**的总量：`COUNT(DISTINCT path)`。每册徽标继续用
//! 前缀统计（那里语义是「这册覆盖了多少」，父子各显示完整覆盖是对的），
//! 但页头汇总必须用这里，否则同一个量在两张页面两种口径。
//!
//! 另附 `prune_duplicate_scan_rows`：把历史上同一路径的多余行清掉（保留最新一行，
//! 级联清向量 / 描述向量 / 分类命中 / 缩略图缓存行）。读数立刻正确，且表不再越扫越肥。

use tauri::State;

use crate::{require_user, AppState, SessionState};

/// 全库入库读数（与 `db::content::ScanLibraryTotals` 同构，直接复用）
pub use crate::db::content::ScanLibraryTotals;

/// 全库入库总量（去重口径）
///
/// 返回 `rows`（表行数）/ `paths`（去重路径数）/ `duplicate_rows`（前者减后者）。
/// 页头「已入库照片」用 `paths`；`duplicate_rows > 0` 说明库里还留着历史冗余行，
/// 前端据此提示「可清理」。
#[tauri::command]
pub fn scan_library_totals(
    state: State<AppState>,
    session: State<SessionState>,
) -> Result<ScanLibraryTotals, String> {
    let _t = log_call!("scan_library_totals");
    let user_id = require_user(&session)?;
    let r = (|| -> Result<ScanLibraryTotals, String> {
        let db = state.0.lock().map_err(|e| format!("{e:?}"))?;
        db.scan_library_totals(user_id).map_err(|e| format!("{e}"))
    })();
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "scan_library_totals",
            _t,
            &format!("OK | rows={} paths={} dup={}", v.rows, v.paths, v.duplicate_rows),
        ),
        Err(e) => crate::logger::log_call_end_with("scan_library_totals", _t, &format!("ERR | {e}")),
    }
    r
}

/// 清理「同一路径被记了多行」的历史冗余（每路径保留最新一行）
///
/// 返回清理掉的**多余行数**。只删扫描记录及其派生数据（向量 / 描述向量 /
/// 分类命中 / 缩略图缓存行），**绝不碰用户磁盘上的照片**。
#[tauri::command]
pub fn prune_duplicate_scan_rows(
    state: State<AppState>,
    session: State<SessionState>,
) -> Result<usize, String> {
    let _t = log_call!("prune_duplicate_scan_rows");
    let user_id = require_user(&session)?;
    let r = (|| -> Result<usize, String> {
        let db = state.0.lock().map_err(|e| format!("{e:?}"))?;
        db.prune_duplicate_scan_rows(user_id).map_err(|e| format!("{e}"))
    })();
    match &r {
        Ok(n) => crate::logger::log_call_end_with(
            "prune_duplicate_scan_rows",
            _t,
            &format!("OK | removed={n}"),
        ),
        Err(e) => {
            crate::logger::log_call_end_with("prune_duplicate_scan_rows", _t, &format!("ERR | {e}"))
        }
    }
    r
}
