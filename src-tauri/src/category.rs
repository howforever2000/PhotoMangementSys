//! 语义分类服务层（v5）—— 关键词 → 向量 → 全库匹配 → 命中物化
//!
//! ## 打分公式（P0 实测校准，见 design/ 与 python/bench/calib_debias.py）
//!
//! ```text
//! base(x)      = mean_b cos(x, b)            // b = 中性基线提示词（7 条）
//! score(x, c)  = max_kw cos(x, kw) - base(x) // 净语义增益（"比中性描述强多少"）
//! 命中条件      = score >= category.threshold 且 排除词未更优
//! ```
//!
//! 为什么不是裸余弦：B/16 上无关图对的余弦普遍落在 0.33~0.39，且**不同概念的
//! 基线不同**（美食 0.363 / 文档 0.330），单一绝对阈值在 0.30 会命中 99% 图库、
//! 在 0.38 边界处精度已崩（人工抽查 5 张仅 1 张对）。减去中性基线后 P0 抽查：
//! 「一只猫」阈值 0.03 命中 30 张（抽查 10 张 7 张有猫）、「美食」命中 297 张
//! （抽查 10 张 9 张有食物），且库中不存在的概念（文字/文档）自然 0 命中。
//!
//! ## 职责边界（CS1）
//!   - 本模块：服务层（关键词向量解析 + 匹配计算 + 编排）
//!   - `db::category`：持久层（表 CRUD / 命中批量写 / 聚合）
//!   - `vision`：CLIP HTTP 客户端（文本批量编码）
//!   - 本模块 commands：接口层薄壳（参数校验 + 转发 + 出入口日志）
//!
//! ## 性能
//! 关键词向量只编码一次并落 `clip_text_cache`；匹配是全内存点积
//! （1 万张 × 512 维 ≈ 5.6M 乘加/关键词，毫秒级）。真正的瓶颈是图片编码（扫描侧）。

use std::collections::HashMap;
use std::time::Instant;

use serde::Serialize;
use tauri::Emitter;

use crate::db::category::{self, CategoryHitRecord, CategoryInput};
use crate::{logger, AppState};

/// 默认语义匹配强度阈值（净增益；P0 实测：0.03 在真实库上精度 70%~90%）
pub const DEFAULT_THRESHOLD: f64 = 0.03;
/// 阈值可调范围（UI 滑块）
pub const THRESHOLD_MIN: f64 = 0.0;
pub const THRESHOLD_MAX: f64 = 0.10;
/// 排除词判废余量：排除词相似度不低于正向最高分 - 余量 → 判为误召回
const EXCLUDE_MARGIN: f64 = 0.005;

/// 中性基线提示词（P0 实测：其平均余弦 ≈ 0.386，是所有概念共同的"底噪"）
const NEUTRAL_PROMPTS: &[&str] = &[
    "一张照片",
    "一张图片",
    "一张普通的照片",
    "随手拍的照片",
    "日常生活照片",
    "一张图片素材",
    "相册里的一张图",
];

