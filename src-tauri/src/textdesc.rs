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

use std::path::Path;

use crate::db::SmartHit;
use crate::{AppState, SessionState};

/// 语义相似度下限（与 `content.rs` 既有语义通道同阈值口径）
pub const MIN_SIM_DEFAULT: f64 = 0.30;
/// 以图搜图默认返回条数
const IMAGE_TOPK_DEFAULT: usize = 60;

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
