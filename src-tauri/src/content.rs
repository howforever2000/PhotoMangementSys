//! 内容扫描服务层（FEAT-022：AI 内容扫描入库 + 照片智能搜索）
//!
//! 职责：
//! 1. `scan_album_content`：对相册执行 AI 内容识别（复用 `vision` 微服务）+ EXIF
//!    提取（复用 `photo_scan`），为每张照片计算唯一哈希（路径+大小+修改时间），
//!    按哈希 upsert 落库（二次扫描以二次结果为准）。
//! 2. `search_photo_content`：按关键词搜索照片内容（群相册全局 / 单相册内部，范围由
//!    `album_id` 决定），复用 `db::content` 持久层。
//!
//! 解耦原则（C4，文件不过重）：
//! - 持久化（建表/写入/查询）全部在 `db::content`，本模块只做编排
//! - 识别/EXIF 分别复用 `vision` / `photo_scan`，不重复实现
//! - 命令定义为薄壳（`commands` 子模块），`lib.rs` 仅注册
//! - 接入公共 logger 出入口日志（CS2）

use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Serialize;
use tauri::Emitter;

use crate::db::PhotoContentRecord;

/// 支持向上游上报的扫描进度事件载荷
#[derive(Debug, Clone, Serialize)]
pub struct ContentScanProgress {
    pub current: usize,
    pub total: usize,
    pub file_name: String,
}

/// 一次内容扫描的报告
#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    /// 本次扫描识别到的图片数（含识别失败）
    pub total: usize,
    /// 成功写入/更新的记录数
    pub written: usize,
    /// 识别失败（未落库）数
    pub failed: usize,
}

/// 内容扫描命令返回值：报告 + 识别明细（供前端复用现有识别表格展示）
#[derive(Debug, Clone, Serialize)]
pub struct ScanOutcome {
    pub report: ScanReport,
    /// 本次识别的照片明细（含类别/细类/label/人物/耗时等，复用 `vision` 结果）
    pub results: Vec<crate::vision::VisionResult>,
}