/// 内置预设分类（首次进入自动落库；用户可改关键词 / 调阈值 / 删除）
/// 关键词用「自然语言短语」而非单词——CLIP 对短句的语义更稳。
const PRESETS: &[(&str, &str, &[&str], f64)] = &[
    ("动物", "🐾", &["动物", "野生动物", "宠物"], DEFAULT_THRESHOLD),
    ("猫咪", "🐱", &["一只猫", "小猫", "猫咪"], DEFAULT_THRESHOLD),
    ("狗狗", "🐶", &["一只狗", "小狗", "狗狗"], DEFAULT_THRESHOLD),
    ("美食", "🍜", &["美食", "食物", "菜肴", "甜点"], DEFAULT_THRESHOLD),
    ("花卉", "🌸", &["花朵", "鲜花", "盛开的花"], DEFAULT_THRESHOLD),
    ("自然风景", "🏞️", &["自然风景", "山水风景", "湖泊"], DEFAULT_THRESHOLD),
    ("天空云彩", "☁️", &["天空", "蓝天白云", "云彩"], DEFAULT_THRESHOLD),
    ("建筑", "🏛️", &["建筑物", "楼房", "城市建筑"], DEFAULT_THRESHOLD),
    ("车辆", "🚗", &["汽车", "车辆", "摩托车"], DEFAULT_THRESHOLD),
    ("日落晚霞", "🌅", &["日落", "晚霞", "夕阳"], DEFAULT_THRESHOLD),
    ("雪景", "❄️", &["雪景", "雪山", "积雪"], DEFAULT_THRESHOLD),
    ("海边", "🌊", &["大海", "海边", "海滩"], DEFAULT_THRESHOLD),
    ("夜景灯光", "🌃", &["夜景", "夜晚的城市", "灯光"], DEFAULT_THRESHOLD),
    ("文字截图", "📱", &["文字", "文档", "屏幕截图"], DEFAULT_THRESHOLD),
    ("聚会合影", "🥂", &["聚会", "合影", "一群人"], DEFAULT_THRESHOLD),
    ("儿童", "🧒", &["婴儿", "小孩", "儿童"], DEFAULT_THRESHOLD),
    ("运动", "🏃", &["运动", "体育比赛", "跑步"], DEFAULT_THRESHOLD),
];

// ---------------------------------------------------------------------------
// DTO
// ---------------------------------------------------------------------------

/// 分类重建 / 预览报告
#[derive(Debug, Clone, Serialize)]
pub struct RebuildReport {
    pub categories: usize,
    pub hits: usize,
    pub ms: u64,
    pub model: String,
    /// 参与匹配的照片数（当前档位模型下的向量数）
    pub indexed: usize,
    /// 是否因索引为空而跳过（前端据此提示先做语义扫描）
    pub empty_index: bool,
}

/// 预览样张
#[derive(Debug, Clone, Serialize)]
pub struct CategoryPreviewSample {
    pub photo_hash: String,
    pub path: String,
    /// 归属相册（前端 get_photo_thumbs 取缩略图）
    pub album_id: Option<i64>,
    pub score: f64,
    pub matched_keyword: String,
}

/// 分类预览（改关键词/拖阈值时实时看命中数与样张，不落库）
#[derive(Debug, Clone, Serialize)]
pub struct CategoryPreview {
    /// 当前阈值下的命中数
    pub count: i64,
    /// 参与匹配的照片总数（= 已索引照片数）
    pub total: i64,
    /// 分数分布分位（帮助用户理解阈值位置）
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
    /// 命中集内强度最高的若干张（前端缩略图预览）
    pub samples: Vec<CategoryPreviewSample>,
    pub model: String,
}

// ---------------------------------------------------------------------------
// 内部：向量解析与匹配
// ---------------------------------------------------------------------------

/// 一张照片的内存向量行
struct PhotoVec {
    hash: String,
    path: String,
    /// 归属相册（预览样张取缩略图要用）
    album_id: Option<i64>,
    vec: Vec<f32>,
}

/// 收集某分类参与匹配的全部文本（正向 + 排除）
fn category_texts(input: &CategoryInput) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in input.keywords.iter().chain(input.exclude_keywords.iter()) {
        let t = k.trim();
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// 文本 → 向量：先查 `clip_text_cache`，缺失的批量走 CLIP 文本塔并回写缓存
async fn resolve_text_vectors(
    app: &tauri::AppHandle,
    state: &AppState,
    model: &str,
    texts: &[String],
) -> Result<HashMap<String, Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(HashMap::new());
    }
    let cached = {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        db.load_text_vectors(model, texts).map_err(|e| format!("{e}"))?
    };
    let missing: Vec<String> = texts
        .iter()
        .filter(|t| !cached.contains_key(*t))
        .cloned()
        .collect();
    let mut out = cached;
    if missing.is_empty() {
        return Ok(out);
    }
    logger::log_info(&format!(
        "[category] 关键词向量缺失 {} 条，调用 CLIP 文本塔编码（model={model}）",
        missing.len()
    ));
    let encoded = crate::vision::embed_text_batch(&missing, app).await?;
    let mut to_save: Vec<(String, Vec<f32>)> = Vec::new();
    for (text, vec) in encoded {
        out.insert(text.clone(), vec.clone());
        to_save.push((text, vec));
    }
    if !to_save.is_empty() {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        db.save_text_vectors(model, &to_save).map_err(|e| format!("{e}"))?;
    }
    Ok(out)
}

