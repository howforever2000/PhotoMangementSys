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

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::AppState;

use crate::db::PhotoContentRecord;

/// FEAT-SEM：语义向量内存缓存（user_id → (版本信号, [(photo_hash, album_id, 向量)])）
///
/// - 向量为服务端已 L2 归一化的 512 维 f32，检索端点积即余弦
/// - 版本信号 = photo_embeddings 的 (count, max_rowid)，任一变化（新扫描/删除）
///   时懒重载，避免每次搜索全量读库
/// - Arc 包装：读路径零拷贝（全库 1 万张 ≈ 23MB，clone 仅引用计数）
type EmbedCacheEntry = Arc<(crate::db::EmbeddingVersion, Vec<(String, Option<i64>, Vec<f32>)>)>;
static EMBED_CACHE: Mutex<Option<HashMap<i64, EmbedCacheEntry>>> = Mutex::new(None);

/// 语义召回置信度默认阈值（Phase 0 实测：相关命中集中 0.41~0.46，非相关背景值更低）
const SEMANTIC_MIN_SIM_DEFAULT: f64 = 0.30;
/// 语义召回硬上限（极端大库保护，按余弦降序截断）
const SEMANTIC_MAX_CANDIDATES: usize = 2000;
/// RRF 融合常数（标准取值 60）
const RRF_K: f64 = 60.0;

/// 查询文本 → 语义召回列表（余弦 ≥ min_similarity，降序）
///
/// 失败（服务未就绪/编码失败）返回 Err，调用方静默降级纯关键词。
async fn semantic_recall(
    keyword: &str,
    min_similarity: f64,
    user_id: i64,
    state: &tauri::State<'_, AppState>,
    app: &tauri::AppHandle,
) -> Result<Vec<(crate::db::SmartHit, f64)>, String> {
    // 退避窗口内直接降级（避免确定性失败反复拉起服务 + 等超时）
    if crate::vision::semantic_backoff_active() {
        return Err("CLIP 服务暂不可用（退避中）".into());
    }
    // 直接编码：内部走「宽松就绪」（拉起服务但不等主链路模型），
    // /embed_text 会触发 CLIP 懒加载——重启后首次搜索也能拿到向量（不再是必须
    // 先做一次语义扫描才能搜索）。失败才标记退避。
    let q = match crate::vision::embed_text_query(keyword, app).await {
        Ok(q) => {
            crate::vision::clear_semantic_down();
            q
        }
        Err(e) => {
            crate::vision::mark_semantic_down();
            return Err(e);
        }
    };
    let mut q = q;
    let qnorm = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    if qnorm > 1e-6 {
        for x in q.iter_mut() {
            *x /= qnorm;
        }
    }

    // 全库向量（版本变化懒重载；锁内不做 IO，db 读写均在 EMBED_CACHE 锁外）
    let version = {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        db.embedding_version(user_id).map_err(|e| format!("{e}"))?
    };
    let cached: Option<EmbedCacheEntry> = {
        let cache = EMBED_CACHE.lock().map_err(|e| format!("{e}"))?;
        cache
            .as_ref()
            .and_then(|m| m.get(&user_id))
            .filter(|e| e.0 == version)
            .cloned()
    };
    let entry: EmbedCacheEntry = match cached {
        Some(e) => e,
        None => {
            let list = {
                let db = state.0.lock().map_err(|e| format!("{e}"))?;
                db.load_all_embeddings(user_id).map_err(|e| format!("{e}"))?
            };
            let e: EmbedCacheEntry = Arc::new((version, list));
            if let Ok(mut cache) = EMBED_CACHE.lock() {
                if let Some(map) = cache.as_mut() {
                    map.insert(user_id, e.clone());
                }
            }
            e
        }
    };
    continue_recall(entry, &q, min_similarity, user_id, state).await
}

