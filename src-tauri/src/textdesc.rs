//! FEAT-067：智能搜索的两条新通道 —— 以图搜图（图像塔）与描述向量（文本塔）
//!
//! ## 定稿决策（任务书第 0 节，勿改）
//! - **人物编号不进向量**：`P001` 检索准确率 10%（≈随机）、`N号人` 60~70%，均未达 90% 门槛；
//!   人物一律走 `faces.person_id` 精确过滤（人脸识别 100% 准确）。
//! - 描述只放「时间 / 地点 / 场景 / 用户标签 / 人物真名」，真名可配置开关（默认开）。
//! - 扫描与嵌入分两步：扫描先产出地点/时间，嵌入后跑；重算靠 `source_hash` 判定增量。
//!
//! ## 分层
//! - 纯计算（`rank_by_cosine` / 描述拼接 / `source_hash`）：无 IO，单测直接覆盖
//! - 持久化：`db::embedding` 的 `photo_text_embeddings`
//! - 向量：一律走项目自己的文本塔/图像塔（`vision` → VCR 服务），不另下模型

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use serde::Serialize;

use crate::db::embedding::TextEmbeddingRecord;
use crate::db::SmartHit;
use crate::{AppState, SessionState};

/// 语义相似度下限（与 `content.rs` 既有语义通道同阈值口径）
pub const MIN_SIM_DEFAULT: f64 = 0.30;
/// 以图搜图默认返回条数
const IMAGE_TOPK_DEFAULT: usize = 60;

// ---------------------------------------------------------------------------
// 描述（文本塔输入）
// ---------------------------------------------------------------------------

/// 描述分段分隔符（固定顺序 + 固定分隔符；空字段整体跳过，不留空分隔符）
const DESC_SEP: &str = " · ";
/// `source_hash` 的内部拼接分隔符（控制字符，正常字段里不可能出现）
const HASH_SEP: char = '\u{1f}';
/// 文本种类：目前只有「多模态描述」一路
pub const DESC_KIND: &str = "desc";
/// 无意义场景词：`content` 里大量冗余（"other other 其他 其他"），进向量只会稀释语义
const SCENE_STOPWORDS: &[&str] = &[
    "其他", "other", "未知", "unknown", "none", "null", "无", "未识别", "n/a",
];

/// 描述输入：参与拼接的全部字段（**不含人物编号**，人物靠 faces 精确 join）
#[derive(Debug, Clone, Default)]
pub struct DescInput {
    pub shoot_time: Option<String>,
    pub location: Option<String>,
    /// 清洗后的场景（`content` 去停用词 / 去人物编号 / 去重）
    pub scene: String,
    pub user_tags: Vec<String>,
    /// 该照片人物的**真名**列表（有真名才填；编号一律不进描述）
    pub person_names: Vec<String>,
}

/// 配置键：描述里是否嵌入人物真名（**默认开**；关掉则描述完全不含人物）
pub const CFG_PERSON_NAME_IN_DESC: &str = "search.person_name_in_desc";

/// 开关读取（未落库 = 默认开；`"0"` = 关）
pub fn person_name_in_desc(db: &crate::db::Database) -> bool {
    db.get_setting(CFG_PERSON_NAME_IN_DESC)
        .unwrap_or(None)
        .map(|v| v != "0")
        .unwrap_or(true)
}

/// 真名判定：`persons.name` 等于编号本身 = 用户没命名，不算真名
///
/// 这一步是「描述里不写 P 编号」的守门员：未命名人物的名字就是 `P001`，
/// 直接嵌入等于绕开定稿决策①。
pub fn real_name(id: &str, name: &str) -> Option<String> {
    let n = name.trim();
    if n.is_empty() || n == id.trim() {
        None
    } else {
        Some(n.to_string())
    }
}

/// 人物 id → 真名映射（只含有真名的人物；人物库缺失 → 空表，不影响主流程）
fn person_name_map() -> HashMap<String, String> {
    crate::persons::list_persons()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| real_name(&p.id, &p.name).map(|n| (p.id, n)))
        .collect()
}