/// 加载当前档位模型的全部照片向量
fn load_photo_vecs(state: &AppState, user_id: i64, model: &str) -> Result<Vec<PhotoVec>, String> {
    let db = state.0.lock().map_err(|e| format!("{e}"))?;
    let rows = db
        .load_all_embeddings(user_id, model)
        .map_err(|e| format!("读取语义向量失败: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(hash, album_id, path, vec)| PhotoVec {
            hash,
            path,
            album_id,
            vec,
        })
        .collect())
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::MIN;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// 中性基线：每张图对 7 条中性提示的平均余弦
fn neutral_base(photos: &[PhotoVec], neutral: &[Vec<f32>]) -> Vec<f32> {
    photos
        .iter()
        .map(|p| {
            if neutral.is_empty() {
                0.0
            } else {
                neutral.iter().map(|b| dot(&p.vec, b)).sum::<f32>() / neutral.len() as f32
            }
        })
        .collect()
}

/// 单分类打分：返回 (命中行, 全库分数向量)
fn score_category(
    photos: &[PhotoVec],
    base: &[f32],
    category_id: i64,
    user_id: i64,
    input: &CategoryInput,
    vecs: &HashMap<String, Vec<f32>>,
) -> (Vec<CategoryHitRecord>, Vec<f32>) {
    let pos: Vec<(&str, &Vec<f32>)> = input
        .keywords
        .iter()
        .filter_map(|k| vecs.get(k.trim()).map(|v| (k.trim(), v)))
        .collect();
    let neg: Vec<&Vec<f32>> = input
        .exclude_keywords
        .iter()
        .filter_map(|k| vecs.get(k.trim()))
        .collect();
    let mut hits: Vec<CategoryHitRecord> = Vec::new();
    let mut scores: Vec<f32> = vec![0.0; photos.len()];
    if pos.is_empty() {
        return (hits, scores);
    }
    for (i, p) in photos.iter().enumerate() {
        let mut best = f32::MIN;
        let mut best_kw = "";
        for (kw, v) in &pos {
            let s = dot(&p.vec, v);
            if s > best {
                best = s;
                best_kw = kw;
            }
        }
        let rel = best - base[i];
        scores[i] = rel;
        if (rel as f64) < input.threshold {
            continue;
        }
        // 排除词：任一排除词相似度不低于正向最高分（留余量）→ 判为误召回
        if !neg.is_empty() {
            let worst = neg.iter().map(|v| dot(&p.vec, v)).fold(f32::MIN, f32::max);
            if worst >= best - EXCLUDE_MARGIN as f32 {
                continue;
            }
        }
        hits.push(CategoryHitRecord {
            category_id,
            user_id,
            photo_hash: p.hash.clone(),
            path: p.path.clone(),
            score: rel as f64,
            matched_keyword: best_kw.to_string(),
        });
    }
    (hits, scores)
}

fn percentile(sorted: &[f32], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[idx.min(sorted.len() - 1)] as f64
}

/// 当前生效语义档位标识（服务不可达时报错，调用方据此提示）
pub async fn current_model_id(app: &tauri::AppHandle) -> Result<String, String> {
    crate::vision::clip_model_id(app).await
}

// ---------------------------------------------------------------------------
// 对外服务
// ---------------------------------------------------------------------------

/// 重建全部语义分类命中（扫描完成后自动调用 / 用户手动「重建」）
pub async fn rebuild_semantic_hits(
    app: &tauri::AppHandle,
    state: &AppState,
    user_id: i64,
) -> Result<RebuildReport, String> {
    rebuild_inner(app, state, user_id, None).await
}

/// 重建单个分类命中
pub async fn rebuild_one(
    app: &tauri::AppHandle,
    state: &AppState,
    user_id: i64,
    category_id: i64,
) -> Result<RebuildReport, String> {
    rebuild_inner(app, state, user_id, Some(category_id)).await
}