/// 照片唯一哈希：路径 + 文件大小 + 修改时间（纳秒）组合，FNV-1a 64 位 → 16 位十六进制
///
/// - 不整读文件内容，快速稳定（跨进程/重启一致，可持久化去重）
/// - 二次扫描同哈希 → `db` 层 upsert 覆盖更新（以二次结果为准）
/// - 文件内容变化但大小/mtime 不变（极少）时可能不识别为新文件，可接受
pub(crate) fn photo_hash(path: &str, len: u64, mtime_ns: u128) -> String {
    let input = format!("{len}|{mtime_ns}|{path}");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in input.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 扫描编排：识别 + 提取 EXIF + 计算哈希 → 构建待写入记录
///
/// 在阻塞线程执行（含逐张文件 IO），通过 `app` 实时上报 `content-scan-progress` 事件。
/// 返回待写入记录列表；识别失败的照片跳过（不落库）。
fn build_records(
    album_id: i64,
    user_id: i64,
    results: &[crate::vision::VisionResult],
    app: &tauri::AppHandle,
) -> Result<Vec<PhotoContentRecord>, String> {
    let mut recs: Vec<PhotoContentRecord> = Vec::with_capacity(results.len());
    let total = results.len();
    let mut current = 0usize;

    for r in results {
        current += 1;
        // 识别失败的照片跳过落库
        if r.error.is_some() {
            continue;
        }
        let path = Path::new(&r.path);
        // 唯一哈希：路径 + 大小 + 修改时间
        let (len, mtime_ns) = match std::fs::metadata(path) {
            Ok(md) => (
                md.len(),
                md.modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ),
            Err(_) => (0, 0),
        };
        let hash = photo_hash(&r.path, len, mtime_ns);
        // 父目录（绝对地址索引用）
        let parent_dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        // 预留 EXIF 字段：复用 photo_scan 提取
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ex = crate::photo_scan::read_photo_exif(path, &name);

        // 聚合可搜索文本（大类+细类+label+Top3 细类+人物标号）
        let mut parts: Vec<String> = Vec::new();
        if !r.category.is_empty() {
            parts.push(r.category.clone());
        }
        if !r.sub_category.is_empty() {
            parts.push(r.sub_category.clone());
        }
        if !r.label.is_empty() {
            parts.push(r.label.clone());
        }
        for t in &r.top3 {
            if !t.label.is_empty() {
                parts.push(t.label.clone());
            }
        }
        parts.extend(r.person_ids.iter().cloned());

        let top3_json = if r.top3.is_empty() {
            None
        } else {
            serde_json::to_string(&r.top3).ok()
        };
        let person_ids_json = if r.person_ids.is_empty() {
            None
        } else {
            serde_json::to_string(&r.person_ids).ok()
        };

        recs.push(PhotoContentRecord {
            photo_hash: hash,
            path: r.path.clone(),
            parent_dir,
            album_id: Some(album_id),
            user_id,
            content: parts.join(" ").to_lowercase(),
            category: opt_nonempty(&r.category),
            sub_category: opt_nonempty(&r.sub_category),
            label: opt_nonempty(&r.label),
            confidence: Some(r.confidence),
            top3_json,
            person_ids: person_ids_json,
            person_count: r.person_count as i64,
            shoot_time: ex.shoot_time,
            location: ex.place,
            shutter_speed: ex.shutter_speed,
            iso: ex.iso,
            aperture: ex.aperture,
            focal_length: ex.focal_length,
            lat: ex.lat,
            lon: ex.lon,
        });

        let _ = app.emit(
            "content-scan-progress",
            ContentScanProgress {
                current,
                total,
                file_name: r.file_name.clone(),
            },
        );
    }
    Ok(recs)
}

/// 空字符串 → None（落库为 NULL），否则保留
fn opt_nonempty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// 命令层（薄壳，逻辑见上；`lib.rs` 仅注册）
pub mod commands {
    use super::*;
    use crate::{db, logger, require_user, AppState, SessionState};

    /// 对相册执行 AI 内容扫描并落库（二次扫描按哈希覆盖更新）
    ///
    /// - 识别与 EXIF 提取在阻塞线程执行，不冻结 UI
    /// - `batch_size`：推理批次（默认 8），经 vision 透传给 Python 服务
    /// - 通过 `content-scan-progress` 事件实时上报进度
    /// - 多用户隔离：仅能扫描归属当前登录用户的相册
    #[tauri::command]
    pub async fn scan_album_content(
        album_id: i64,
        batch_size: Option<i64>,
        app: tauri::AppHandle,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<ScanOutcome, String> {
        let _t = log_call!("scan_album_content", &format!("album_id={album_id} batch={batch_size:?}"));
        let user_id = require_user(&session)?;
        // 获取相册路径（多用户隔离）
        let path = {
            let db = state.0.lock().map_err(|e| e.to_string())?;
            db.get_album(album_id, user_id).map_err(|e| e.to_string())?.path
        };
        // AI 内容识别（async，复用 vision 微服务）
        let batch = batch_size.unwrap_or(8).max(1) as usize;
        let results = crate::vision::classify_album(&path, batch, &app).await?;
        let total = results.len();
        let failed = results.iter().filter(|r| r.error.is_some()).count();
        // 构建记录（阻塞线程：逐张 EXIF + 哈希 + 上报进度）
        let app2 = app.clone();
        let results_for_block = results.clone();
        let recs = tauri::async_runtime::spawn_blocking(move || {
            build_records(album_id, user_id, &results_for_block, &app2)
        })
        .await
        .map_err(|e| format!("任务线程失败: {e}"))??;

        // 落库（单事务批量 upsert）
        let written = recs.len();
        let upsert = (|| -> Result<ScanReport, String> {
            let db = state.0.lock().map_err(|e| e.to_string())?;
            db.upsert_photo_contents(&recs).map_err(|e| e.to_string())?;
            Ok(ScanReport { total, written, failed })
        })();
        let outcome = upsert.map(|rep| ScanOutcome { report: rep, results });

        match &outcome {
            Ok(o) => logger::log_call_end_with(
                "scan_album_content",
                _t,
                &format!("OK | total={} written={} failed={}", o.report.total, o.report.written, o.report.failed),
            ),
            Err(e) => logger::log_call_end_with("scan_album_content", _t, &format!("ERR | {e}")),
        }
        outcome
    }

    /// 按关键词搜索照片内容（智能搜索）
    ///
    /// - `album_id`：`None` → 群相册/全局搜索；`Some(id)` → 单相册内部搜索（需求 R4）
    /// - 多用户隔离：仅搜索当前登录用户的照片内容
    #[tauri::command]
    pub async fn search_photo_content(
        keyword: String,
        album_id: Option<i64>,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<db::ContentSearchHit>, String> {
        let _t = log_call!("search_photo_content", &format!("keyword={keyword} album_id={album_id:?}"));
        let user_id = require_user(&session)?;
        let kw = keyword.trim().to_string();
        if kw.is_empty() {
            return Ok(Vec::new());
        }
        let r = (|| -> Result<Vec<db::ContentSearchHit>, String> {
            let db = state.0.lock().map_err(|e| e.to_string())?;
            db.search_photo_content(&kw, user_id, album_id).map_err(|e| e.to_string())
        })();
        match &r {
            Ok(list) => logger::log_call_end_with(
                "search_photo_content",
                _t,
                &format!("OK | hits={}", list.len()),
            ),
            Err(e) => logger::log_call_end_with("search_photo_content", _t, &format!("ERR | {e}")),
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_distinct() {
        let a = photo_hash("/x/a.jpg", 100, 1234567890);
        let b = photo_hash("/x/a.jpg", 100, 1234567890);
        assert_eq!(a, b, "同输入哈希应稳定一致（跨重启去重）");
        let c = photo_hash("/x/b.jpg", 100, 1234567890);
        assert_ne!(a, c, "不同路径哈希应不同");
        let d = photo_hash("/x/a.jpg", 101, 1234567890);
        assert_ne!(a, d, "不同大小哈希应不同");
        let e = photo_hash("/x/a.jpg", 100, 1234567891);
        assert_ne!(a, e, "不同 mtime 哈希应不同");
    }
}