/// 某照片的人物真名列表（按 person_ids 顺序，去重保序；无真名的人物直接不出现）
pub fn person_names_of(ids: &[String], map: &HashMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if let Some(n) = map.get(id) {
            if !out.iter().any(|x| x == n) {
                out.push(n.clone());
            }
        }
    }
    out
}

/// JSON 数组文本 → 字符串列表（空/异常 → 空列表）
pub fn parse_json_list(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

/// 人物编号判定（`P001` / `p001`）—— 定稿决策①：编号不进向量
fn is_person_code(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() >= 2 && (b[0] == b'p' || b[0] == b'P') && b[1..].iter().all(|c| c.is_ascii_digit())
}

/// 场景清洗：按空白与「·」切分 → 去停用词 / 去人物编号 → 去重保序 → 空格连接
///
/// `other other 其他 其他` → `""`（整段被判无意义，描述里该段整体跳过）
pub fn clean_scene(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in content.split(|c: char| c.is_whitespace() || c == '·') {
        let t = raw.trim().to_lowercase();
        if t.is_empty() || SCENE_STOPWORDS.contains(&t.as_str()) || is_person_code(&t) {
            continue;
        }
        if !out.iter().any(|x| *x == t) {
            out.push(t);
        }
    }
    out.join(" ")
}

/// 时间分段：`YYYY-MM-DD HH:MM:SS` → `YYYY年M月`；无年份只有月份 → 季节
pub fn time_segment(shoot_time: Option<&str>) -> String {
    let Some(s) = shoot_time.map(|x| x.trim()).filter(|x| !x.is_empty()) else {
        return String::new();
    };
    let date = s.split(' ').next().unwrap_or("");
    let parts: Vec<&str> = date.split('-').collect();
    let year: Option<i32> = parts.first().and_then(|x| x.parse().ok()).filter(|y| *y > 1900);
    let month: Option<u32> = parts
        .get(1)
        .and_then(|x| x.parse().ok())
        .filter(|m| (1..=12).contains(m));
    match (year, month) {
        (Some(y), Some(m)) => format!("{y}年{m}月"),
        (None, Some(m)) => season_of(m).to_string(),
        _ => String::new(),
    }
}

fn season_of(month: u32) -> &'static str {
    match month {
        3..=5 => "春",
        6..=8 => "夏",
        9..=11 => "秋",
        _ => "冬",
    }
}

/// 描述分段（固定顺序，空段不出现）：时间 / 地点 / 场景 / 用户标签 / 人物真名
fn desc_segments(input: &DescInput) -> Vec<String> {
    let mut segs: Vec<String> = Vec::new();
    let time = time_segment(input.shoot_time.as_deref());
    if !time.is_empty() {
        segs.push(time);
    }
    if let Some(loc) = input.location.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        segs.push(loc.to_string());
    }
    if !input.scene.trim().is_empty() {
        segs.push(input.scene.trim().to_string());
    }
    let tags: Vec<&str> = input
        .user_tags
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if !tags.is_empty() {
        segs.push(tags.join("、"));
    }
    let names: Vec<&str> = input
        .person_names
        .iter()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .collect();
    if !names.is_empty() {
        segs.push(names.join("、"));
    }
    segs
}

/// 描述文本（进文本塔的原文）：`2025年11月 · 杭州西湖 · 夜景 · 小明、小红`
pub fn build_desc(input: &DescInput) -> String {
    desc_segments(input).join(DESC_SEP)
}