async fn rebuild_inner(
    app: &tauri::AppHandle,
    state: &AppState,
    user_id: i64,
    only: Option<i64>,
) -> Result<RebuildReport, String> {
    let t0 = Instant::now();
    let model = current_model_id(app).await?;
    let cats = {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        db.list_categories(user_id).map_err(|e| format!("{e}"))?
    };
    let targets: Vec<_> = cats
        .into_iter()
        .filter(|c| only.map(|id| id == c.id).unwrap_or(true))
        .collect();

    // 1. builtin（规则分类）：SQL 直算，不跑 CLIP
    let mut total_hits = 0usize;
    let mut semantic_count = 0usize;
    for c in targets.iter().filter(|c| c.source == category::SOURCE_BUILTIN) {
        let n = {
            let db = state.0.lock().map_err(|e| format!("{e}"))?;
            db.replace_builtin_hits(c.id, user_id, &c.slug)
                .map_err(|e| format!("{e}"))?
        };
        total_hits += n;
    }

    // 2. 语义分类
    let sem: Vec<_> = targets
        .iter()
        .filter(|c| c.source != category::SOURCE_BUILTIN && !c.keywords.is_empty())
        .collect();
    let mut indexed = 0usize;
    if !sem.is_empty() {
        let photos = load_photo_vecs(state, user_id, &model)?;
        indexed = photos.len();
        if photos.is_empty() {
            // 索引为空：清空命中，明确告知而非静默
            for c in &sem {
                let db = state.0.lock().map_err(|e| format!("{e}"))?;
                db.replace_category_hits(c.id, user_id, &[])
                    .map_err(|e| format!("{e}"))?;
            }
            return Ok(RebuildReport {
                categories: targets.len(),
                hits: total_hits,
                ms: t0.elapsed().as_millis() as u64,
                model,
                indexed,
                empty_index: true,
            });
        }
        let mut all_texts: Vec<String> = NEUTRAL_PROMPTS.iter().map(|s| s.to_string()).collect();
        for c in &sem {
            for t in c.keywords.iter().chain(c.exclude_keywords.iter()) {
                let t = t.trim().to_string();
                if !t.is_empty() && !all_texts.iter().any(|x| *x == t) {
                    all_texts.push(t);
                }
            }
        }
        let vecs = resolve_text_vectors(app, state, &model, &all_texts).await?;
        let neutral: Vec<Vec<f32>> = NEUTRAL_PROMPTS
            .iter()
            .filter_map(|t| vecs.get(*t).cloned())
            .collect();
        let base = neutral_base(&photos, &neutral);
        for c in &sem {
            let input = CategoryInput {
                name: c.name.clone(),
                icon: c.icon.clone(),
                keywords: c.keywords.clone(),
                exclude_keywords: c.exclude_keywords.clone(),
                threshold: c.threshold,
                sort_order: c.sort_order,
                enabled: c.enabled,
            };
            let (hits, _scores) = score_category(&photos, &base, c.id, user_id, &input, &vecs);
            let n = {
                let db = state.0.lock().map_err(|e| format!("{e}"))?;
                db.replace_category_hits(c.id, user_id, &hits)
                    .map_err(|e| format!("{e}"))?
            };
            total_hits += n;
            semantic_count += 1;
        }
    }

    let report = RebuildReport {
        categories: targets.len(),
        hits: total_hits,
        ms: t0.elapsed().as_millis() as u64,
        model,
        indexed,
        empty_index: false,
    };
    logger::log_info(&format!(
        "[category] 重建完成：分类 {}（语义 {}）· 命中 {} · 索引 {} · {}ms",
        report.categories, semantic_count, report.hits, report.indexed, report.ms
    ));
    let _ = app.emit("category-rebuild-done", &report);
    Ok(report)
}

