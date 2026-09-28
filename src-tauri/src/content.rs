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
type EmbedCacheEntry = Arc<(crate::db::EmbeddingVersion, Vec<(String, Option<i64>, String, Vec<f32>)>)>;
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
            // 只有确定性失败（模型缺失 / 端口不通 / 组件过期）才进退避；
            // 「服务未就绪·加载中」这类可重试型失败只跳过本次——否则重启后 CLIP
            // 首次加载的一次 503 会把之后数十秒内所有搜索都锁成纯关键词，
            // 表现正是「向量在库里却搜不出来」（BUG-2026-0920-008）
            crate::vision::note_semantic_failure(&e);
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
    // v5：按当前语义档位模型过滤 —— 换档后旧向量维度/空间不同，绝不能混用
    let model = crate::vision::clip_model_id(app).await?;
    let version = {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        db.embedding_version(user_id, &model).map_err(|e| format!("{e}"))?
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
                db.load_all_embeddings(user_id, &model).map_err(|e| format!("{e}"))?
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
        .filter_map(|(hash, _album, _path, v)| {
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
    fuse_hits3(fts, sem, Vec::new())
}

/// FEAT-067 步骤 5：三路 RRF（FTS + 图像向量语义 + 描述向量语义）
///
/// 只是在既有两路融合上**加一路**，融合公式与权重完全复用（同 K、同 tie-break）。
/// `desc` 传空列表时与 `fuse_hits` 行为完全一致（既有语义未回退）。
fn fuse_hits3(
    fts: Vec<crate::db::SmartHit>,
    img: Vec<(crate::db::SmartHit, f64)>,
    desc: Vec<(crate::db::SmartHit, f64)>,
) -> Vec<crate::db::SmartHit> {
    let mut score: HashMap<String, f64> = HashMap::new();
    let mut rank: HashMap<String, usize> = HashMap::new();
    let mut by_path: HashMap<String, (crate::db::SmartHit, Option<f64>)> = HashMap::new();
    for (i, h) in fts.into_iter().enumerate() {
        *score.entry(h.path.clone()).or_default() += 1.0 / (RRF_K + i as f64 + 1.0);
        rank.insert(h.path.clone(), i);
        by_path.insert(h.path.clone(), (h, None));
    }
    for lane in [img, desc] {
        for (i, (h, cos)) in lane.into_iter().enumerate() {
            *score.entry(h.path.clone()).or_default() += 1.0 / (RRF_K + i as f64 + 1.0);
            rank.entry(h.path.clone()).or_insert(usize::MAX);
            let e = by_path.entry(h.path.clone()).or_insert((h, None));
            // 两路语义都命中时取较高者（前端只展示一个「AI 匹配」徽标）
            e.1 = Some(match e.1 {
                Some(c) => c.max(cos),
                None => cos,
            });
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
///
/// ⚠️ 字段口径（BUG-2026-0922-008）：
///
/// `total` 曾经是「本次处理/识别的张数」。增量差集引入后，这个含义在
/// 「已入库相册」上会退化成 0（差集为空），而前端表格按「共 N 张」展示 ——
/// 于是全局扫描完毕后，十几个明明有照片的相册全部显示「共 0 张」。
///
/// 现在把三件事拆开，各自只有一个含义，**不再复用**：
/// - `total`：**该相册目录内的图片总数**（不随增量模式变化，是稳定的「相册有多大」）
/// - `processed`：本次真正处理（识别/解码/写库）的张数 = 差集大小
/// - `skipped`：因已入库且文件未变化而跳过的张数
///
/// 恒等式（增量与全量均成立）：`total == processed + skipped`
///
/// 保留 `total` 为「目录总数」而非改名，是为了不破坏既有前端消费点；
/// `skipped` 用 `#[serde(default)]` 以兼容仅传三个字段的旧载荷。
#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    /// 该相册目录内的图片总数（**不是**本次处理数）
    pub total: usize,
    /// 成功写入/更新的记录数
    pub written: usize,
    /// 识别失败（未落库）数
    pub failed: usize,
    /// 本次真正处理（进入识别/解码/写库）的张数 = 增量差集大小
    pub processed: usize,
    /// 因已入库且文件未变化而跳过的张数
    #[serde(default)]
    pub skipped: usize,
}

impl ScanReport {
    /// 由「相册目录总数 + 本次处理数」构造报告（其余字段各自传入）。
    ///
    /// 抽成构造函数而不是让每个分支各自手写字面量：上一轮的 bug 正是
    /// 「AI 分支填差集、非 AI 分支填写入数」这类**各写各的**造成的语义漂移。
    /// 统一入口后，`total`/`processed`/`skipped` 的三角关系只有一处实现，
    /// 单测可以直接断言恒等式（见 `scan_report_identity_holds`）。
    pub(crate) fn new(
        dir_total: usize,
        processed: usize,
        written: usize,
        failed: usize,
    ) -> ScanReport {
        // clamped：`processed` 由各分支独立统计，理论上可能因「缩略图失败重算」
        // 等边角情况略大于目录总数。宁可钳到 0 也不要出现负数溢出（usize 减法会 panic）。
        let skipped = dir_total.saturating_sub(processed);
        ScanReport {
            total: dir_total,
            written,
            failed,
            processed,
            skipped,
        }
    }
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
        // FEAT-067：地点补齐（GPS → 离线行政区划反查）
        let place = resolve_location(&ex);

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
            location: place,
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
            // 本函数同时产出 AI 字段与 EXIF（影调不在此路径）→ 声明 AI+EXIF 所有权
            owns: crate::db::FieldGroups::AI_EXIF,
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
/// - **P4**：`exif_scan` 由调用方传入（第 1 步已读过的那一份），本函数不再自己
///   `read_photo_exif`。此前同一份 EXIF 被读两遍（第 1 步一遍、这里一遍），
///   而第 1 步读出的 `exifs` 在 AI 分支完全不参与 → 读了就丢，纯浪费。
///   现在 EXIF 也纳入增量差集，这个「一份数据两处读」必须先消除，
///   否则增量逻辑会建立在双读的错误前提上。
/// - 返回 `(records, unified_rows)`：records 供落库，unified_rows 供前端统一表格展示
#[allow(clippy::too_many_arguments)]
fn build_records_combined(
    album_id: i64,
    user_id: i64,
    results: &[crate::vision::VisionResult],
    tone_scan: Option<&Vec<crate::tone::PhotoTone>>,
    exif_scan: &[crate::photo_scan::PhotoExif],
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

    // P4：EXIF 也按路径建索引，复用第 1 步读出的那一份（不再逐张重读）。
    // 未命中时退化为「字段全空」的行 —— 与旧行为一致（旧代码读失败也是 None 字段），
    // 但省掉一次完整的 EXIF 解析。
    let exif_map: std::collections::HashMap<&str, &crate::photo_scan::PhotoExif> =
        exif_scan.iter().map(|e| (e.path.as_str(), e)).collect();

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
        // P4：优先复用第 1 步已读出的 EXIF；未命中才现读。
        // 正常路径下必然命中（esults\ 的路径来自同一次 walk），
        // 兜底现读只是保证「未勾 basic 时本分支仍能自给自足」。
        let ex_owned;
        let ex: &crate::photo_scan::PhotoExif = match exif_map.get(r.path.as_str()) {
            Some(e) => e,
            None => {
                ex_owned = crate::photo_scan::read_photo_exif(path, &name);
                &ex_owned
            }
        };
        // FEAT-067：地点补齐（GPS → 离线行政区划反查）
        let place = resolve_location(ex);

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
            location: place,
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
            // P2：AI 分支**必定**产出 AI 字段；
            // EXIF 每次都会读到（exif_map 命中或兜底现读）⇒ 归本次所有，可直接覆盖；
            // 影调只在本轮跑了才拥有 —— 未跑时 `tone_type/avg_luma` 恒为 None，
            // 若声明拥有就会把旧影调写成空（勾「只做 AI」时反而毁掉上次的影调结果）。
            owns: crate::db::FieldGroups {
                ai: true,
                exif: true,
                tone: tone_scan.is_some(),
            },
        });

        rows.push(UnifiedScanRow {
            file_name: r.file_name.clone(),
            path: r.path.clone(),
            iso: ex.iso.clone(),
            aperture: ex.aperture.clone(),
            shutter_speed: ex.shutter_speed.clone(),
            focal_length: ex.focal_length.clone(),
            shoot_time: ex.shoot_time.clone(),
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

/// FEAT-067 步骤 3：扫描期地点补齐
///
/// `photo_scan::read_photo_exif` 走的是快路径（不反查地名），`place` 恒为 None；
/// 这里补一步**离线**反查（GPS → 省/市，见 `geo_index`），让 `location` 不再只有
/// 个位数百分比的填充率。无网络依赖，未命中（国外/无 GPS）留 None。
///
/// 只负责「把数据搞到」，不做嵌入 —— 符合「扫描与嵌入分两步」的定稿（决策⑥）。
fn resolve_location(ex: &crate::photo_scan::PhotoExif) -> Option<String> {
    if let Some(p) = ex.place.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(p.to_string());
    }
    match (ex.lat, ex.lon) {
        (Some(lat), Some(lon)) => crate::geo_index::find_region(lat, lon),
        _ => None,
    }
}

/// 空字符串 → None（落库为 NULL），否则保留
fn opt_nonempty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// P2：非 AI 分支（只勾 EXIF / 影调）的记录构造 —— 让「做过」这件事留下证据。
///
/// 为什么必须有：增量扫描的**唯一判据**是 `photo_content_scan` 里有没有这一行。
/// 此前非 AI 分支扫完只在内存里拼 `UnifiedScanRow` 给前端看，一条也不落库
/// ⇒ ① 刷新/重进页面数据就没了（用户视角：白扫）② 下次扫描的差集算不出这一张
/// ⇒ 永远全量重做，增量功能对「只勾影调」的用户完全失效。
///
/// 字段填写纪律（**只看自己产出的那几个**）：
/// - 填：`path` / `parent_dir` / `album_id` / `user_id` / EXIF 全字段 / 影调两字段
/// - 留 `None`：`category` / `sub_category` / `label` / `confidence` / `top3_json` /
///   `person_ids` / `person_count` / `content`
///   —— 这些是 AI 分支的产出。留空不是偷懒，而是靠 `owns` 所有权声明来保护：
///   AI 字段组不归本次所有时，`upsert_one` 用 `COALESCE` 保留旧值，不动老标签。
///   若这里硬塞空串/0，会把该照片此前 AI 扫描的标签整片抹掉（见 `db/content.rs`
///   里 `upsert_one` 的长注释）。
///
/// `owns` 归属：纯影调路径 → 只拥有 `tone`；以 EXIF 为轴的路径 → 拥有 `exif`，
/// 影调看本轮是否命中。**AI 字段组恒为 `false`**。
///
/// 影调沿用与 AI 分支同一套匹配口径（按 `path` 索引），保证两条路径写出的
/// `tone_type` 字符串完全一致（都是 `format!("{:?}")`，即 `LowKey`/`MidKey`/`HighKey`）。
fn build_records_from_exif_tone(
    album_id: i64,
    user_id: i64,
    exifs: &[crate::photo_scan::PhotoExif],
    tones: &[crate::tone::PhotoTone],
) -> Vec<PhotoContentRecord> {
    let tone_map: HashMap<&str, &crate::tone::PhotoTone> =
        tones.iter().map(|t| (t.path.as_str(), t)).collect();

    // 只用 EXIF 列表作主轴（勾 basic 时它涵盖全部差集照片）。
    // 未勾 basic 只勾影调时，`exifs` 为空 → 改以影调列表为轴，保证影调结果是完整的。
    let mut recs: Vec<PhotoContentRecord> = Vec::with_capacity(exifs.len().max(tones.len()));

    if exifs.is_empty() {
        for t in tones {
            let path = Path::new(&t.path);
            let (len, mtime_ns) = stat_len_mtime(path);
            recs.push(PhotoContentRecord {
                photo_hash: photo_hash(&t.path, len, mtime_ns),
                path: t.path.clone(),
                parent_dir: parent_dir_of(path),
                album_id: Some(album_id),
                user_id,
                content: String::new(),
                category: None,
                sub_category: None,
                label: None,
                confidence: None,
                top3_json: None,
                person_ids: None,
                person_count: 0,
                shoot_time: None,
                location: None,
                shutter_speed: None,
                iso: None,
                aperture: None,
                focal_length: None,
                iso_num: None,
                focal_num: None,
                aperture_num: None,
                shutter_num: None,
                tone_type: t.tone_type.map(|e| format!("{:?}", e)),
                avg_luma: t.avg_luma,
                lat: None,
                lon: None,
                // 纯影调路径：AI 字段一概不碰（含 content），只拥有影调。
                // 这里 exifs 为空 ⇒ 没有 EXIF 可写，故 exif 也声明为「不拥有」，
                // 否则会把此前 basic 扫出的 EXIF 整片清空。
                owns: crate::db::FieldGroups { ai: false, exif: false, tone: true },
            });
        }
        return recs;
    }

    for ex in exifs {
        let path = Path::new(&ex.path);
        let (len, mtime_ns) = stat_len_mtime(path);
        let tone = tone_map.get(ex.path.as_str());
        recs.push(PhotoContentRecord {
            photo_hash: photo_hash(&ex.path, len, mtime_ns),
            path: ex.path.clone(),
            parent_dir: parent_dir_of(path),
            album_id: Some(album_id),
            user_id,
            content: String::new(),
            category: None,
            sub_category: None,
            label: None,
            confidence: None,
            top3_json: None,
            person_ids: None,
            person_count: 0,
            shoot_time: ex.shoot_time.clone(),
            location: resolve_location(ex),
            shutter_speed: ex.shutter_speed.clone(),
            iso: ex.iso.clone(),
            aperture: ex.aperture.clone(),
            focal_length: ex.focal_length.clone(),
            iso_num: ex.iso_num,
            focal_num: ex.focal_num,
            aperture_num: ex.aperture_num,
            shutter_num: ex.shutter_num,
            tone_type: tone.and_then(|t| t.tone_type.map(|e| format!("{:?}", e))),
            avg_luma: tone.and_then(|t| t.avg_luma),
            lat: ex.lat,
            lon: ex.lon,
            // 以 EXIF 为轴的路径：EXIF 全字段直接覆盖；
            // 影调仅在命中（本轮影调结果里有这一张）时才拥有 ——
            // 未勾影调 / 未命中时 `tone_type` 为 None，不拥有才不会误清旧值。
            owns: crate::db::FieldGroups {
                ai: false,
                exif: true,
                tone: tone.is_some(),
            },
        });
    }
    recs
}

/// 取 `(len, mtime_ns)` 供 `photo_hash` 使用；stat 失败返回 `(0, 0)`。
///
/// 与 `build_records_combined` 内的同款逻辑抽出，避免三处各写一遍。
/// 失败时并非「静默丢弃」：`photo_hash(path, 0, 0)` 仍会产出一个确定的哈希，
/// 该照片照常入库；若文件真的不可读，下游 EXIF/解码阶段会走各自的失败留痕路径。
fn stat_len_mtime(path: &Path) -> (u64, u128) {
    match std::fs::metadata(path) {
        Ok(md) => (
            md.len(),
            md.modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ),
        Err(_) => (0, 0),
    }
}

/// 取父目录字符串（无父目录时为空串，与 `build_records_combined` 口径一致）
fn parent_dir_of(path: &Path) -> String {
    path.parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
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
/// 流程：路径由调用方传入（同一次 walk 的产物 + 已算好的增量差集）→ 缩略图映射（查表优先，
/// 缺失现场生成）→ 同档位向量差集（跳过已编码）→ 分批 `vision::embed_images_batch`
/// （fp16，batch 可选默认 8）→ 500/批事务 upsert → 返回报告与明细行（供前端统一表格展示）。
async fn scan_album_embeddings(
    album_id: i64,
    user_id: i64,
    paths: &[std::path::PathBuf],
    batch_size: usize,
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(ScanReport, Vec<UnifiedScanRow>), String> {
    use std::collections::HashMap;

    use crate::db::{embedding::EmbeddingRecord, DbError};

    // 路径由调用方传入（同一次 walk 的产物 + 已算好的增量差集），
    // 本函数不再自己 `collect_images(dir)` 重走一遍目录，也不再自己判断增量
    // （判据已统一到 `photo_hash` 差集，见 `scan_album_combined` 的 P8 段）。
    let photos: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if photos.is_empty() {
        return Ok((ScanReport::new(0, 0, 0, 0), Vec::new()));
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
        // 缩略图全部生成失败 → 这一批（差集）一张都处理不了。
        // `processed` 记 0（确实没有成功处理的），`failed` 记差集大小（全部失败）。
        return Ok((ScanReport::new(photos.len(), 0, 0, photos.len()), Vec::new()));
    }

    // 3. 同档位向量差集：已有向量的照片跳过（重复扫描秒级完成；按当前档位模型隔离）
    //
    // ⚠ 这里**绝不能**用一个写死的模型标识兜底。历史上写成
    // `clip_model_id(app).await.unwrap_or_else(|_| "…-fp16")`：一旦 /health 拿不到
    // model_id（服务刚起、模型还在加载），向量就会被贴上「fp16」标签落库。若真实
    // 档位是 fp32，这批向量在查询侧永远匹配不上——**向量在库里，却一条都搜不出来**
    // （BUG-2026-0921-004）。宁可显式失败，也不要静默写错标。
    let model = crate::vision::clip_model_id(app)
        .await
        .map_err(|e| format!("无法确定当前语义模型档位，已中止向量写入以免写错标签：{e}"))?;
    let all_hashes: Vec<String> = meta.iter().map(|(_, h, _)| h.clone()).collect();
    let existing: std::collections::HashSet<String> = {
        let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
        db.lookup_embedding_hashes(&all_hashes, &model).unwrap_or_default()
    };
    let meta_len = meta.len();
    // `photo_hash` 差集已在调用方统一算过（P8），走到这里 `photos` 本身**就是**差集；
    // 此处再叠一层「已有同档位向量」的过滤，是因为两条判据不同源：
    //   - `photo_hash` 差集 ⊆ `photo_content_scan`（EXIF/AI/影调表的记录）
    //   - 向量表单独的「已编码」记录按 (photo_hash, model) 存
    // 换过 CLIP 档位时，照片在 `photo_content_scan` 有记录但向量表没有 → 必须补齐。
    // 因此这一层**不能删**，它不是重复判断，而是覆盖「模型切档后向量需重建」的场景。
    let todo: Vec<(String, String, String)> = meta
        .into_iter()
        .filter(|(_, h, _)| !existing.contains(h))
        .collect();
    // 日志口径：`photos.len() - todo.len()` 会把**缩略图生成失败**的照片
    // 也算成「已有向量跳过」—— 这两件事完全不同：前者是失败（需排查文件），
    // 后者是正常的增量跳过（预期行为）。混在一起会让「跳过 N 张」这个数字失去意义。
    let skipped = meta_len - todo.len();
    let thumb_failed = photos.len() - meta_len;
    // 增量决策必须可见 —— 此前只有 AI 分支打了「跳过 N 张」，
    // 向量分支跳过多少张、为什么跳过完全看不出来，导致「选了增量还是慢」无法归因。
    crate::logger::log_info(&format!(
        "[scan.incr] album={album_id} 向量 | 输入 {total} 张 · 待编码 {todo_n} 张 · 已有同档位向量跳过 {skipped} 张 · 缩略图失败 {thumb_failed} 张",
        total = photos.len(),
        todo_n = todo.len(),
    ));

    // 4. 分批编码（进度事件由 embed_images_batch 内部 emit "embed-progress"）
    let thumb_to_src: HashMap<String, (String, String)> = todo
        .iter()
        .map(|(src, h, tp)| (tp.clone(), (src.clone(), h.clone())))
        .collect();
    let thumb_paths: Vec<String> = todo.iter().map(|(_, _, tp)| tp.clone()).collect();
    let results = crate::vision::embed_images_batch(&thumb_paths, batch_size, app, Some(cancel)).await?;

    // 5. 500/批事务写库（f32 小端 BLOB，服务端已归一化）
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
                model: model.clone(),
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
        // `photos.len()` 在外层合并逻辑里只被用于累加 written / failed
        // （total / processed / skipped 由调用方按相册目录口径自己持有），
        // 但这里仍要给出自洽的一组值，避免单独调用时出现 total < written 的矛盾报告。
        // `processed` = 实际进入编码的照片数（`meta_len`，即缩略图可用的那批）。
        ScanReport::new(photos.len(), meta_len, written, failed),
        rows,
    ))
}

/// 命令层（薄壳，逻辑见上；`lib.rs` 仅注册）
use std::sync::atomic::Ordering;

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
            // `total` 是整册识别出的张数（此路径为全量识别，故 processed == total）
            Ok(ScanReport::new(total, total, written, failed))
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
            .all(|s| ["basic", "tone", "person", "ai", "semantic"].contains(&s.as_str()))
        {
            return Err("非法 scan_types，允许 basic / tone / person / semantic".to_string());
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
        // v5：「ai」更名为「person」（分类模型下线后该分支只做人物/夜景/文档规则识别）；
        // 旧前端可能仍传 "ai"，一并兼容
        let do_ai = scan_types.contains(&"person".to_string()) || scan_types.contains(&"ai".to_string());
        // FEAT-SEM：语义向量扫描（独立于 AI 分支，缩略图直编码，增量跳过已有向量）
        let do_semantic = scan_types.contains(&"semantic".to_string());
        let batch = batch_size.unwrap_or(8).clamp(4, 64) as usize;

        // 细粒度耗时记账：每个板块一段，命令结束前打一行汇总（见 scan_timing 模块说明）。
        // 纯记账 —— 不参与任何判断分支，出问题最多是少一行日志。
        let mut timing = crate::scan_timing::ScanTiming::new(album_id);

        // ===================================================================
        // T10/P5：目录只走一遍（且不阻塞异步运行时）
        //
        // 此前 AI 识别、影调、EXIF 三个分支各自 walkdir 一遍同一目录（语义分支
        // 内部还会再走一次），同一棵树被走 4 遍。机械盘/网络盘上每次遍历都是一串
        // 随机 IO，这是纯粹的重复浪费。现在统一 walk 一次，各分支复用产物。
        // walk_image_paths 是同步 IO，包进 spawn_blocking 避免占死异步 worker。
        // ===================================================================
        let t_walk = std::time::Instant::now();
        let all_paths = {
            let dir2 = dir.clone();
            tauri::async_runtime::spawn_blocking(move || crate::vision::walk_image_paths(&dir2))
                .await
                .map_err(|e| format!("目录遍历任务线程失败: {e}"))??
        };
        timing.span_with("walk", t_walk, &format!("{}张", all_paths.len()));

        // ===================================================================
        // P8：增量统一判据 —— photo_hash 差集
        //
        // 旧实现里每条腿各自为政：AI 分支自己查一次 hash 差集、向量分支自己查一次，
        // 影调/EXIF 则完全不看增量。判据分散导致「某条腿漏判/误判」
        // 且无法统一解释「这次扫描到底跳过了什么」。
        //
        // 现在统一在这里算一次差集。成本极低：`photo_hash` 只取
        // (文件长度, mtime, 路径) 三项 —— 微秒级 stat，而解码是毫秒级，差三个数量级。
        //
        // ⚠️ 诚实边界：差集省不掉 walk + stat。要确定「文件没变」就必须看文件；
        //    能省的只有解码（本地耗时的 90%+）。
        //
        // 判据表是 `photo_content_scan`（EXIF/AI/影调共同落库的那张表），
        // 与旧 AI 分支用的 `lookup_scanned_hashes_by_album` 是同一张表 → 口径一致。
        // ===================================================================
        let t_diff = std::time::Instant::now();
        let pending: Vec<std::path::PathBuf> = if overwrite {
            logger::log_info(&format!(
                "[scan.incr] album={album_id} 全量覆盖模式 | 目录 {} 张 · 待处理 {} 张 · 跳过 0 张",
                all_paths.len(),
                all_paths.len()
            ));
            all_paths.clone()
        } else {
            let scanned: std::collections::HashSet<String> = {
                let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
                db.lookup_scanned_hashes_by_album(album_id)
                    .map_err(|e| format!("{e}"))?
            };
            let kept: Vec<std::path::PathBuf> = all_paths
                .iter()
                .filter(|p| {
                    let ps = p.to_string_lossy();
                    let hash = match std::fs::metadata(p) {
                        Ok(md) => {
                            let mtime = md
                                .modified()
                                .ok()
                                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                                .map(|d| d.as_nanos())
                                .unwrap_or(0);
                            photo_hash(&ps, md.len(), mtime)
                        }
                        // stat 失败（权限/已删除）→ 保守地算作「待处理」，
                        // 让后面对它的读取失败走既有错误路径并留痕，而不是静默丢弃。
                        Err(_) => String::new(),
                    };
                    hash.is_empty() || !scanned.contains(&hash)
                })
                .cloned()
                .collect();
            let skipped = all_paths.len() - kept.len();
            logger::log_info(&format!(
                "[scan.incr] album={album_id} 增量模式（统一判据 photo_hash）| 目录 {} 张 · 待处理 {} 张 · 跳过 {skipped} 张",
                all_paths.len(),
                kept.len()
            ));
            kept
        };
        timing.span_with("差集", t_diff, &format!("待处理{}张", pending.len()));

        // ===================================================================
        // P10：相册级「零变化」早退
        //
        // 全局批量增量扫几十个已入库相册时，每个都要走完全部阶段（含多次 spawn_blocking
        // 与 microservice 调用），绝大部分相册其实一张新照片都没有。
        //
        // 正确表达是「**文件级差集为空**」而不是「凭记录不看目录」——
        // 后者会漏掉外部改动（用户在资源管理器里换了图但路径未变）。
        // 这里已经真的 walk 过、真的逐张 stat 过，所以早退是安全的。
        // ===================================================================
        if pending.is_empty() {
            logger::log_info(&format!(
                "[scan.incr] album={album_id} 差集为空 → 本相册无变化，跳过全部阶段"
            ));
            logger::log_call_end_with(
                "scan_album_combined",
                _t,
                &format!("OK | 增量无变化 | 目录 {} 张 · 全部跳过", all_paths.len()),
            );
            // BUG-2026-0922-008：这里曾经返回 `total: 0` —— 前端据此显示
            // 「共 0 张」，与「相册里明明有照片」直接矛盾（全局扫描一结束，
            // 所有已入库相册全变成 0/0）。早退只说明「本次没有要处理的」，
            // **不等于相册是空的**，所以 total 必须填真实目录张数。
            timing.emit("早退·无变化");
            return Ok(CombinedScanOutcome {
                report: ScanReport::new(all_paths.len(), 0, 0, 0),
                rows: Vec::new(),
            });
        }

        // 本地并行线程数：EXIF / 影调是纯本地 IO+CPU 任务，多线程真实摊薄延迟；
        // 夹在 [1,8]，避免小机器上线程过多互抢。
        let threads_local = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 8);

        // EXIF 扫描（AI 与非 AI 分支都要：AI 分支的 build_records_combined 复用这份数据，
        // 消除此前「同一份 EXIF 读两遍」的浪费）→ 放入阻塞线程
        let t_exif = std::time::Instant::now();
        let exifs = if do_basic {
            let paths2 = pending.clone();
            tauri::async_runtime::spawn_blocking(move || {
                crate::photo_scan::read_exif_paths_parallel(&paths2, threads_local)
            })
            .await
            .map_err(|e| format!("EXIF 任务线程失败: {e}"))?
        } else {
            Vec::new()
        };
        if do_basic {
            timing.span_with("EXIF", t_exif, &format!("{}线程·{}张", threads_local, exifs.len()));
        }

        // ===================================================================
        // P-OPT3（点 1）：三腿并发 —— 影调 ∥ 语义向量 ∥ AI 识别
        //
        // 现状是三条腿严格串行 await，但三者**彼此零依赖**：
        //   - 影调是纯本地解码+直方图，与任何模型无关；
        //   - 语义腿打 /embed_batch（CLIP 图像塔），AI 腿打 /classify_batch
        //     （det/face/ocr），不同端点、不同会话，互不消费对方输出；
        //   - 三者输入都只是上面算好的不可变差集 `pending`。
        // 串行 await 白白丢掉重叠：实测（bench_cpu_gpu_modes.py --overlap 4，P=6）
        // 语义腿 ∥ AI 腿 = 顺序 9905ms → 并发 7636ms，**1.30x**（同一份 CPU 预算
        // 争用使其达不到理论 max() 的 1.67x，但重叠确实发生）。
        //
        // 硬约束照旧：EXIF 是阶段 0，**先跑完**再启动三腿（时间+地点先于语义向量）。
        // 取消语义照旧：两条 HTTP 腿仍只在批次之间检查 `cancel`。
        // ===================================================================
        let t_tone = std::time::Instant::now();

        // AI 腿的增量日志：内容在并发前算好，避免把 all_paths 移进 future
        // （future 是 async move，引用不 Copy 的值会被整体搬走）。
        let ai_paths: Vec<String> = if do_ai {
            let v: Vec<String> = pending
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            logger::log_info(&format!(
                "[scan.incr] album={album_id} AI 识别 | 目录 {} 张 · 待识别 {} 张 · 跳过 {} 张（增量判据：photo_hash）",
                all_paths.len(),
                v.len(),
                all_paths.len().saturating_sub(v.len()),
            ));
            v
        } else {
            Vec::new()
        };

        // 腿 1：影调（本地 CPU，spawn_blocking 不占异步 worker）
        let tone_fut = {
            let paths2 = pending.clone();
            async move {
                if do_tone {
                    tauri::async_runtime::spawn_blocking(move || {
                        crate::tone::analyze_paths_parallel(&paths2, threads_local)
                    })
                    .await
                    .map_err(|e| format!("影调任务线程失败: {e}"))
                } else {
                    Ok(Vec::new())
                }
            }
        };

        // 腿 2：语义向量（异步 HTTP → vcr-clip）。`Instant` 在 future 内部读，
        // 这样记的是该腿自己的墙钟，而不是「启动三腿到 join 返回」的公共时长。
        // 引用一律显式绑定后再进 future：async move 会把非 Copy 的捕获整体搬走，
        // 而 `pending` / `app` / `state` 后面还要用。
        let sem_fut = {
            let pending_ref: &[std::path::PathBuf] = &pending;
            let app_ref = &app;
            let state_ref = &state;
            let cancel_sem = cancel.clone();
            async move {
                if do_semantic {
                    let t = std::time::Instant::now();
                    let r = scan_album_embeddings(
                        album_id,
                        user_id,
                        pending_ref,
                        batch,
                        app_ref,
                        state_ref,
                        cancel_sem,
                    )
                    .await;
                    Some((t, r))
                } else {
                    None
                }
            }
        };

        // 腿 3：AI 识别（异步 HTTP → vcr-ai）。P8/P9：差集已统一算好，
        // 这里不再自己查一遍 hash（判据统一，也省一次遍历）。
        let ai_fut = {
            let app_ref = &app;
            let cancel_ai = cancel.clone();
            async move {
                if do_ai {
                    let t = std::time::Instant::now();
                    crate::vision::classify_paths(&ai_paths, batch, app_ref, Some(cancel_ai))
                        .await
                        .map(|v| (t, v))
                } else {
                    Ok((std::time::Instant::now(), Vec::new()))
                }
            }
        };

        let (tone_res, sem_res, ai_res) = tokio::join!(tone_fut, sem_fut, ai_fut);

        let tones = tone_res?;
        if do_tone {
            timing.span_with("影调", t_tone, &format!("{}线程·{}张", threads_local, tones.len()));
        }

        // FEAT-SEM：语义向量扫描与 AI 识别同为异步 HTTP，在 outcome 构建前完成，
        // 结果合并进 CombinedScanOutcome（report 累加，rows 追加）。
        let semantic_outcome: Option<Result<(ScanReport, Vec<UnifiedScanRow>), String>> =
            match sem_res {
                Some((t_sem, r)) => {
                    timing.span_with("语义向量", t_sem, &format!("批次{}（与AI腿并发）", batch));
                    Some(r)
                }
                None => None,
            };

        let (t_ai, vision_results) = ai_res?;
        if do_ai {
            timing.span_with(
                "AI识别",
                t_ai,
                &format!("批次{}·{}张（与语义腿并发）", batch, vision_results.len()),
            );
        }

        let t_store = std::time::Instant::now();
        let mut outcome: Result<CombinedScanOutcome, String> = (|| -> Result<CombinedScanOutcome, String> {
            if do_ai {
                let tone_ref = if do_tone { Some(&tones) } else { None };
                // P4：EXIF 由上面第 1 步已读出的那一份传入，本函数不再自己逐张重读
                // （此前同一份 EXIF 被读两遍：第 1 步一遍、这里一遍）。
                let (recs, rows) =
                    build_records_combined(album_id, user_id, &vision_results, tone_ref, &exifs, &app)?;
                let written_count = recs.len();
                {
                    let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
                    db.upsert_photo_contents(&recs).map_err(|e| format!("{:?}", e))?;
                }
                // BUG-2026-0922-008：`total` 曾经填 `vision_results.len()`（= 差集大小）。
                // 增量模式下差集常常为 0，前端就把有照片的相册显示成「共 0 张」。
                // 现在 total = 目录真实张数；本次处理了多少交给 `processed`。
                let report = ScanReport::new(
                    all_paths.len(),
                    vision_results.len(),
                    written_count,
                    vision_results.iter().filter(|r| r.error.is_some()).count(),
                );
                Ok(CombinedScanOutcome { report, rows })
            } else {
                // P2：非 AI 分支也必须落库。
                //
                // 旧行为：只在内存拼 `UnifiedScanRow` 给前端看 → `written: 0`。
                // 三个后果，按严重度排序：
                //   ① **增量扫描对它永久失效**：差集判据是 `photo_content_scan` 有没有这一行，
                //      不落库 ⇒ 每次扫描都把它当新照片，全量重做（这是用户最直观的「白等」）。
                //   ② **数据丢失缺陷**：刷新页面/切走再回来，刚扫出的 EXIF/影调全没了。
                //   ③ 前端表格显示的 `total` 有值而 `written` 恒为 0，语义自相矛盾。
                //
                // 实现上**先造 records、再由 records 派生 rows**（而不是像旧代码那样
                // 两个循环各拼一遍）：DB 与 UI 同源，杜绝「表里有、库里没有」这类漂移。
                let recs = build_records_from_exif_tone(album_id, user_id, &exifs, &tones);

                // 供前端表格展示：影调/EXIF 按路径索引（与 recs 同一份数据）
                let tone_map: std::collections::HashMap<&str, &crate::tone::PhotoTone> =
                    tones.iter().map(|t| (t.path.as_str(), t)).collect();
                let exif_map: std::collections::HashMap<&str, &crate::photo_scan::PhotoExif> =
                    exifs.iter().map(|e| (e.path.as_str(), e)).collect();

                let mut all_rows: Vec<UnifiedScanRow> = Vec::with_capacity(recs.len());
                for rec in &recs {
                    let ex = exif_map.get(rec.path.as_str());
                    let tone = tone_map.get(rec.path.as_str());
                    let file_name = Path::new(&rec.path)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    all_rows.push(UnifiedScanRow {
                        file_name,
                        path: rec.path.clone(),
                        iso: ex.and_then(|e| e.iso.clone()),
                        aperture: ex.and_then(|e| e.aperture.clone()),
                        shutter_speed: ex.and_then(|e| e.shutter_speed.clone()),
                        focal_length: ex.and_then(|e| e.focal_length.clone()),
                        shoot_time: ex.and_then(|e| e.shoot_time.clone()),
                        iso_num: ex.and_then(|e| e.iso_num),
                        focal_num: ex.and_then(|e| e.focal_num),
                        aperture_num: ex.and_then(|e| e.aperture_num),
                        shutter_num: ex.and_then(|e| e.shutter_num),
                        tone_type: tone.and_then(|t| t.tone_type.map(|e| format!("{:?}", e))),
                        avg_luma: tone.and_then(|t| t.avg_luma),
                        category: None,
                        sub_category: None,
                        label: None,
                        confidence: None,
                        top3: Vec::new(),
                        person_ids: Vec::new(),
                        person_count: 0,
                    });
                }

                // P2 落库：写失败必须报错而不是静默通过 —— 否则 `written` 会撒谎，
                // 用户以为扫完了，实际下次仍是全量。
                let written_count = recs.len();
                {
                    let db = state.0.lock().map_err(|e| format!("{:?}", e))?;
                    db.upsert_photo_contents(&recs).map_err(|e| format!("{:?}", e))?;
                }
                logger::log_info(&format!(
                    "[scan.incr] album={album_id} 非AI分支落库 | EXIF {} 张 · 影调 {} 张 → 写入 {written_count} 条（照片级去重后）",
                    exifs.len(),
                    tones.len(),
                ));
                Ok(CombinedScanOutcome {
                    // BUG-2026-0922-008：同样不能把 `written_count` 当 total。
                    // 非 AI 分支只处理 diff 里的照片，写库数与「相册有多大」是两件事。
                    report: ScanReport::new(all_paths.len(), recs.len(), written_count, 0),
                    rows: all_rows,
                })
            }
        })();
        let store_note = match &outcome {
            Ok(o) => format!("写入{}条", o.report.written),
            Err(_) => "失败".to_string(),
        };
        timing.span_with("构建+落库", t_store, &store_note);

        // FEAT-SEM：合并语义向量扫描结果；语义子任务失败则整体报错（用户明确勾选，
        // 部分成功无意义——向量未入库，搜索仍搜不到）
        match semantic_outcome {
            Some(Err(e)) => outcome = Err(format!("语义扫描失败: {e}")),
            Some(Ok((report, rows))) => {
                if let Ok(o) = outcome.as_mut() {
                    // BUG-2026-0922-008：**只能累加 written / failed**。
                    //
                    // `total` / `processed` / `skipped` 都是「相册目录」维度的量，
                    // 而两条腿跑的是**同一个相册的同一份差集**：
                    //   total      两侧完全相同 → 相加会翻倍成 2N
                    //   processed  两侧都是同一个差集大小 → 相加会翻倍
                    //   skipped    相加同样翻倍
                    // 旧代码只累加 total 而 total 恰好是差集大小，所以「翻倍」被掩盖了。
                    // 语义分支的跳过数另有归因，走独立日志（见 `[scan.incr] … 向量 |`），
                    // 不往相册级报告里混。
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
        let t_prewarm = std::time::Instant::now();
        if do_ai && outcome.is_ok() {
            let prewarm_paths: Vec<String> = vision_results
                .iter()
                .filter(|r| r.error.is_none())
                .map(|r| r.path.clone())
                .collect();
            if !prewarm_paths.is_empty() {
                let _ = prewarm_thumbs_after_scan(&app, &state, album_id, user_id, &prewarm_paths).await;
                timing.span_with("缩略图预热", t_prewarm, &format!("{}张", prewarm_paths.len()));
            }
        }
        // v5 语义分类：扫描完成后自动重建分类命中（关键词向量已缓存，成本毫秒~秒级）。
        // 失败不阻塞扫描结果——用户仍可在分类页手动「重建」。
        let t_rebuild = std::time::Instant::now();
        if outcome.is_ok() && (do_ai || do_semantic) {
            match crate::category::rebuild_semantic_hits(&app, &state, user_id).await {
                Ok(rep) => {
                    logger::log_info(&format!(
                        "[scan] 分类命中已重建：分类 {} · 命中 {} · 索引 {} · {}ms",
                        rep.categories, rep.hits, rep.indexed, rep.ms
                    ));
                    timing.span_with("分类重建", t_rebuild, &format!("命中{}", rep.hits));
                }
                Err(e) => {
                    logger::log_info(&format!("[scan] 分类重建跳过（不影响扫描）：{e}"));
                    timing.span_with("分类重建", t_rebuild, "跳过");
                }
            }
        }
        // 细粒度耗时汇总（一行看清本次扫描每个板块各花了多少）
        timing.emit(&format!(
            "{} | 目录{}张",
            if overwrite { "全量" } else { "增量" },
            all_paths.len()
        ));
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
            // 第一路：图像向量语义（既有逻辑，一字不改）
            let mut img_lane: Vec<(db::SmartHit, f64)> = Vec::new();
            match semantic_recall(&kw, min_sim, user_id, &state, &app).await {
                Ok(sem) if !sem.is_empty() => {
                    img_lane = sem;
                }
                Ok(_) => {}
                Err(e) => {
                    // 降级是预期行为（模型未下载/服务未启动/向量库为空），不打扰用户；
                    // 但**必须落日志**：此前只 eprintln!，用户与排查者都看不到原因，
                    // 导致「语义检索突然失效但无任何提示」（BUG-2026-0920-006）
                    logger::log_info(&format!("[smart_search] 图像语义降级: {e}"));
                }
            }
            // FEAT-067 第二路：描述向量语义（时间/地点/场景/标签/人物真名）
            // 只加一路，融合公式与权重沿用既有 RRF。
            let mut desc_lane: Vec<(db::SmartHit, f64)> = Vec::new();
            match crate::textdesc::recall_by_text(&app, &state, user_id, &kw, min_sim).await {
                Ok(list) if !list.is_empty() => {
                    desc_lane = list;
                }
                Ok(_) => {}
                Err(e) => logger::log_info(&format!("[smart_search] 描述语义降级: {e}")),
            }
            if !img_lane.is_empty() || !desc_lane.is_empty() {
                let n_img = img_lane.len();
                let n_desc = desc_lane.len();
                let before = hits.len();
                hits = fuse_hits3(hits, img_lane, desc_lane);
                logger::log_call_end_with(
                    "smart_search.semantic",
                    _t,
                    &format!(
                        "OK | 图像语义 {} · 描述语义 {} · 融合 {} → {}",
                        n_img,
                        n_desc,
                        before,
                        hits.len()
                    ),
                );
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

    /// FEAT-SEM：语义链路状态（前端据此给出**分档**提示 + 重试入口）
    ///
    /// 与 `warmup_semantic_service` 的区别：后者只返回一个 bool，前端拿到 false 只能
    /// 渲染同一句「CLIP 模型未下载」；而真实原因可能是退避中、服务没起来、或索引与当前
    /// 模型档位不一致（BUG-2026-0920-006/007）——用户照提示去「下载模型」当然没用。
    ///
    /// **只读探测**：不拉起服务、不改动退避状态。否则「看一眼状态」本身就变成一次冷启动。
    #[tauri::command]
    pub async fn semantic_status(
        app: tauri::AppHandle,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<crate::vision::SemanticStatus, String> {
        use crate::vision::{
            clip_model_present, clip_ready, semantic_backoff_remaining, SemanticStatus,
        };

        // 1. 退避窗口（最高优先：退避期内其它探测都无意义）
        if let Some(rest) = semantic_backoff_remaining() {
            return Ok(SemanticStatus {
                ready: false,
                code: "backoff".into(),
                message: format!(
                    "语义服务刚失败过一次，正在约 {} 秒的短暂退避内；本次已自动降级为关键词匹配，稍后会重试。",
                    rest.as_secs().max(1)
                ),
                backoff_remaining_ms: rest.as_millis() as u64,
            });
        }
        // 2. 模型文件缺失（唯一真正需要用户「去下载」的情形）
        if !clip_model_present() {
            return Ok(SemanticStatus {
                ready: false,
                code: "model_missing".into(),
                message: "CLIP 语义模型尚未下载，当前只能做关键词匹配。可前往「扫描中心 → ⚙ 性能设置」下载语义模型。".into(),
                backoff_remaining_ms: 0,
            });
        }
        // 3. 服务未运行 / 模型未加载完成
        if !clip_ready(&app).await {
            return Ok(SemanticStatus {
                ready: false,
                code: "service_unreachable".into(),
                message: "语义识别服务未就绪（尚未启动，或正在加载模型）。直接搜索就会触发启动，也可点「重试」立即拉起。".into(),
                backoff_remaining_ms: 0,
            });
        }
        // 4. 索引与当前档位不一致：向量确实在库里，但没有一条属于当前模型 —— 换档后
        //    未重建的典型症状，用户看到的就是「向量存在却搜不出任何东西」。
        let user_id = require_user(&session)?;
        if let Ok(model) = crate::vision::clip_model_id(&app).await {
            let stats = {
                let db = state.0.lock().map_err(|e| format!("{e:?}"))?;
                db.category_index_stats(user_id, &model).ok()
            };
            if let Some(s) = stats {
                if s.indexed == 0 && s.stale > 0 {
                    return Ok(SemanticStatus {
                        ready: false,
                        code: "index_model_mismatch".into(),
                        message: format!(
                            "语义索引与当前模型档位「{model}」不匹配：库中 {} 条向量出自其它档位，无法参与检索。请在「扫描中心」重建语义向量索引。",
                            s.stale
                        ),
                        backoff_remaining_ms: 0,
                    });
                }
            }
        }
        Ok(SemanticStatus {
            ready: true,
            code: "ready".into(),
            message: String::new(),
            backoff_remaining_ms: 0,
        })
    }
}


// =====================================================================
// 以下命令自 lib.rs 迁入（lib.rs 瘦身）：智能分类命令层（薄包装）
// =====================================================================


/// 视觉内容识别（YOLOv8n-cls，测试功能，不落库）
///
/// 启动/复用独立 Python 微服务，批量识别相册目录内图片的内容，
/// 通过 `classify-progress` 事件实时上报进度。识别逻辑全部在
/// 独立模块 `vision` 中，此处仅保留薄命令壳（功能解耦）。
#[tauri::command]
pub async fn classify_album(
    path: String,
    batch_size: Option<i64>,
    app: tauri::AppHandle,
    scan: tauri::State<'_, crate::ScanState>,
) -> Result<Vec<crate::vision::VisionResult>, String> {
    let _t = log_call!("classify_album", &format!("path={path}"));
    scan.0.store(false, Ordering::SeqCst);
    let r = crate::vision::classify_album(
        &path,
        batch_size.unwrap_or(8).max(1) as usize,
        &app,
        Some(scan.0.clone()),
    )
    .await;
    match &r {
        Ok(list) => crate::logger::log_call_end_with(
            "classify_album",
            _t,
            &format!("OK | photos={}", list.len()),
        ),
        Err(e) => crate::logger::log_call_end_with("classify_album", _t, &format!("ERR | {e}")),
    }
    r
}


#[cfg(test)]
mod semantic_tests {
    use super::{fuse_hits, fuse_hits3};
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

    /// FEAT-067 步骤 5：加一路描述语义后，既有两路融合结果不变（回归第 8 条）
    #[test]
    fn three_lane_fuse_keeps_two_lane_result_when_desc_empty() {
        let fts = vec![hit("c.jpg"), hit("a.jpg")];
        let sem = vec![(hit("b.jpg"), 0.46), (hit("a.jpg"), 0.44)];
        let two = fuse_hits(fts.clone(), sem.clone());
        let three = fuse_hits3(fts, sem, Vec::new());
        let p2: Vec<&str> = two.iter().map(|h| h.path.as_str()).collect();
        let p3: Vec<&str> = three.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(p2, p3, "描述路为空时三路融合必须等价于既有两路");
    }

    /// 描述路带来的命中是**追加**而非替换：原命中集合不缩水
    #[test]
    fn three_lane_fuse_only_adds_hits() {
        let fts = vec![hit("c.jpg"), hit("a.jpg")];
        let img = vec![(hit("b.jpg"), 0.46)];
        let desc = vec![(hit("d.jpg"), 0.52)];
        let out = fuse_hits3(fts, img, desc);
        let paths: Vec<&str> = out.iter().map(|h| h.path.as_str()).collect();
        for p in ["a.jpg", "b.jpg", "c.jpg", "d.jpg"] {
            assert!(paths.contains(&p), "融合后不应丢失任何一路的命中: {p}");
        }
        // 两路都命中时取较高余弦
        let both = fuse_hits3(
            vec![hit("x.jpg")],
            vec![(hit("x.jpg"), 0.40)],
            vec![(hit("x.jpg"), 0.61)],
        );
        assert!((both[0].semantic_score.unwrap() - 0.61).abs() < 1e-9);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FEAT-067 步骤 3：地点补齐 —— EXIF 地名优先，其次 GPS 离线反查，都没有则 None
    #[test]
    fn resolve_location_prefers_exif_place_then_geo() {
        // 不存在的文件 → read_photo_exif 返回全 None（快路径本身不做反查）
        let mut base = crate::photo_scan::read_photo_exif(Path::new("/nonexistent/x.jpg"), "x.jpg");
        assert_eq!(resolve_location(&base), None, "无 GPS 无地名 → None");
        base.lat = Some(30.6593);
        base.lon = Some(104.0657);
        assert_eq!(resolve_location(&base).as_deref(), Some("四川省 · 成都市"));
        // EXIF 已带地名 → 直接用，不被覆盖
        base.place = Some("杭州西湖".into());
        assert_eq!(resolve_location(&base).as_deref(), Some("杭州西湖"));
    }

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

    // =====================================================================
    // BUG-2026-0922-008：ScanReport 三字段恒等式
    //
    // 这组断言锁住的是「前端为什么能看到 0」这件事不再复发：
    //   total == processed + skipped  必须**永远**成立
    // 且「相册里有多少张」这个数（total）绝不能随增量模式变化。
    // =====================================================================

    /// 恒等式：total 永远等于 processed + skipped（各分支共用同一构造函数）
    #[test]
    fn scan_report_identity_holds() {
        // 全量扫描：目录 10 张，全部处理
        let full = ScanReport::new(10, 10, 10, 0);
        assert_eq!(full.total, 10);
        assert_eq!(full.processed, 10);
        assert_eq!(full.skipped, 0);
        assert_eq!(
            full.total,
            full.processed + full.skipped,
            "全量：total 必须 = processed + skipped"
        );

        // 增量扫描：目录 10 张、差集 3 张 → 跳过 7 张
        let incr = ScanReport::new(10, 3, 3, 0);
        assert_eq!(incr.total, 10, "total 是目录张数，不随增量模式缩水");
        assert_eq!(incr.skipped, 7);
        assert_eq!(incr.total, incr.processed + incr.skipped);

        // 零变化早退：目录 10 张、差集空 → total 仍须是 10，绝不是 0
        let noop = ScanReport::new(10, 0, 0, 0);
        assert_eq!(noop.total, 10, "无变化 ≠ 空相册：total 必须保留真实张数");
        assert_eq!(noop.skipped, 10);
        assert_eq!(noop.total, noop.processed + noop.skipped);
    }

    /// 缩略图全失败等边角情况：processed 由各分支独立统计可能略大于目录总数，
    /// `skipped` 必须钳到 0 而不是下溢 panic（usize 减法溢出在 debug 下会直接崩）。
    #[test]
    fn scan_report_never_underflows_when_processed_exceeds_total() {
        let odd = ScanReport::new(5, 7, 0, 7);
        assert_eq!(odd.total, 5);
        assert_eq!(odd.skipped, 0, "processed > total 时 skipped 必须钳为 0，不得下溢");
    }

    /// 真实空相册：目录 0 张 → 三数全 0，此时前端显示「共 0 张」才是正确的。
    #[test]
    fn scan_report_for_truly_empty_album_is_all_zero() {
        let empty = ScanReport::new(0, 0, 0, 0);
        assert_eq!((empty.total, empty.processed, empty.skipped), (0, 0, 0));
    }

    // =====================================================================
    // P2：非 AI 分支落库 —— 记录构造的字段分区纪律
    //
    // 这三条断言锁住的是「增量扫描能否生效」的前提：非 AI 分支必须造出记录，
    // 且造出的记录**不能带 AI 字段的假值**（否则会经 upsert 覆盖掉 AI 结果）。
    // =====================================================================

    /// 构造一个只填了 EXIF 字段的样本（模拟 `read_exif_paths_parallel` 的产出）
    fn sample_exif(path: &str) -> crate::photo_scan::PhotoExif {
        let mut ex = crate::photo_scan::read_photo_exif(Path::new("/nonexistent/p.jpg"), "p.jpg");
        ex.path = path.to_string();
        ex.file_name = Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        ex.shoot_time = Some("2024-02-10 18:30:00".into());
        ex.iso = Some("400".into());
        ex.iso_num = Some(400);
        ex.aperture = Some("f/1.8".into());
        ex.aperture_num = Some(1.8);
        ex.shutter_speed = Some("1/125s".into());
        ex.shutter_num = Some(0.008);
        ex.focal_length = Some("35mm".into());
        ex.focal_num = Some(35.0);
        ex
    }

    fn sample_tone(path: &str, luma: f64) -> crate::tone::PhotoTone {
        crate::tone::PhotoTone {
            file_name: Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: path.to_string(),
            histogram: vec![0u32; 256],
            avg_luma: Some(luma),
            tone_type: Some(if luma < 85.0 {
                crate::tone::ToneType::LowKey
            } else if luma > 170.0 {
                crate::tone::ToneType::HighKey
            } else {
                crate::tone::ToneType::MidKey
            }),
        }
    }

    /// P2 正向：EXIF+影调必须产出记录（否则增量判据缺失，每次都全量重做）
    #[test]
    fn p2_exif_tone_path_writes_records_without_ai_fields() {
        let exifs = vec![sample_exif("/a/1.jpg")];
        let tones = vec![sample_tone("/a/1.jpg", 60.0)];
        let recs = build_records_from_exif_tone(1, 1, &exifs, &tones);
        assert_eq!(recs.len(), 1, "非 AI 分支必须产出记录（落库是增量判据的前提）");
        let r = &recs[0];
        assert!(r.category.is_none(), "category 是 AI 产出，非 AI 分支必须留 None");
        assert!(r.sub_category.is_none());
        assert!(r.label.is_none());
        assert!(r.confidence.is_none());
        assert!(r.top3_json.is_none());
        assert!(r.person_ids.is_none());
        assert_eq!(r.person_count, 0, "0 由 SQL 侧 CASE 判为「无产出」");
        assert!(r.content.is_empty(), "空串由 SQL 侧 NULLIF 判为「无产出」");
        assert!(!r.owns.ai, "声明 owns.ai=false 才是保留旧标签的充分条件");
    }

    /// P2 兜底：只勾影调不勾 EXIF 时，记录仍以影调列表为轴产出 ——
    /// 否则「只统计影调」这个选项永远不落库。
    #[test]
    fn p2_tone_only_path_still_writes_records() {
        let tones = vec![sample_tone("/a/1.jpg", 200.0), sample_tone("/a/2.jpg", 100.0)];
        let recs = build_records_from_exif_tone(3, 1, &[], &tones);
        assert_eq!(recs.len(), 2, "只勾影调也必须落库");
        assert_eq!(recs[0].tone_type.as_deref(), Some("HighKey"));
        assert_eq!(recs[1].tone_type.as_deref(), Some("MidKey"));
        // EXIF 字段全空（本次没读）
        assert!(recs[0].iso.is_none());
        assert!(recs[0].shoot_time.is_none());
        // 关键：本次没读 EXIF ⇒ **不能**声明拥有 exif，
        // 否则会把此前 basic 扫出的拍摄时间/地点整片清空。
        assert!(!recs[0].owns.exif, "没读 EXIF 就不得声明拥有");
        assert!(!recs[0].owns.ai);
        assert!(recs[0].owns.tone, "影调是本轴数据，归本次所有");
    }

    /// P2 口径一致性：两条分支写出的 `tone_type` 字符串必须**逐字节相同**。
    ///
    /// 这条容易被忽略但后果实在：若一边写 `"LowKey"`（`{:?}`）、另一边写
    /// `"low-key"`（serde kebab-case），那么按影调筛选时会漏掉一半照片，
    /// 而且因为两批数据都「看起来正常」，极难发现。
    #[test]
    fn p2_tone_type_string_matches_other_branch() {
        let tones = vec![sample_tone("/a/1.jpg", 60.0)];
        let recs = build_records_from_exif_tone(1, 1, &[], &tones);
        assert_eq!(
            recs[0].tone_type.as_deref(),
            Some(format!("{:?}", crate::tone::ToneType::LowKey).as_str()),
            "必须与 build_records_combined 的 format!({{:?}}) 口径一致"
        );
    }
}