/// 参与拼接字段的指纹（FNV-1a 64 位）
///
/// 指纹基于**描述分段本身**（而非原始字段）计算：`shoot_time` 从 11:12:34 变成
/// 11:12:35 时描述仍是「2024年2月」，不该触发重算。字段没变 → 跳过，不重算。
pub fn source_hash(input: &DescInput) -> String {
    let joined = desc_segments(input).join(&HASH_SEP.to_string());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in joined.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 余弦 topK（纯函数，便于单测）
///
/// - 向量已由服务端 L2 归一化，点积即余弦
/// - 维度不一致的行直接跳过（换档位/脏数据保护）
/// - 返回按余弦降序，`limit` 截断、`min_sim` 过滤
fn rank_by_cosine(
    items: &[(String, Vec<f32>)],
    q: &[f32],
    limit: usize,
    min_sim: f64,
) -> Vec<(String, f64)> {
    let mut scored: Vec<(f64, &String)> = items
        .iter()
        .filter_map(|(hash, v)| {
            if v.len() != q.len() || v.is_empty() {
                return None;
            }
            let dot = v.iter().zip(q.iter()).map(|(a, b)| (*a * *b) as f64).sum::<f64>();
            if dot >= min_sim {
                Some((dot, hash))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored.into_iter().map(|(s, h)| (h.clone(), s)).collect()
}

/// 就地 L2 归一化（服务端已归一化，这里只防御性兜底：未归一化的向量会让阈值失真）
fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| *x * x).sum::<f32>().sqrt();
    if norm > 1e-6 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// 查询图的编码输入：优先复用该照片的缩略图
///
/// 建索引时编码的是缩略图（见 `content::scan_album_embeddings`），查询时用同一份输入
/// 才能保证「拿已索引的图查它自己」余弦 = 1.0（验收第 3 条）。缩略图缺失时回退原图。
fn query_image_input(state: &AppState, path: &str) -> String {
    let p = Path::new(path);
    let meta = std::fs::metadata(p).ok().map(|md| {
        (
            md.len(),
            md.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        )
    });
    let Some((len, mtime)) = meta else {
        return path.to_string();
    };
    let hash = crate::content::photo_hash(path, len, mtime);
    let thumbs = {
        let db = match state.0.lock() {
            Ok(db) => db,
            Err(_) => return path.to_string(),
        };
        db.lookup_thumb_caches(&[hash]).unwrap_or_default()
    };
    let hit = thumbs
        .into_iter()
        .find(|t| !t.thumb_path.is_empty() && Path::new(&t.thumb_path).is_file());
    match hit {
        Some(t) => t.thumb_path,
        None => path.to_string(),
    }
}

/// 以图搜图：图片路径 → 图像塔编码 → 与 `photo_embeddings` 余弦 → topK
///
/// - 复用现有图像向量，**不重建索引**（644 × 512 维全表点积为微秒级）
/// - 阈值/硬上限沿用语义通道口径；失败（服务未就绪/编码失败）返回 Err，由调用方提示
pub async fn recall_by_image(
    app: &tauri::AppHandle,
    state: &AppState,
    user_id: i64,
    image_path: &str,
    limit: usize,
    min_similarity: f64,
) -> Result<Vec<SmartHit>, String> {
    if crate::vision::semantic_backoff_active() {
        return Err("CLIP 服务暂不可用（退避中）".into());
    }
    let model = crate::vision::clip_model_id(app).await?;

    // 1. 查询图 → 向量（缩略图优先，保证与建库输入一致）
    let input = query_image_input(state, image_path);
    let mut q = match crate::vision::embed_images_batch(&[input], 1, app, None)
        .await?
        .into_iter()
        .next()
        .and_then(|r| r.embedding)
    {
        Some(v) => v,
        None => return Err("查询图编码失败（图片无法解码？）".into()),
    };
    l2_normalize(&mut q);
    crate::vision::clear_semantic_down();

    // 2. 全库图像向量（按当前档位模型隔离）
    let all = {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        db.load_all_embeddings(user_id, &model)
            .map_err(|e| format!("读取图像向量失败: {e}"))?
    };
    let items: Vec<(String, Vec<f32>)> =
        all.into_iter().map(|(hash, _album, _path, v)| (hash, v)).collect();
    if items.is_empty() {
        return Err("图像向量索引为空（请先勾选「语义向量」做一次扫描）".into());
    }

    // 3. 余弦 topK → 批量取展示字段（lookup_hits_by_hashes 按入参顺序返回）
    let ranked = rank_by_cosine(&items, &q, limit.max(1), min_similarity);
    let hashes: Vec<String> = ranked.iter().map(|(h, _)| h.clone()).collect();
    let mut hits = {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        db.lookup_hits_by_hashes(user_id, &hashes)
            .map_err(|e| format!("{e}"))?
    };
    for (hit, (_h, score)) in hits.iter_mut().zip(ranked.iter()) {
        hit.semantic_score = Some(*score);
    }
    Ok(hits)
}

/// 描述索引重建报告
#[derive(Debug, Clone, Serialize)]
pub struct DescBuildReport {
    /// 参与的照片数（= 该用户 photo_content_scan 行数）
    pub total: usize,
    /// 本次实际编码写入的条数
    pub built: usize,
    /// `source_hash` 未变、跳过不重算的条数
    pub unchanged: usize,
    /// 描述为空（无任何可写字段）未纳入的条数
    pub empty: usize,
    /// 本次去重后送去编码的不同描述数
    pub unique_texts: usize,
    pub ms: u64,
    pub model: String,
}

/// 当前时刻（Unix 毫秒字符串）
///
/// 秒级精度无法区分「同一秒内的重算」，验收第 6 条要看 `generated_at` 是否变化，
/// 故这里用毫秒。
fn now_ms() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

/// 重建描述向量索引（**真正的增量**：只算 `source_hash` 变了的那些）
///
/// 流程：读全部已扫描照片 → 逐张拼描述 + 算指纹 → 与库内 `source_hash` 比对
/// （相同则跳过）→ 去重后的描述批量走文本塔 → upsert。
///
/// 因此改名 / 改地点 / 改标签后重跑，只有受影响的那批照片会被重算，
/// 其余照片的向量**一动不动**（字节级不变）。
pub async fn rebuild_text_index(
    app: &tauri::AppHandle,
    state: &AppState,
    user_id: i64,
) -> Result<DescBuildReport, String> {
    let t0 = Instant::now();
    let model = crate::vision::clip_model_id(app).await?;

    // 1. 短锁：素材 + 已有指纹
    let (rows, existing) = {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        (
            db.load_desc_sources(user_id).map_err(|e| format!("读取描述素材失败: {e}"))?,
            db.text_embedding_source_hashes(DESC_KIND, &model)
                .map_err(|e| format!("读取描述向量指纹失败: {e}"))?,
        )
    };

    // 2. 真名开关（默认开）+ 人物真名映射（关掉 → 描述完全不含人物）
    let include_names = {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        person_name_in_desc(&db)
    };
    let name_map: HashMap<String, String> = if include_names {
        person_name_map()
    } else {
        HashMap::new()
    };

    // 3. 逐张算描述与指纹，挑出需要重算的
    let mut pending: Vec<(String, String, String)> = Vec::new(); // (photo_hash, text, source_hash)
    let mut unchanged = 0usize;
    let mut empty = 0usize;
    for r in &rows {
        let input = DescInput {
            shoot_time: r.shoot_time.clone(),
            location: r.location.clone(),
            scene: clean_scene(r.content.as_deref().unwrap_or("")),
            user_tags: parse_json_list(r.user_tags.as_deref()),
            person_names: if include_names {
                person_names_of(&parse_json_list(r.person_ids.as_deref()), &name_map)
            } else {
                Vec::new()
            },
        };
        let text = build_desc(&input);
        if text.is_empty() {
            empty += 1;
            continue;
        }
        let sh = source_hash(&input);
        if existing.get(&r.photo_hash).map(|old| old == &sh).unwrap_or(false) {
            unchanged += 1;
            continue;
        }
        pending.push((r.photo_hash.clone(), text, sh));
    }

    let report = |built: usize, unique_texts: usize| DescBuildReport {
        total: rows.len(),
        built,
        unchanged,
        empty,
        unique_texts,
        ms: t0.elapsed().as_millis() as u64,
        model: model.clone(),
    };

    if pending.is_empty() {
        return Ok(report(0, 0));
    }

    // 4. 描述去重后批量编码（同一段描述只编码一次，653 张通常只有几十种）
    let mut texts: Vec<String> = Vec::new();
    for (_, t, _) in &pending {
        if !texts.iter().any(|x| x == t) {
            texts.push(t.clone());
        }
    }
    let unique_texts = texts.len();
    let encoded = crate::vision::embed_text_batch(&texts, app).await?;
    let by_text: HashMap<String, Vec<f32>> = encoded.into_iter().collect();

    // 5. 落库（只写真正重算的那些行）
    let stamp = now_ms();
    let mut recs: Vec<TextEmbeddingRecord> = Vec::with_capacity(pending.len());
    for (hash, text, sh) in &pending {
        let Some(vec) = by_text.get(text) else {
            continue; // 该条编码失败 → 不写库，下次再补（指纹未落库 = 未算）
        };
        recs.push(TextEmbeddingRecord {
            photo_hash: hash.clone(),
            kind: DESC_KIND.to_string(),
            text: text.clone(),
            model: model.clone(),
            dim: vec.len() as i64,
            embedding: vec.clone(),
            source_hash: sh.clone(),
            generated_at: stamp.clone(),
        });
    }
    let built = recs.len();
    {
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        db.upsert_text_embeddings(&recs)
            .map_err(|e| format!("描述向量写入失败: {e}"))?;
    }
    Ok(report(built, unique_texts))
}

/// 命令层（薄壳）
pub mod commands {
    use super::*;
    use crate::{logger, require_user};

    /// FEAT-067 步骤 1：以图搜图
    ///
    /// 入参一张本地图片 → 出参相似照片（含现有元数据 + 相似度）。
    /// 结果可直接喂给搜索页结果网格（`SmartHit` 与 `smart_search` 同构）。
    #[tauri::command]
    pub async fn smart_search_by_image(
        path: String,
        limit: Option<i64>,
        min_similarity: Option<f64>,
        app: tauri::AppHandle,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<Vec<SmartHit>, String> {
        let _t = log_call!("smart_search_by_image", &format!("path={path}"));
        let user_id = require_user(&session)?;
        let min_sim = min_similarity.unwrap_or(MIN_SIM_DEFAULT).clamp(0.05, 0.95);
        let topk = limit.unwrap_or(IMAGE_TOPK_DEFAULT as i64).clamp(1, 500) as usize;
        let r = recall_by_image(&app, &state, user_id, &path, topk, min_sim).await;
        match &r {
            Ok(list) => logger::log_call_end_with(
                "smart_search_by_image",
                _t,
                &format!("OK | hits={}", list.len()),
            ),
            Err(e) => logger::log_call_end_with("smart_search_by_image", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// FEAT-067 步骤 4：读「描述是否嵌入人物真名」开关（默认开）
    #[tauri::command]
    pub async fn get_desc_person_name(
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<bool, String> {
        let _ = require_user(&session)?;
        let db = state.0.lock().map_err(|e| format!("{e}"))?;
        Ok(person_name_in_desc(&db))
    }

    /// FEAT-067 步骤 4：切换「描述是否嵌入人物真名」（关掉 → 描述完全不含人物）
    #[tauri::command]
    pub async fn set_desc_person_name(
        enabled: bool,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<bool, String> {
        let _t = log_call!("set_desc_person_name", &format!("enabled={enabled}"));
        let _ = require_user(&session)?;
        let r = {
            let db = state.0.lock().map_err(|e| format!("{e}"))?;
            db.set_setting(CFG_PERSON_NAME_IN_DESC, if enabled { "1" } else { "0" })
                .map(|_| enabled)
                .map_err(|e| format!("{e}"))
        };
        match &r {
            Ok(v) => logger::log_call_end_with("set_desc_person_name", _t, &format!("OK | enabled={v}")),
            Err(e) => logger::log_call_end_with("set_desc_person_name", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// FEAT-067 步骤 2：重建描述向量索引（增量，`source_hash` 未变的照片不重算）
    #[tauri::command]
    pub async fn rebuild_desc_index(
        app: tauri::AppHandle,
        state: tauri::State<'_, AppState>,
        session: tauri::State<'_, SessionState>,
    ) -> Result<DescBuildReport, String> {
        let _t = log_call!("rebuild_desc_index", "");
        let user_id = require_user(&session)?;
        let r = rebuild_text_index(&app, &state, user_id).await;
        match &r {
            Ok(rep) => logger::log_call_end_with(
                "rebuild_desc_index",
                _t,
                &format!(
                    "OK | total={} built={} unchanged={} empty={} texts={} {}ms",
                    rep.total, rep.built, rep.unchanged, rep.empty, rep.unique_texts, rep.ms
                ),
            ),
            Err(e) => logger::log_call_end_with("rebuild_desc_index", _t, &format!("ERR | {e}")),
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(v: Vec<f32>) -> Vec<f32> {
        let mut v = v;
        l2_normalize(&mut v);
        v
    }

    /// 自洽性（验收第 3 条的纯计算部分）：查询向量取自库内某张图时，它自己必须排第一且 ≥0.999
    #[test]
    fn identical_vector_ranks_first_with_cos_one() {
        let a = norm(vec![0.1, 0.2, 0.3, 0.4]);
        let items = vec![
            ("b".to_string(), norm(vec![0.4, 0.3, 0.2, 0.1])),
            ("self".to_string(), a.clone()),
            ("c".to_string(), norm(vec![-0.1, -0.2, 0.9, 0.1])),
        ];
        let out = rank_by_cosine(&items, &a, 10, MIN_SIM_DEFAULT);
        assert_eq!(out[0].0, "self");
        assert!(out[0].1 >= 0.999, "自身余弦应≈1，实际 {}", out[0].1);
        // 降序
        for w in out.windows(2) {
            assert!(w[0].1 >= w[1].1);
        }
    }

    /// 阈值与截断：低于阈值的行不出现；limit 生效
    #[test]
    fn rank_by_cosine_threshold_and_limit() {
        let q = norm(vec![1.0, 0.0]);
        let items = vec![
            ("high".to_string(), norm(vec![1.0, 0.0])),
            ("mid".to_string(), norm(vec![0.5, 0.866])),
            ("low".to_string(), norm(vec![0.0, 1.0])),
        ];
        let all = rank_by_cosine(&items, &q, 10, MIN_SIM_DEFAULT);
        assert_eq!(all.len(), 2, "正交向量余弦 0 应被阈值滤掉");
        assert_eq!(all[0].0, "high");
        let one = rank_by_cosine(&items, &q, 1, MIN_SIM_DEFAULT);
        assert_eq!(one.len(), 1);
        // 维度不一致 / 空向量直接跳过
        let bad = vec![("x".to_string(), vec![1.0]), ("y".to_string(), Vec::new())];
        assert!(rank_by_cosine(&bad, &q, 10, MIN_SIM_DEFAULT).is_empty());
    }

    /// 描述模板（验收第 4 条）：固定顺序 + 固定分隔符，空字段整体跳过
    #[test]
    fn desc_skips_empty_fields_and_keeps_order() {
        let full = DescInput {
            shoot_time: Some("2025-11-03 08:00:00".into()),
            location: Some("杭州西湖".into()),
            scene: "夜景".into(),
            user_tags: vec!["旅行".into()],
            person_names: vec!["小明".into(), "小红".into()],
        };
        assert_eq!(build_desc(&full), "2025年11月 · 杭州西湖 · 夜景 · 旅行 · 小明、小红");
        // 只有时间：不出现行首/行尾多余分隔符
        let only_time = DescInput { shoot_time: Some("2024-02-20 11:12:34".into()), ..Default::default() };
        assert_eq!(build_desc(&only_time), "2024年2月");
        // 中间字段为空：不出现 "· ·"
        let gap = DescInput {
            shoot_time: Some("2024-02-20 11:12:34".into()),
            location: None,
            scene: String::new(),
            user_tags: vec!["  ".into()],
            person_names: vec!["小明".into()],
        };
        let d = build_desc(&gap);
        assert_eq!(d, "2024年2月 · 小明");
        assert!(!d.contains("· ·"));
        assert!(!d.starts_with("·") && !d.ends_with("·"));
        // 全空 → 空描述（不入索引，避免出现无意义向量）
        assert_eq!(build_desc(&DescInput::default()), "");
    }

    /// `content` 清洗（验收第 5 条）：无意义值与人物编号都不进描述
    #[test]
    fn clean_scene_drops_stopwords_and_person_codes() {
        assert_eq!(clean_scene("other other 其他 其他"), "");
        assert_eq!(clean_scene("未知 unknown 未识别"), "");
        // 人物编号不进向量（定稿决策①）
        assert_eq!(clean_scene("portrait closeup 人物特写·1人 人物特写·1人 p001"), "portrait closeup 人物特写 1人");
        // 去重保序
        assert_eq!(clean_scene("street street 扫街·3人 扫街·3人"), "street 扫街 3人");
        assert_eq!(clean_scene(""), "");
    }

    /// 时间分段：有年月 →「YYYY年M月」，只有月 → 季节，无时间 → 空
    #[test]
    fn time_segment_year_month_or_season() {
        assert_eq!(time_segment(Some("2024-02-20 11:12:34")), "2024年2月");
        assert_eq!(time_segment(Some("2025-11-03")), "2025年11月");
        // 年份不可信（0000）只有月份 → 退回季节
        assert_eq!(time_segment(Some("0000-07-20 10:00:00")), "夏");
        assert_eq!(time_segment(None), "");
        assert_eq!(time_segment(Some("not-a-date")), "");
    }

    /// `source_hash` 增量语义（验收第 6 条的核心）：
    /// 只有参与描述的字段变了，指纹才变
    #[test]
    fn source_hash_changes_only_with_desc_fields() {
        let base = DescInput {
            shoot_time: Some("2024-02-20 11:12:34".into()),
            location: Some("成都".into()),
            scene: "夜景".into(),
            ..Default::default()
        };
        let h0 = source_hash(&base);
        // 秒级时间变化不影响「2024年2月」→ 不该触发重算
        let mut same = base.clone();
        same.shoot_time = Some("2024-02-20 11:12:35".into());
        assert_eq!(h0, source_hash(&same), "描述未变就不该重算");
        // 地点变了 → 必须重算
        let mut moved = base.clone();
        moved.location = Some("杭州西湖".into());
        assert_ne!(h0, source_hash(&moved));
        // 场景变了 → 必须重算
        let mut scene2 = base.clone();
        scene2.scene = "人像".into();
        assert_ne!(h0, source_hash(&scene2));
        // 加了真名 → 必须重算（步骤 4 改名重算的基础）
        let mut named = base.clone();
        named.person_names = vec!["小明".into()];
        assert_ne!(h0, source_hash(&named));
    }

    /// JSON 列表解析：异常/空值兜底为空列表
    #[test]
    fn parse_json_list_is_defensive() {
        assert_eq!(parse_json_list(Some(r#"["a","b"]"#)), vec!["a", "b"]);
        assert!(parse_json_list(None).is_empty());
        assert!(parse_json_list(Some("not json")).is_empty());
    }

    /// 真名判定（验收第 7 条的前提）：名字等于编号 = 没命名，不进描述
    #[test]
    fn real_name_rejects_unnamed_persons() {
        assert_eq!(real_name("P001", "P001"), None, "未命名人物不能把编号写进描述");
        assert_eq!(real_name("P001", "  "), None);
        assert_eq!(real_name("P001", "小明"), Some("小明".into()));
        assert_eq!(real_name("P001", " 小明 "), Some("小明".into()));
    }

    /// 真名列表：只含有真名的人物，去重保序；关掉开关 → 完全不含人物
    #[test]
    fn person_names_of_only_named_and_dedup() {
        let mut map = HashMap::new();
        map.insert("P001".to_string(), "小明".to_string());
        map.insert("P002".to_string(), "小红".to_string());
        // P003 未命名 → 不出现
        let ids = vec!["P001".to_string(), "P003".to_string(), "P001".to_string(), "P002".to_string()];
        assert_eq!(person_names_of(&ids, &map), vec!["小明", "小红"]);
        // 空映射（开关关闭 / 无人命名）→ 空
        assert!(person_names_of(&ids, &HashMap::new()).is_empty());
    }

    /// 归一化：非单位向量先归一化再用（否则阈值失真）
    #[test]
    fn normalize_makes_unit_length() {
        let v = norm(vec![3.0, 4.0]);
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-6);
        // 零向量不除零
        let z = norm(vec![0.0, 0.0]);
        assert_eq!(z, vec![0.0, 0.0]);
    }
}