/// 预览：按给定关键词/阈值实时算命中数与样张，**不落库**
pub async fn preview(
    app: &tauri::AppHandle,
    state: &AppState,
    user_id: i64,
    input: &CategoryInput,
) -> Result<CategoryPreview, String> {
    let model = current_model_id(app).await?;
    let photos = load_photo_vecs(state, user_id, &model)?;
    let total = photos.len() as i64;
    let texts = category_texts(input);
    if photos.is_empty() || texts.is_empty() {
        return Ok(CategoryPreview {
            count: 0,
            total,
            p50: 0.0,
            p90: 0.0,
            p99: 0.0,
            max: 0.0,
            samples: Vec::new(),
            model,
        });
    }
    let mut all_texts: Vec<String> = NEUTRAL_PROMPTS.iter().map(|s| s.to_string()).collect();
    for t in &texts {
        if !all_texts.iter().any(|x| x == t) {
            all_texts.push(t.clone());
        }
    }
    let vecs = resolve_text_vectors(app, state, &model, &all_texts).await?;
    let neutral: Vec<Vec<f32>> = NEUTRAL_PROMPTS
        .iter()
        .filter_map(|t| vecs.get(*t).cloned())
        .collect();
    let base = neutral_base(&photos, &neutral);
    let (mut hits, scores) = score_category(&photos, &base, 0, user_id, input, &vecs);

    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(&sorted, 0.50);
    let p90 = percentile(&sorted, 0.90);
    let p99 = percentile(&sorted, 0.99);
    let max = sorted.last().copied().unwrap_or(0.0) as f64;

    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let album_by_hash: HashMap<&str, Option<i64>> =
        photos.iter().map(|p| (p.hash.as_str(), p.album_id)).collect();
    let samples: Vec<CategoryPreviewSample> = hits
        .iter()
        .take(12)
        .map(|h| CategoryPreviewSample {
            photo_hash: h.photo_hash.clone(),
            path: h.path.clone(),
            album_id: album_by_hash.get(h.photo_hash.as_str()).copied().flatten(),
            score: h.score,
            matched_keyword: h.matched_keyword.clone(),
        })
        .collect();
    let count = hits.len() as i64;
    Ok(CategoryPreview {
        count,
        total,
        p50,
        p90,
        p99,
        max,
        samples,
        model,
    })
}

