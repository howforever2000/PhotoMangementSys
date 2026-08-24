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

use serde::{Deserialize, Serialize};
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

// ---- FEAT-026：组合扫描 + 读表 + 条件搜索 ----

/// 组合扫描统一展示行：合并 EXIF / 影调 / AI 三类结果
#[derive(Debug, Clone, Serialize)]
pub struct UnifiedScanRow {
    pub file_name: String,
    pub path: String,
    // EXIF
    pub iso: Option<String>,
    pub aperture: Option<String>,
    pub shutter_speed: Option<String>,
    pub focal_length: Option<String>,
    pub shoot_time: Option<String>,
    pub iso_num: Option<u32>,
    pub focal_num: Option<f64>,
    pub aperture_num: Option<f64>,
    pub shutter_num: Option<f64>,
    // 影调
    pub tone_type: Option<String>,
    pub avg_luma: Option<f64>,
    // AI
    pub category: Option<String>,
    pub sub_category: Option<String>,
    pub label: Option<String>,
    pub confidence: Option<f64>,
    pub top3: Vec<crate::vision::VisionTopItem>,
    pub person_ids: Vec<String>,
    pub person_count: i64,
}

/// 组合扫描结果：报告 + 统一行
#[derive(Debug, Clone, Serialize)]
pub struct CombinedScanOutcome {
    pub report: ScanReport,
    pub rows: Vec<UnifiedScanRow>,
}

/// 内容搜索过滤条件（前端下拉/预设直接映射）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContentScanFilters {
    pub iso_min: Option<u32>,
    pub iso_max: Option<u32>,
    pub shutter_min: Option<f64>,
    pub shutter_max: Option<f64>,
    pub aperture_min: Option<f64>,
    pub aperture_max: Option<f64>,
    pub focal_min: Option<f64>,
    pub focal_max: Option<f64>,
    pub tone_type: Option<String>,
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
            iso_num: ex.iso_num,
            focal_num: ex.focal_num,
            aperture_num: ex.aperture_num,
            shutter_num: ex.shutter_num,
            tone_type: None,
            avg_luma: None,
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