/// 余弦过滤（≥ 阈值，降序，硬上限）→ hash 批量取展示字段
async fn continue_recall(
    entry: EmbedCacheEntry,
    q: &[f32],
    min_similarity: f64,
    user_id: i64,
    state: &tauri::State<'_, AppState>,
) -> Result<Vec<(crate::db::SmartHit, f64)>, String> {
    let (_version, list) = entry.as_ref();
    let mut scored: Vec<(f64, &String)> = list
        .iter()
        .filter_map(|(hash, _album, v)| {
            if v.len() != q.len() {
                return None;
            }
            let dot = v.iter().zip(q.iter()).map(|(a, b)| (*a * *b) as f64).sum::<f64>();
            if dot >= min_similarity {
                Some((dot, hash))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(SEMANTIC_MAX_CANDIDATES);

    let hashes: Vec<String> = scored.iter().map(|(_, h)| (*h).clone()).collect();
    let hits = {
        let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
        db.lookup_hits_by_hashes(user_id, &hashes)
            .map_err(|e| format!("{e}"))?
    };
    let cos_by_hash: HashMap<&String, f64> = scored.iter().map(|(s, h)| (&**h, *s)).collect();
    // lookup_hits_by_hashes 按 hashes 顺序返回，zip 后直接取对应余弦
    let mut out: Vec<(crate::db::SmartHit, f64)> = hits
        .into_iter()
        .zip(hashes.iter())
        .filter_map(|(h, hash)| {
            let cos = cos_by_hash.get(hash).copied()?;
            Some((h, cos))
        })
        .collect();
    // 保证余弦降序（与召回榜同序）
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

/// RRF 融合：FTS 榜 + 语义榜（K=60），语义命中附加 semantic_score
fn fuse_hits(fts: Vec<crate::db::SmartHit>, sem: Vec<(crate::db::SmartHit, f64)>) -> Vec<crate::db::SmartHit> {
    let mut score: HashMap<String, f64> = HashMap::new();
    let mut rank: HashMap<String, usize> = HashMap::new();
    let mut by_path: HashMap<String, (crate::db::SmartHit, Option<f64>)> = HashMap::new();
    for (i, h) in fts.into_iter().enumerate() {
        *score.entry(h.path.clone()).or_default() += 1.0 / (RRF_K + i as f64 + 1.0);
        rank.insert(h.path.clone(), i);
        by_path.insert(h.path.clone(), (h, None));
    }
    for (i, (h, cos)) in sem.into_iter().enumerate() {
        *score.entry(h.path.clone()).or_default() += 1.0 / (RRF_K + i as f64 + 1.0);
        rank.entry(h.path.clone()).or_insert(usize::MAX);
        let e = by_path
            .entry(h.path.clone())
            .or_insert((h, None));
        if e.1.is_none() {
            e.1 = Some(cos);
        }
    }
    let mut merged: Vec<(f64, usize, crate::db::SmartHit)> = score
        .into_iter()
        .map(|(path, s)| {
            let (mut h, sem_score) = by_path.remove(&path).expect("path 已登记");
            h.semantic_score = sem_score;
            (s, rank.get(&path).copied().unwrap_or(usize::MAX), h)
        })
        .collect();
    merged.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    merged.into_iter().map(|(_, _, h)| h).collect()
}

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
            // BUG-2026-0909-001：识别失败被静默跳过（损坏文件每次扫描都缺、
            // 且换相册归属也无法解释），留痕到日志方便事后定位
            // 「已入库 N/M 差额」到底缺了哪几张、为什么缺。
            crate::logger::log_info(&format!(
                "[scan] 识别失败跳过: {} ({})",
                r.path,
                r.error.as_deref().unwrap_or("未知错误")
            ));
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

/// FEAT-044：扫描入库成功后预热本批缩略图，让智慧相册子页面首屏 0 IO 命中。
///
/// 与 lib.rs `prewarm_thumbs` 独立：这里不返回计数（入库主路径不希望预热失败
/// 拖到入库失败）；且本函数为 async 供扫描主路径调用。
async fn prewarm_thumbs_after_scan(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    album_id: i64,
    user_id: i64,
    paths: &[String],
) -> Result<(), String> {
    use std::collections::HashMap;
    use std::sync::Arc;

    let thumbs_dir = match crate::thumbs_dir(app) {
        Ok(t) => t,
        Err(_) => return Ok(()), // 拿不到 thumbs 目录 → 静默跳过，不影响入库
    };

    // 1. 主流程加锁查表（短锁）
    let hit_map_outer: Arc<HashMap<String, String>> = {
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

    // 2. spawn_blocking 内走生成
    #[derive(Default)]
    struct GenBuf(std::sync::Mutex<Vec<(String, String, u64, u128)>>);
    let gen_buf = Arc::new(GenBuf::default());
    let gen_buf_for_cb = gen_buf.clone();
    let hit_for_cb = hit_map_outer.clone();
    let paths_owned: Vec<String> = paths.to_vec();
    let thumbs_dir_clone = thumbs_dir.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        crate::thumbnail::ensure_grid_thumbs_with_lookup(
            album_id,
            &paths_owned,
            &thumbs_dir_clone,
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

    // 3. 写表（主流程加锁）
    let items = gen_buf.0.lock().map_err(|e| format!("{:?}", e))?;
    if !items.is_empty() {
        let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
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
        let _ = db.upsert_thumb_caches(&recs); // 写表失败不阻塞
    }
    Ok(())
}

/// 语义向量扫描（FEAT-SEM）：相册内照片 → 缩略图 → CLIP 编码 → photo_embeddings 落库
///
/// 流程：收集照片（与 AI 分支同规则）→ 缩略图映射（查表优先，缺失现场生成）→
/// 增量差集（跳过已有向量）→ 分批 `vision::embed_images_batch`（fp16，batch 可选默认 8）→
/// 500/批事务 upsert → 返回报告与明细行（供前端统一表格展示）。
#[allow(clippy::too_many_arguments)]
async fn scan_album_embeddings(
    album_id: i64,
    user_id: i64,
    dir: &str,
    batch_size: usize,
    overwrite: bool,
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(ScanReport, Vec<UnifiedScanRow>), String> {
    use std::collections::HashMap;

    use crate::db::{embedding::EmbeddingRecord, DbError};

    let photos = crate::vision::collect_images(dir)?;
    if photos.is_empty() {
        return Ok((ScanReport { total: 0, written: 0, failed: 0 }, Vec::new()));
    }
    let thumbs_dir = crate::thumbs_dir(app)?;


    // 1. 短锁查表：源图路径 → (hash, thumb_path)
    let mut meta: Vec<(String, String, String)> = Vec::with_capacity(photos.len()); // (source, hash, thumb)
    let mut missing: Vec<(String, String)> = Vec::new(); // (source, hash)
    {
        let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
        let hashes: Vec<String> = photos
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
        let hit_map: HashMap<String, String> = db
            .lookup_thumb_caches(&hashes)
            .unwrap_or_default()
            .into_iter()
            .filter(|h| !h.thumb_path.is_empty() && std::path::Path::new(&h.thumb_path).is_file())
            .map(|h| (h.photo_hash, h.thumb_path))
            .collect();
        for (p, h) in photos.iter().zip(hashes.iter()) {
            match hit_map.get(h) {
                Some(tp) => meta.push((p.clone(), h.clone(), tp.clone())),
                None => missing.push((p.clone(), h.clone())),
            }
        }
    }

    // 2. 缺失缩略图现场生成（阻塞线程，db=None 只生成文件不写表；写表随向量批次一并完成）
    if !missing.is_empty() {
        let thumbs_dir2 = thumbs_dir.clone();
        let missing2 = missing.clone();
        let generated: Vec<(String, String, String)> = tauri::async_runtime::spawn_blocking(move || {
            missing2
                .iter()
                .filter_map(|(src, hash)| {
                    match crate::thumbnail::ensure_grid_thumb(
                        album_id,
                        std::path::Path::new(src),
                        &thumbs_dir2,
                        None,
                        user_id,
                    ) {
                        Ok(tp) => Some((src.clone(), hash.clone(), tp)),
                        Err(_) => None,
                    }
                })
                .collect()
        })
        .await
        .map_err(|e| format!("缩略图生成任务失败: {e}"))?;
        meta.extend(generated);
    }
    if meta.is_empty() {
        return Ok((ScanReport { total: photos.len(), written: 0, failed: photos.len() }, Vec::new()));
    }

    // 3. 增量差集：已有向量的照片跳过（重复扫描秒级完成）
    let all_hashes: Vec<String> = meta.iter().map(|(_, h, _)| h.clone()).collect();
    let existing: std::collections::HashSet<String> = {
        let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
        db.lookup_embedding_hashes(&all_hashes).unwrap_or_default()
    };
    let meta_len = meta.len();
    // 覆盖模式：全部重算（已有向量被 upsert 覆盖）；增量模式：跳过已有
    let todo: Vec<(String, String, String)> = meta
        .into_iter()
        .filter(|(_, h, _)| overwrite || !existing.contains(h))
        .collect();
    let _skipped = photos.len() - todo.len();

    // 4. 分批编码（进度事件由 embed_images_batch 内部 emit "embed-progress"）
    let thumb_to_src: HashMap<String, (String, String)> = todo
        .iter()
        .map(|(src, h, tp)| (tp.clone(), (src.clone(), h.clone())))
        .collect();
    let thumb_paths: Vec<String> = todo.iter().map(|(_, _, tp)| tp.clone()).collect();
    let results = crate::vision::embed_images_batch(&thumb_paths, batch_size, app, Some(cancel)).await?;

    // 5. 500/批事务写库（f32 小端 BLOB，服务端已归一化）
    let model = "chinese-clip-vit-b16-fp16";
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default();
    let mut written = 0usize;
    let mut buf: Vec<EmbeddingRecord> = Vec::new();
    let mut rows: Vec<UnifiedScanRow> = Vec::new();
    let flush = |buf: &Vec<EmbeddingRecord>, written: &mut usize| -> Result<(), String> {
        if buf.is_empty() {
            return Ok(());
        }
        let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
        db.upsert_embeddings(buf).map_err(|e: DbError| format!("向量写入失败: {e}"))?;
        *written += buf.len();
        Ok(())
    };
    for r in &results {
        if let (Some(vec), Some((src, hash))) = (&r.embedding, thumb_to_src.get(&r.path)) {
            buf.push(EmbeddingRecord {
                photo_hash: hash.clone(),
                user_id,
                album_id: Some(album_id),
                path: src.clone(),
                dim: vec.len() as i64,
                model: model.to_string(),
                embedding: vec.clone(),
                generated_at: now.clone(),
            });
            rows.push(UnifiedScanRow {
                file_name: std::path::Path::new(src)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: src.clone(),
                iso: None, aperture: None, shutter_speed: None, focal_length: None,
                shoot_time: None, iso_num: None, focal_num: None, aperture_num: None,
                shutter_num: None, tone_type: None, avg_luma: None,
                category: None, sub_category: None, label: None, confidence: None,
                top3: Vec::new(), person_ids: Vec::new(), person_count: 0,
            });
            if buf.len() >= 500 {
                flush(&buf, &mut written)?;
                buf.clear();
            }
        }
    }
    flush(&buf, &mut written)?;
    let failed = results.iter().filter(|r| r.embedding.is_none()).count() + (photos.len() - meta_len);

    Ok((
        ScanReport { total: photos.len(), written, failed },
        rows,
    ))
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
        // FEAT-044：扫描入库完成后预热缩略图（为智慧相册子页面提供首屏 0 IO 命中）
        // 独立 try，避免预热失败不影响入库结果。
        if upsert.is_ok() {
            let paths: Vec<String> = recs.iter().map(|r| r.path.clone()).collect();
            if !paths.is_empty() {
                let _ = prewarm_thumbs_after_scan(&app, &state, album_id, user_id, &paths).await;
            }
        }
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

    /// FEAT-D：确保单张照片已扫描并落库（用于大图查看器点击原图时）
    ///
    /// 行为：
    /// 1. 若 photo_content_scan 中已有该 path 的记录 → 直接返回（复用已有信息）。
    /// 2. 否则 → 调 vision::classify_album 识别该单张图片（最小扫描开销），
    ///    提取 EXIF + 计算哈希 → 落库 → 返回单条 AlbumContentRow。
    /// 3. 照片不存在 / 识别失败 → 返回 None（让前端继续展示原图）。
    ///
    /// 与 scan_album_content 区别：仅扫描 1 张（最小 IO），用于“打开原图时静默补全”。
    /// 多用户隔离：限定 user_id。
    #[tauri::command]
    pub async fn ensure_photo_scanned(
        album_id: i64,
        path: String,
        app: tauri::AppHandle,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Option<db::AlbumContentRow>, String> {
        let _t = log_call!("ensure_photo_scanned", &format!("album_id={album_id} path={path}"));
        let user_id = require_user(&session)?;

        // 1. 先查表：已有就直接返回
        let existing = (|| -> Result<Option<db::AlbumContentRow>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.get_photo_content_by_path(&path, user_id)
                .map_err(|e| format!("{:?}", e))
        })()?;
        if let Some(row) = existing {
            logger::log_call_end_with("ensure_photo_scanned", _t, "HIT | from_db");
            return Ok(Some(row));
        }

        // 2. 没有 → 识别单张图片
        let result = crate::vision::classify_single(&path, &app).await?;

        // 3. 构建记录（哈希 + EXIF + 拍摄时间 + 人物）
        let app2 = app.clone();
        let recs = tauri::async_runtime::spawn_blocking(move || {
            build_records(album_id, user_id, std::slice::from_ref(&result), &app2)
        })
        .await
        .map_err(|e| format!("任务线程失败: {e}"))??;

        let wrote = (|| -> Result<usize, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.upsert_photo_contents(&recs).map_err(|e| format!("{:?}", e))?;
            Ok(recs.len())
        })()?;

        // 4. 再读一次返回（拿 AlbumContentRow 全字段）
        let out = (|| -> Result<Option<db::AlbumContentRow>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.get_photo_content_by_path(&path, user_id)
                .map_err(|e| format!("{:?}", e))
        })()?;

        logger::log_call_end_with(
            "ensure_photo_scanned",
            _t,
            &format!("SCAN | wrote={wrote} returned={}", out.is_some()),
        );
        Ok(out)
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
        // FEAT-SEM：覆盖/增量。true（默认）= 全量重扫并覆盖已入库结果；
        // false = 跳过已入库照片，只处理新照片
        overwrite: Option<bool>,
        app: tauri::AppHandle,
        scan: tauri::State<'_, crate::ScanState>,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<CombinedScanOutcome, String> {
        let overwrite = overwrite.unwrap_or(true);
        let _t = log_call!("scan_album_combined", &format!("album_id={album_id} scan_types={scan_types:?} overwrite={overwrite}"));
        let user_id = require_user(&session)?;

        if scan_types.is_empty() {
            return Err("scan_types 不能为空".to_string());
        }
        if !scan_types
            .iter()
            .all(|s| ["basic", "tone", "ai", "semantic"].contains(&s.as_str()))
        {
            return Err("非法 scan_types，允许 basic / tone / ai / semantic".to_string());
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
        // FEAT-SEM：语义向量扫描（独立于 AI 分支，缩略图直编码，增量跳过已有向量）
        let do_semantic = scan_types.contains(&"semantic".to_string());
        let batch = batch_size.unwrap_or(8).clamp(4, 64) as usize;

        // FEAT-SEM：语义向量扫描与 AI 识别同为异步 HTTP，在 outcome 构建前完成，
        // 结果合并进 CombinedScanOutcome（report 累加，rows 追加）
        let semantic_outcome: Option<Result<(ScanReport, Vec<UnifiedScanRow>), String>> = if do_semantic {
            Some(
                scan_album_embeddings(
                    album_id,
                    user_id,
                    &dir,
                    batch,
                    overwrite,
                    &app,
                    &state,
                    cancel.clone(),
                )
                .await,
            )
        } else {
            None
        };

        // AI 识别为异步 HTTP（在异步运行时上每批检查取消标记），不占用主线程
        let vision_results = if do_ai {
            // FEAT-SEM：增量模式跳过已入库照片（识别是最重的一步，跳过可省大部分耗时）
            let photos = crate::vision::collect_images(&dir)?;
            let paths: Vec<String> = if overwrite {
                photos
            } else {
                let (path_hash_pairs, scanned) = {
                    let hashes: Vec<(String, String)> = photos
                        .iter()
                        .filter_map(|p| {
                            let path = Path::new(p);
                            let (len, mtime) = std::fs::metadata(path).ok().map(|md| {
                                (
                                    md.len(),
                                    md.modified()
                                        .ok()
                                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                                        .map(|d| d.as_nanos())
                                        .unwrap_or(0),
                                )
                            })?;
                            Some((p.clone(), photo_hash(p, len, mtime)))
                        })
                        .collect();
                    let scanned = {
                        let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
                        db.lookup_scanned_hashes_by_album(album_id)
                            .map_err(|e| format!("{e}"))?
                    };
                    (hashes, scanned)
                };
                let kept: Vec<String> = path_hash_pairs
                    .into_iter()
                    .filter(|(_, h)| !scanned.contains(h))
                    .map(|(p, _)| p)
                    .collect();
                logger::log_info(&format!(
                    "[scan] 增量模式：{}/{} 张待识别（跳过已入库 {} 张）",
                    kept.len(),
                    photos.len(),
                    photos.len() - kept.len()
                ));
                kept
            };
            crate::vision::classify_paths(&paths, batch, &app, Some(cancel.clone())).await?
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

        let mut outcome: Result<CombinedScanOutcome, String> = (|| -> Result<CombinedScanOutcome, String> {
            if do_ai {
                let tone_ref = if do_tone { Some(&tones) } else { None };
                let (recs, rows) =
                    build_records_combined(album_id, user_id, &vision_results, tone_ref, &app)?;
                let written_count = recs.len();
                {
                    let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
                    db.upsert_photo_contents(&recs).map_err(|e| format!("{:?}", e))?;
                }
                // FEAT-044：组合扫描入库完成后预热缩略图。
                // recs 准备返回外层用于 in-progress 预热——避免闭包生命周期问题
                // 在这里直接持有 recs paths，outcome 返回后在外层调预热。
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

        // FEAT-SEM：合并语义向量扫描结果；语义子任务失败则整体报错（用户明确勾选，
        // 部分成功无意义——向量未入库，搜索仍搜不到）
        match semantic_outcome {
            Some(Err(e)) => outcome = Err(format!("语义扫描失败: {e}")),
            Some(Ok((report, rows))) => {
                if let Ok(o) = outcome.as_mut() {
                    o.report.total += report.total;
                    o.report.written += report.written;
                    o.report.failed += report.failed;
                    o.rows.extend(rows);
                }
            }
            None => {}
        }

        match &outcome {
            Ok(o) => logger::log_call_end_with(
                "scan_album_combined",
                _t,
                &format!("OK | total={} written={}", o.report.total, o.report.written),
            ),
            Err(e) => logger::log_call_end_with("scan_album_combined", _t, &format!("ERR | {e}")),
        }
        // FEAT-044：组合扫描入库成功后预热缩略图（仅 do_ai 路径写库，prewarm 也只对 do_ai 生效）
        if do_ai && outcome.is_ok() {
            let prewarm_paths: Vec<String> = vision_results
                .iter()
                .filter(|r| r.error.is_none())
                .map(|r| r.path.clone())
                .collect();
            if !prewarm_paths.is_empty() {
                let _ = prewarm_thumbs_after_scan(&app, &state, album_id, user_id, &prewarm_paths).await;
            }
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

    /// 跨相册照片时间线（FEAT-033）：返回当前用户全部已扫描照片按时间升序
    #[tauri::command]
    pub async fn list_timeline(
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<db::ContentSearchHit>, String> {
        let _t = log_call!("list_timeline", "");
        let user_id = require_user(&session)?;

        let r = (|| -> Result<Vec<db::ContentSearchHit>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.list_timeline(user_id).map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok(list) => logger::log_call_end_with("list_timeline", _t, &format!("OK | rows={}", list.len())),
            Err(e) => logger::log_call_end_with("list_timeline", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 内容分类两级聚合（FEAT-048）：大类 → 细类计数 + 各大类封面（置信度最高）
    #[tauri::command]
    pub async fn list_content_categories(
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<db::CategoryGroupRow>, String> {
        let _t = log_call!("list_content_categories", "");
        let user_id = require_user(&session)?;
        let r = (|| -> Result<Vec<db::CategoryGroupRow>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.list_content_categories(user_id)
                .map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok(list) => logger::log_call_end_with(
                "list_content_categories",
                _t,
                &format!("OK | groups={}", list.len()),
            ),
            Err(e) => logger::log_call_end_with("list_content_categories", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 按大类/细类列出照片（FEAT-048 分类浏览二级视图）
    #[tauri::command]
    pub async fn list_photos_by_category(
        category: String,
        sub_category: Option<String>,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<db::ContentSearchHit>, String> {
        let _t = log_call!(
            "list_photos_by_category",
            &format!("category={category} sub={sub_category:?}")
        );
        let user_id = require_user(&session)?;
        let r = (|| -> Result<Vec<db::ContentSearchHit>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.list_photos_by_category(user_id, &category, sub_category.as_deref())
                .map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok(list) => logger::log_call_end_with(
                "list_photos_by_category",
                _t,
                &format!("OK | rows={}", list.len()),
            ),
            Err(e) => logger::log_call_end_with("list_photos_by_category", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 地点聚合（FEAT-049）：geo_index 离线反查 + 回写 location 缓存
    #[tauri::command]
    pub async fn list_photo_locations(
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<db::LocationGroupRow>, String> {
        let _t = log_call!("list_photo_locations", "");
        let user_id = require_user(&session)?;
        let r = (|| -> Result<Vec<db::LocationGroupRow>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.list_photo_locations(user_id)
                .map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok(list) => logger::log_call_end_with(
                "list_photo_locations",
                _t,
                &format!(
                    "OK | groups={} named_photos={}",
                    list.len(),
                    list.iter().filter(|g| g.location.is_some()).map(|g| g.count).sum::<i64>()
                ),
            ),
            Err(e) => logger::log_call_end_with("list_photo_locations", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 按地点列出照片（FEAT-049 地点浏览二级视图；None = 未记录地点组）
    #[tauri::command]
    pub async fn list_photos_by_location(
        location: Option<String>,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<db::ContentSearchHit>, String> {
        let _t = log_call!("list_photos_by_location", &format!("location={location:?}"));
        let user_id = require_user(&session)?;
        let r = (|| -> Result<Vec<db::ContentSearchHit>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.list_photos_by_location(user_id, location.as_deref())
                .map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok(list) => logger::log_call_end_with(
                "list_photos_by_location",
                _t,
                &format!("OK | rows={}", list.len()),
            ),
            Err(e) => logger::log_call_end_with("list_photos_by_location", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 设置照片用户标签（FEAT-050 覆盖式保存；返回规范化后的标签列表）
    ///
    /// 评分走既有 photo_ratings 体系（set_photo_rating / get_photo_ratings）。
    #[tauri::command]
    pub async fn set_photo_tags(
        path: String,
        tags: Vec<String>,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<String>, String> {
        let _t = log_call!("set_photo_tags", &format!("path={path} tags={}", tags.len()));
        let user_id = require_user(&session)?;
        let r = (|| -> Result<Vec<String>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.set_photo_tags(user_id, &path, &tags)
                .map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok(list) => logger::log_call_end_with("set_photo_tags", _t, &format!("OK | tags={}", list.len())),
            Err(e) => logger::log_call_end_with("set_photo_tags", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 读取照片用户标签（FEAT-050；无记录/无标签返回空数组）
    #[tauri::command]
    pub async fn get_photo_tags(
        path: String,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<String>, String> {
        let _t = log_call!("get_photo_tags", &format!("path={path}"));
        let user_id = require_user(&session)?;
        let r = (|| -> Result<Vec<String>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.get_photo_tags(user_id, &path)
                .map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok(list) => logger::log_call_end_with("get_photo_tags", _t, &format!("OK | tags={}", list.len())),
            Err(e) => logger::log_call_end_with("get_photo_tags", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 智能搜索（FEAT-034）：关键词宽匹配 + 多维筛选，跨相册
    #[allow(clippy::too_many_arguments)]
    #[tauri::command]
    pub async fn smart_search(
        keyword: String,
        date_from: Option<String>,
        date_to: Option<String>,
        location: Option<String>,
        category: Option<String>,
        label: Option<String>,
        person_id: Option<String>,
        tone_type: Option<String>,
        // FEAT-SEM：语义融合开关与置信度下限（默认开启 / 0.30）
        include_semantic: Option<bool>,
        min_similarity: Option<f64>,
        app: tauri::AppHandle,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<db::SmartHit>, String> {
        let _t = log_call!("smart_search", &format!("keyword={keyword}"));
        let user_id = require_user(&session)?;

        let r = (|| -> Result<Vec<db::SmartHit>, String> {
            let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
            db.smart_search(
                user_id,
                &keyword,
                date_from.as_deref(),
                date_to.as_deref(),
                location.as_deref(),
                category.as_deref(),
                label.as_deref(),
                person_id.as_deref(),
                tone_type.as_deref(),
            )
            .map_err(|e| format!("{:?}", e))
        })();
        match &r {
            Ok(list) => {
                logger::log_call_end_with("smart_search", _t, &format!("OK | hits={}", list.len()))
            }
            Err(e) => logger::log_call_end_with("smart_search", _t, &format!("ERR | {e}")),
        }
        let mut hits = r?;

        // FEAT-SEM：语义融合（keyword 非空 + 未显式关闭 + CLIP 已就绪）
        // 任何环节不可用 → 静默降级纯关键词结果，对用户无感
        let kw = keyword.trim().to_string();
        if !kw.is_empty() && include_semantic != Some(false) {
            let min_sim = min_similarity
                .unwrap_or(SEMANTIC_MIN_SIM_DEFAULT)
                .clamp(0.05, 0.95);
            match semantic_recall(&kw, min_sim, user_id, &state, &app).await {
                Ok(sem) if !sem.is_empty() => {
                    let n_sem = sem.len();
                    hits = fuse_hits(hits, sem);
                    logger::log_call_end_with(
                        "smart_search.semantic",
                        _t,
                        &format!("OK | 语义命中 {n_sem}，融合后 {}", hits.len()),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    // 降级是预期行为（模型未下载/服务未启动/向量库为空），不打扰用户
                    eprintln!("[smart_search] 语义降级: {e}");
                }
            }
        }
        Ok(hits)
    }

    /// FEAT-SEM：语义服务预热（搜索页进入时后台调用，fire-and-forget）
    ///
    /// 背景：重启应用后 VCR 服务未运行、CLIP 会话未加载，若等用户真正搜索时才做
    /// 会多等数秒。此处提前预热。模型未下载 → 返回 false（不拉起无谓进程）；
    /// 退避窗口内 → 跳过。
    #[tauri::command]
    pub async fn warmup_semantic_service(app: tauri::AppHandle) -> Result<bool, String> {
        if !crate::vision::clip_model_present() || crate::vision::semantic_backoff_active() {
            return Ok(false);
        }
        let _t = log_call!("warmup_semantic_service", "");
        match crate::vision::warmup_clip(&app).await {
            Ok(()) => {
                logger::log_call_end_with("warmup_semantic_service", _t, "OK");
                Ok(true)
            }
            Err(e) => {
                logger::log_call_end_with("warmup_semantic_service", _t, &format!("ERR | {e}"));
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod semantic_tests {
    use super::fuse_hits;
    use crate::db::SmartHit;

    fn hit(path: &str) -> SmartHit {
        SmartHit {
            id: 0,
            path: path.into(),
            album_id: None,
            album_name: None,
            category: None,
            sub_category: None,
            label: None,
            location: None,
            shoot_time: None,
            tone_type: None,
            person_ids: vec![],
            semantic_score: None,
        }
    }

    #[test]
    fn rrf_fuse_ranks_dual_hits_first_and_keeps_semantic_order() {
        // c.jpg 仅 FTS 命中（第 1 位）、a.jpg 双命中、b.jpg 仅语义命中（余弦最高）
        let fts = vec![hit("c.jpg"), hit("a.jpg")];
        let sem = vec![(hit("b.jpg"), 0.46), (hit("a.jpg"), 0.44)];
        let out = fuse_hits(fts, sem);
        let paths: Vec<&str> = out.iter().map(|h| h.path.as_str()).collect();
        // a.jpg 双榜命中 RRF 分最高（2/62）；b 与 c 同为 1/61 同分，
        // tie-break 规则：FTS 原生顺序优先于语义-only（保住关键词用户预期）
        assert_eq!(paths, vec!["a.jpg", "c.jpg", "b.jpg"]);
        // 语义命中附加余弦徽标；纯关键词命中不带
        let a = out.iter().find(|h| h.path == "a.jpg").unwrap();
        assert!((a.semantic_score.unwrap() - 0.44).abs() < 1e-9);
        let c = out.iter().find(|h| h.path == "c.jpg").unwrap();
        assert!(c.semantic_score.is_none());
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