/// 关键词可读性校验（接口层共用）：空名 / 超长 / 阈值区间
fn validate_input(input: &CategoryInput) -> Result<(), String> {
    if input.name.trim().is_empty() {
        return Err("分类名称不能为空".into());
    }
    if input.name.chars().count() > 20 {
        return Err("分类名称不超过 20 个字".into());
    }
    let kw: Vec<&String> = input.keywords.iter().filter(|k| !k.trim().is_empty()).collect();
    if kw.len() > 30 {
        return Err("关键词最多 30 个".into());
    }
    if !(THRESHOLD_MIN..=THRESHOLD_MAX).contains(&input.threshold) {
        return Err(format!(
            "匹配阈值需在 {THRESHOLD_MIN:.2} ~ {THRESHOLD_MAX:.2} 之间"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 接口层（Tauri 命令：参数校验 + 转发 + 出入口日志）
// ---------------------------------------------------------------------------
pub mod commands {
    use super::*;
    use crate::db::CategoryOverviewRow;
    use crate::{logger, require_user, SessionState};
    use tauri::State;

    /// 首次进入自动补齐系统分类与内置预设（幂等）
    fn ensure_defaults(state: &AppState, user_id: i64) -> Result<(), String> {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        if !db.builtin_ready(user_id).unwrap_or(false) {
            db.ensure_builtin_categories(user_id).map_err(|e| format!("{e}"))?;
        }
        db.seed_preset_categories(user_id, PRESETS).map_err(|e| format!("{e}"))?;
        Ok(())
    }

    /// 分类总览（卡片：计数 + 封面 + 关键词）
    #[tauri::command]
    pub async fn list_categories(
        state: State<'_, AppState>,
        session: State<'_, SessionState>,
    ) -> Result<Vec<CategoryOverviewRow>, String> {
        let user_id = require_user(&session)?;
        let _t = logger::log_call_start("list_categories", "");
        let r = (|| -> Result<Vec<CategoryOverviewRow>, String> {
            ensure_defaults(&state, user_id)?;
            let db = state.0.lock().map_err(|e| format!("{e}"))?;
            db.category_overview(user_id).map_err(|e| format!("{e}"))
        })();
        match &r {
            Ok(rows) => logger::log_call_end_with("list_categories", _t, &format!("OK | categories={}", rows.len())),
            Err(e) => logger::log_call_end_with("list_categories", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 新建 / 更新分类（保存后自动重建该分类命中）
    #[tauri::command]
    pub async fn save_category(
        id: Option<i64>,
        input: CategoryInput,
        app: tauri::AppHandle,
        state: State<'_, AppState>,
        session: State<'_, SessionState>,
    ) -> Result<CategoryOverviewRow, String> {
        let user_id = require_user(&session)?;
        validate_input(&input)?;
        let _t = crate::logger::log_call_start("save_category", &format!("id={id:?} name={}", input.name));        let r = async {
            ensure_defaults(&state, user_id)?;
            let cat = {
                let db = state.0.lock().map_err(|e| format!("{e}"))?;
                match id {
                    Some(cid) => {
                        if !db.update_category(cid, user_id, &input).map_err(|e| format!("{e}"))? {
                            return Err("分类不存在".to_string());
                        }
                        db.get_category(cid, user_id).map_err(|e| format!("{e}"))?
                    }
                    None => {
                        let cid = db.create_category(user_id, &input).map_err(|e| format!("{e}"))?;
                        db.get_category(cid, user_id).map_err(|e| format!("{e}"))?
                    }
                }
            }
            .ok_or_else(|| "分类保存后读取失败".to_string())?;

            // 保存即重建（关键词向量已缓存，重建为毫秒~秒级）
            if cat.source != category::SOURCE_BUILTIN {
                match rebuild_one(&app, &state, user_id, cat.id).await {
                    Ok(rep) => logger::log_info(&format!(
                        "[category] 保存后重建 {}：命中 {} · {}ms",
                        cat.name, rep.hits, rep.ms
                    )),
                    Err(e) => logger::log_warn(&format!("[category] 保存后重建失败（不影响保存）：{e}")),
                }
            }
            let db = state.0.lock().map_err(|e| format!("{e}"))?;
            let rows = db.category_overview(user_id).map_err(|e| format!("{e}"))?;
            rows.into_iter()
                .find(|r| r.id == cat.id)
                .ok_or_else(|| "分类保存后读取失败".to_string())
        }
        .await;
        match &r {
            Ok(row) => crate::logger::log_call_end_with(
                "save_category",
                _t,
                &format!("OK | id={} count={}", row.id, row.count),
            ),
            Err(e) => crate::logger::log_call_end_with("save_category", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 删除分类（系统分类不可删）
    #[tauri::command]
    pub async fn delete_category(
        id: i64,
        state: State<'_, AppState>,
        session: State<'_, SessionState>,
    ) -> Result<bool, String> {
        let user_id = require_user(&session)?;
        let _t = crate::logger::log_call_start("delete_category", &format!("id={id}"));
        let r = {
            let db = state.0.lock().map_err(|e| format!("{e}"))?;
            db.delete_category(id, user_id).map_err(|e| format!("{e}"))
        };
        match &r {
            Ok(ok) => crate::logger::log_call_end_with("delete_category", _t, &format!("OK | deleted={ok}")),
            Err(e) => crate::logger::log_call_end_with("delete_category", _t, &format!("ERR | {e}")),
        }
        r.map_err(|e| e.to_string())
    }

    /// 语义预览（改关键词/拖阈值实时看命中数与样张，不落库）
    #[tauri::command]
    pub async fn preview_category(
        input: CategoryInput,
        app: tauri::AppHandle,
        state: State<'_, AppState>,
        session: State<'_, SessionState>,
    ) -> Result<CategoryPreview, String> {
        let user_id = require_user(&session)?;
        let _t = crate::logger::log_call_start("preview_category", &format!("kw={:?}", input.keywords));
        let r = preview(&app, &state, user_id, &input).await;
        match &r {
            Ok(p) => crate::logger::log_call_end_with(
                "preview_category",
                _t,
                &format!("OK | count={} total={}", p.count, p.total),
            ),
            Err(e) => crate::logger::log_call_end_with("preview_category", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 重建分类命中（id=None → 全部重建；builtin 由 SQL 直算）
    #[tauri::command]
    pub async fn rebuild_categories(
        id: Option<i64>,
        app: tauri::AppHandle,
        state: State<'_, AppState>,
        session: State<'_, SessionState>,
    ) -> Result<RebuildReport, String> {
        let user_id = require_user(&session)?;
        let _t = crate::logger::log_call_start("rebuild_categories", &format!("id={id:?}"));
        let r = rebuild_inner(&app, &state, user_id, id).await;
        match &r {
            Ok(rep) => crate::logger::log_call_end_with(
                "rebuild_categories",
                _t,
                &format!("OK | cats={} hits={} indexed={} {}ms", rep.categories, rep.hits, rep.indexed, rep.ms),
            ),
            Err(e) => crate::logger::log_call_end_with("rebuild_categories", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 某分类下的照片（按语义强度降序）
    #[tauri::command]
    pub async fn list_category_photos(
        id: i64,
        limit: Option<i64>,
        state: State<'_, AppState>,
        session: State<'_, SessionState>,
    ) -> Result<Vec<crate::db::CategoryPhotoRow>, String> {
        let user_id = require_user(&session)?;
        let _t = crate::logger::log_call_start("list_category_photos", &format!("id={id}"));
        let r = {
            let db = state.0.lock().map_err(|e| format!("{e}"))?;
            db.list_category_photos(id, user_id, limit.unwrap_or(2000).clamp(1, 20000))
                .map_err(|e| format!("{e}"))
        };
        match &r {
            Ok(rows) => crate::logger::log_call_end_with(
                "list_category_photos",
                _t,
                &format!("OK | photos={}", rows.len()),
            ),
            Err(e) => crate::logger::log_call_end_with("list_category_photos", _t, &format!("ERR | {e}")),
        }
        r.map_err(|e| e.to_string())
    }

    /// 语义索引覆盖统计（「已索引 N/M」+ 换档提醒）
    #[tauri::command]
    pub async fn category_index_stats(
        app: tauri::AppHandle,
        state: State<'_, AppState>,
        session: State<'_, SessionState>,
    ) -> Result<crate::db::CategoryIndexStats, String> {
        let user_id = require_user(&session)?;
        let _t = crate::logger::log_call_start("category_index_stats", "");
        let r = async {
            let model = current_model_id(&app).await?;
            let db = state.0.lock().map_err(|e| format!("{e}"))?;
            db.category_index_stats(user_id, &model).map_err(|e| format!("{e}"))
        }
        .await;
        match &r {
            Ok(s) => crate::logger::log_call_end_with(
                "category_index_stats",
                _t,
                &format!("OK | indexed={}/{} stale={} model={}", s.indexed, s.known, s.stale, s.model),
            ),
            Err(e) => crate::logger::log_call_end_with("category_index_stats", _t, &format!("ERR | {e}")),
        }
        r.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pv(hash: &str, v: [f32; 2]) -> PhotoVec {
        PhotoVec {
            hash: hash.into(),
            path: format!("/p/{hash}.jpg"),
            album_id: Some(1),
            vec: v.to_vec(),
        }
    }

    fn form(kws: &[&str], ex: &[&str], thr: f64) -> CategoryInput {
        CategoryInput {
            name: "t".into(),
            icon: String::new(),
            keywords: kws.iter().map(|s| s.to_string()).collect(),
            exclude_keywords: ex.iter().map(|s| s.to_string()).collect(),
            threshold: thr,
            sort_order: 0,
            enabled: true,
        }
    }

    fn vecs(items: &[(&str, [f32; 2])]) -> HashMap<String, Vec<f32>> {
        items.iter().map(|(k, v)| ((*k).to_string(), v.to_vec())).collect()
    }

    /// 打分公式：score = max_kw cos(x,kw) - 中性基线；阈值过滤 + 记录命中关键词
    #[test]
    fn score_is_gain_over_neutral_baseline() {
        let photos = vec![pv("x1", [1.0, 0.0]), pv("x2", [0.0, 1.0]), pv("x3", [0.6, 0.8])];
        let neutral = vec![vec![1.0f32, 0.0]];
        let base = neutral_base(&photos, &neutral);
        assert!((base[0] - 1.0).abs() < 1e-6);
        assert!((base[1] - 0.0).abs() < 1e-6);
        assert!((base[2] - 0.6).abs() < 1e-6);

        let v = vecs(&[("k", [1.0, 0.0]), ("k2", [0.0, 1.0])]);
        let (hits, scores) = score_category(&photos, &base, 7, 1, &form(&["k", "k2"], &[], 0.03), &v);
        // x1: max(1,0)=1 - 1 = 0 → 不命中（与中性描述无差别）
        // x2: max(0,1)=1 - 0 = 1 → 命中
        // x3: max(0.6,0.8)=0.8 - 0.6 = 0.2 → 命中
        assert!((scores[0] - 0.0).abs() < 1e-6);
        assert!((scores[1] - 1.0).abs() < 1e-6);
        assert!((scores[2] - 0.2).abs() < 1e-6);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].photo_hash, "x2");
        assert_eq!(hits[0].matched_keyword, "k2");
        assert_eq!(hits[1].photo_hash, "x3");
        assert_eq!(hits[0].category_id, 7);
    }

    /// 阈值越高命中越少（单调整性）；关键词为空时不命中
    #[test]
    fn threshold_and_empty_keywords() {
        let photos = vec![pv("a", [0.0, 1.0]), pv("b", [0.5, 0.866])];
        let base = vec![0.0f32, 0.0];
        let v = vecs(&[("k", [0.0, 1.0])]);
        let lo = score_category(&photos, &base, 1, 1, &form(&["k"], &[], 0.01), &v).0;
        let hi = score_category(&photos, &base, 1, 1, &form(&["k"], &[], 1.05), &v).0;
        assert_eq!(lo.len(), 2);
        assert_eq!(hi.len(), 0, "阈值高于最高分时不命中");
        let none = score_category(&photos, &base, 1, 1, &form(&[], &[], 0.0), &v).0;
        assert!(none.is_empty());
    }

    /// 排除词：与正向同强时判为误召回（压「热狗∈狗」这类）
    #[test]
    fn exclude_keywords_suppress_false_hits() {
        let photos = vec![pv("hotdog", [1.0, 0.0]), pv("dog", [0.9, 0.435])];
        let base = vec![0.0f32, 0.0];
        let v = vecs(&[("狗", [1.0, 0.0]), ("热狗", [1.0, 0.0])]);
        let (hits, _) = score_category(&photos, &base, 1, 1, &form(&["狗"], &["热狗"], 0.01), &v);
        assert!(hits.is_empty(), "排除词与正向同分时应被压掉");

        // 只有弱排除词相似度时才保留（dog: 排除词 0.9 = 正向 0.9 → 仍被压）
        let v2 = vecs(&[("狗", [1.0, 0.0]), ("热狗", [0.2, 0.98])]);
        let (hits2, _) = score_category(&photos, &base, 1, 1, &form(&["狗"], &["热狗"], 0.01), &v2);
        assert_eq!(hits2.len(), 2, "排除词明显更弱时不应误压");
    }

    #[test]
    fn percentile_handles_edges() {
        assert_eq!(percentile(&[], 0.5), 0.0);
        let v = vec![0.0, 1.0, 2.0, 3.0, 4.0];
        assert!((percentile(&v, 0.0) - 0.0).abs() < 1e-9);
        assert!((percentile(&v, 1.0) - 4.0).abs() < 1e-9);
        assert!((percentile(&v, 0.5) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn validate_rejects_bad_input() {
        assert!(validate_input(&form(&["k"], &[], 0.03)).is_ok());
        assert!(validate_input(&CategoryInput { name: "  ".into(), ..form(&["k"], &[], 0.03) }).is_err());
        assert!(validate_input(&CategoryInput { name: "x".repeat(21), ..form(&["k"], &[], 0.03) }).is_err());
        assert!(validate_input(&form(&["k"], &[], 0.5)).is_err(), "阈值超范围应拒绝");
    }
}