/// FEAT-026：组合扫描（EXIF + 影调 + AI）统一记录构造
///
/// - 参数与 `build_records` 一致 + `tone_scan` 可选（`None` 跳过影调字段）
/// - 影调按路径匹配合并；未命中则 tone 字段留 None
/// - 返回 `(records, unified_rows)`：records 供落库，unified_rows 供前端统一表格展示
fn build_records_combined(
    album_id: i64,
    user_id: i64,
    results: &[crate::vision::VisionResult],
    tone_scan: Option<&Vec<crate::tone::PhotoTone>>,
    app: &tauri::AppHandle,
) -> Result<(Vec<PhotoContentRecord>, Vec<UnifiedScanRow>), String> {
    // 影调按路径建索引（命中才填充，未命中仍返回 EXIF + AI 行）
    let tone_map: std::collections::HashMap<String, &crate::tone::PhotoTone> =
        tone_scan
            .map(|v| {
                v.iter()
                    .map(|t| (t.path.clone(), t))
                    .collect()
            })
            .unwrap_or_default();

    let mut recs: Vec<PhotoContentRecord> = Vec::with_capacity(results.len());
    let mut rows: Vec<UnifiedScanRow> = Vec::with_capacity(results.len());
    let total = results.len();
    let mut current = 0usize;

    for r in results {
        current += 1;
        if r.error.is_some() {
            continue;
        }
        let path = Path::new(&r.path);
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
        let parent_dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ex = crate::photo_scan::read_photo_exif(path, &name);

        // 影调匹配（按路径；未命中留 None）
        let tone = tone_map.get(&r.path);
        let (tone_type, avg_luma) = if let Some(t) = tone {
            (t.tone_type.map(|e| format!("{:?}", e)), t.avg_luma)
        } else {
            (None, None)
        };

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
            shoot_time: ex.shoot_time.clone(),
            location: ex.place.clone(),
            shutter_speed: ex.shutter_speed.clone(),
            iso: ex.iso.clone(),
            aperture: ex.aperture.clone(),
            focal_length: ex.focal_length.clone(),
            iso_num: ex.iso_num,
            focal_num: ex.focal_num,
            aperture_num: ex.aperture_num,
            shutter_num: ex.shutter_num,
            tone_type,
            avg_luma,
            lat: ex.lat,
            lon: ex.lon,
        });

        rows.push(UnifiedScanRow {
            file_name: r.file_name.clone(),
            path: r.path.clone(),
            iso: ex.iso,
            aperture: ex.aperture,
            shutter_speed: ex.shutter_speed,
            focal_length: ex.focal_length,
            shoot_time: ex.shoot_time,
            iso_num: ex.iso_num,
            focal_num: ex.focal_num,
            aperture_num: ex.aperture_num,
            shutter_num: ex.shutter_num,
            tone_type: tone
                .map(|t| t.tone_type.map(|e| format!("{:?}", e)))
                .unwrap_or(None),
            avg_luma: tone.map(|t| t.avg_luma).unwrap_or(None),
            category: opt_nonempty(&r.category),
            sub_category: opt_nonempty(&r.sub_category),
            label: opt_nonempty(&r.label),
            confidence: Some(r.confidence),
            top3: r.top3.clone(),
            person_ids: r.person_ids.clone(),
            person_count: r.person_count as i64,
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
    Ok((recs, rows))
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
        scan: tauri::State<'_, crate::ScanState>,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<ScanOutcome, String> {
        let _t = log_call!("scan_album_content", &format!("album_id={album_id} batch={batch_size:?}"));
        let user_id = require_user(&session)?;
        // 获取相册路径（多用户隔离）
        let path = {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.get_album(album_id, user_id).map_err(|e| format!("{:?}", e))?.path
        };
        // AI 内容识别（async，复用 vision 微服务）；支持停止
        let batch = batch_size.unwrap_or(8).max(1) as usize;
        scan.0.store(false, std::sync::atomic::Ordering::SeqCst);
        let results = crate::vision::classify_album(&path, batch, &app, Some(scan.0.clone())).await?;
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
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.upsert_photo_contents(&recs).map_err(|e| format!("{:?}", e))?;
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
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.search_photo_content(&kw, user_id, album_id).map_err(|e| format!("{:?}", e))
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

    // ---- FEAT-026：组合扫描 + 读表 + 条件搜索 ----

    /// 组合扫描（EXIF + 影调 + AI 可选组合）并统一入库
    ///
    /// - `scan_types`：允许的集合为 `["basic", "tone", "ai"]`，前端勾选项直接映射
    /// - 至少勾选一项；三项全勾则同时执行并合并结果
    /// - 落库到 `photo_content_scan`，前端返回统一行（`UnifiedScanRow`）用于表格展示
    #[tauri::command]
    pub async fn scan_album_combined(
        album_id: i64,
        scan_types: Vec<String>,
        batch_size: Option<i64>,
        app: tauri::AppHandle,
        scan: tauri::State<'_, crate::ScanState>,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<CombinedScanOutcome, String> {
        let _t = log_call!("scan_album_combined", &format!("album_id={album_id} scan_types={scan_types:?}"));
        let user_id = require_user(&session)?;

        if scan_types.is_empty() {
            return Err("scan_types 不能为空".to_string());
        }
        if !scan_types.iter().all(|s| ["basic", "tone", "ai"].contains(&s.as_str())) {
            return Err("非法 scan_types，允许 basic / tone / ai".to_string());
        }

        // 重置取消标记：本次扫描全新开始；前端「停止」→ `cancel_scan` 置位后提前结束
        scan.0.store(false, std::sync::atomic::Ordering::SeqCst);
        let cancel = scan.0.clone();

        let dir = {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.get_album(album_id, user_id).map_err(|e| format!("{:?}", e))?.path
        };

        let do_basic = scan_types.contains(&"basic".to_string());
        let do_tone = scan_types.contains(&"tone".to_string());
        let do_ai = scan_types.contains(&"ai".to_string());
        let batch = batch_size.unwrap_or(8).clamp(4, 64) as usize;

        // AI 识别为异步 HTTP（在异步运行时上每批检查取消标记），不占用主线程
        let vision_results = if do_ai {
            crate::vision::classify_album(&dir, batch, &app, Some(cancel.clone())).await?
        } else {
            Vec::new()
        };

        // 影调扫描为同步重活 → 放入阻塞线程，避免占满异步运行时影响其他命令
        let tones = if do_tone {
            let dir2 = dir.clone();
            tauri::async_runtime::spawn_blocking(move || crate::tone::scan_album_tones(&dir2))
                .await
                .map_err(|e| format!("影调任务线程失败: {e}"))??
        } else {
            Vec::new()
        };

        // EXIF 扫描（仅非 AI 分支需要）→ 同样放入阻塞线程
        let exifs = if do_basic && !do_ai {
            let dir2 = dir.clone();
            tauri::async_runtime::spawn_blocking(move || crate::photo_scan::scan_album_photos(&dir2))
                .await
                .map_err(|e| format!("EXIF 任务线程失败: {e}"))??
        } else {
            Vec::new()
        };

        let outcome: Result<CombinedScanOutcome, String> = (|| -> Result<CombinedScanOutcome, String> {
            if do_ai {
                let tone_ref = if do_tone { Some(&tones) } else { None };
                let (recs, rows) =
                    build_records_combined(album_id, user_id, &vision_results, tone_ref, &app)?;
                let written_count = recs.len();
                {
                    let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
                    db.upsert_photo_contents(&recs).map_err(|e| format!("{:?}", e))?;
                }
                let report = ScanReport {
                    total: vision_results.len(),
                    written: written_count,
                    failed: vision_results.iter().filter(|r| r.error.is_some()).count(),
                };
                Ok(CombinedScanOutcome { report, rows })
            } else {
                let tone_map: std::collections::HashMap<String, &crate::tone::PhotoTone> =
                    if do_tone {
                        tones.iter().map(|t| (t.path.clone(), t)).collect()
                    } else {
                        std::collections::HashMap::new()
                    };

                let mut all_rows: Vec<UnifiedScanRow> = Vec::new();
                if do_basic {
                    for ex in &exifs {
                        let tone = tone_map.get(&ex.path);
                        all_rows.push(UnifiedScanRow {
                            file_name: ex.file_name.clone(),
                            path: ex.path.clone(),
                            iso: ex.iso.clone(),
                            aperture: ex.aperture.clone(),
                            shutter_speed: ex.shutter_speed.clone(),
                            focal_length: ex.focal_length.clone(),
                            shoot_time: ex.shoot_time.clone(),
                            iso_num: ex.iso_num,
                            focal_num: ex.focal_num,
                            aperture_num: ex.aperture_num,
                            shutter_num: ex.shutter_num,
                            tone_type: tone.map(|t| t.tone_type.map(|e| format!("{:?}", e))).unwrap_or(None),
                            avg_luma: tone.map(|t| t.avg_luma).unwrap_or(None),
                            category: None,
                            sub_category: None,
                            label: None,
                            confidence: None,
                            top3: Vec::new(),
                            person_ids: Vec::new(),
                            person_count: 0,
                        });
                    }
                } else {
                    for t in &tones {
                        all_rows.push(UnifiedScanRow {
                            file_name: t.file_name.clone(),
                            path: t.path.clone(),
                            iso: None,
                            aperture: None,
                            shutter_speed: None,
                            focal_length: None,
                            shoot_time: None,
                            iso_num: None,
                            focal_num: None,
                            aperture_num: None,
                            shutter_num: None,
                            tone_type: t.tone_type.map(|e| format!("{:?}", e)),
                            avg_luma: t.avg_luma,
                            category: None,
                            sub_category: None,
                            label: None,
                            confidence: None,
                            top3: Vec::new(),
                            person_ids: Vec::new(),
                            person_count: 0,
                        });
                    }
                }
                Ok(CombinedScanOutcome {
                    report: ScanReport {
                        total: all_rows.len(),
                        written: 0,
                        failed: 0,
                    },
                    rows: all_rows,
                })
            }
        })();

        match &outcome {
            Ok(o) => logger::log_call_end_with(
                "scan_album_combined",
                _t,
                &format!("OK | total={} written={}", o.report.total, o.report.written),
            ),
            Err(e) => logger::log_call_end_with("scan_album_combined", _t, &format!("ERR | {e}")),
        }
        outcome
    }

    /// 单相册内容读表（分页）：把已扫描入库的记录读出供前端表格展示
    ///
    /// - 返回 `Vec<db::AlbumContentRow>`（统一字段：EXIF + 影调 + AI）
    /// - 返回同时通过 `meta` 字段（前端用）返回 total 供分页计算
    #[tauri::command]
    pub async fn read_album_content(
        album_id: i64,
        page: i64,
        page_size: i64,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<(Vec<db::AlbumContentRow>, i64), String> {
        let _t = log_call!("read_album_content", &format!("album_id={album_id} page={page} page_size={page_size}"));
        let user_id = require_user(&session)?;

        let r = (|| -> Result<(Vec<db::AlbumContentRow>, i64), String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.read_album_content(album_id, user_id, page, page_size).map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok((rows, total)) => logger::log_call_end_with(
                "read_album_content",
                _t,
                &format!("OK | rows={} total={}", rows.len(), total),
            ),
            Err(e) => logger::log_call_end_with("read_album_content", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 带过滤条件的内容搜索（FEAT-026）
    ///
    /// - `keyword` 空串不启用关键词过滤
    /// - `filters` 各字段 `None` 不启用该维度过滤；有值才参与范围/枚举限定
    #[tauri::command]
    pub async fn search_photo_content_with_filters(
        keyword: String,
        album_id: i64,
        filters: ContentScanFilters,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<db::AlbumContentRow>, String> {
        let _t = log_call!("search_photo_content_with_filters", &format!("album_id={album_id} keyword={keyword} filters={filters:?}"));
        let user_id = require_user(&session)?;

        let r = (|| -> Result<Vec<db::AlbumContentRow>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            let db_filters = db::ContentFilters {
                iso_min: filters.iso_min,
                iso_max: filters.iso_max,
                shutter_min: filters.shutter_min,
                shutter_max: filters.shutter_max,
                aperture_min: filters.aperture_min,
                aperture_max: filters.aperture_max,
                focal_min: filters.focal_min,
                focal_max: filters.focal_max,
                tone_type: filters.tone_type,
            };
            db.search_photo_content_with_filters(
                &keyword,
                user_id,
                Some(album_id),
                &db_filters,
            )
            .map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok(list) => logger::log_call_end_with(
                "search_photo_content_with_filters",
                _t,
                &format!("OK | hits={}", list.len()),
            ),
            Err(e) => logger::log_call_end_with(
                "search_photo_content_with_filters",
                _t,
                &format!("ERR | {e}"),
            ),
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
