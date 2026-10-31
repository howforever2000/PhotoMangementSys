//! 重复照片检测与清理（FEAT-143）
//!
//! ## 两类"重复"，两条完全不同的判据
//!
//! 1. **完全重复**（`exact`）：文件字节一致 → 内容哈希一致。
//!    注意项目的 `photo_hash` 是「路径 + 大小 + mtime」的组合哈希（见
//!    `content::photo_hash`），它把同一张照片的两个副本算成**不同**值，
//!    所以这里必须另外算真正的**内容哈希**（SHA-256）。
//! 2. **画面相似**（`similar`）：CLIP 图像塔向量（`photo_embeddings`，
//!    512 维、服务端已 L2 归一化）的余弦相似度 ≥ 阈值。
//!    同一张照片的不同尺寸 / 不同压缩（HEIC ↔ JPEG 导出）落在这一类。
//!
//! ## 为什么相似检测不需要启动 Python 微服务
//!
//! 向量早已落库。这里直接用「行数最多的那个 model 标识」读库
//! （[`crate::db::Database::dominant_embedding_model`]），不调 `/health`、
//! 不触发模型加载 —— 重复检测是**离线批处理**，不该被微服务生命周期绑架。
//! 代价是换档后未重建向量的那批照片可能被跳过，这在报告里如实报数。
//!
//! ## 性能：无损剪枝，不做抽样近似
//!
//! 全库两两比对是 O(N²)。对**单位向量**有严格上界：
//!
//! ```text
//! cos(a,b) ≥ t  ⇒  ‖a-b‖² = 2-2cos(a,b) ≤ 2-2t
//!               ⇒  |a_d - b_d| ≤ ‖a-b‖ ≤ √(2-2t)   （任意维度 d）
//! ```
//!
//! 于是：选**方差最大**的那一维把所有向量排序，对每个 i 只向后比较
//! `|x_j - x_i| ≤ √(2-2t)` 的邻居。这是**数学上无损**的剪枝（不会漏掉任何
//! 真配对），阈值越高剪得越狠（t=0.9 → 窗口 0.447）。
//!
//! ## 分组语义
//!
//! 相似分组用并查集（连通分量），即**传递闭包**：A~B、B~C 会把 A、B、C
//! 放进同一组，即使 A 与 C 的相似度低于阈值。这是清理工具的常规语义
//! （组内至少有一条链路），组内展示的 `similarity` 取
//! **保留那张与其余每一张的最低相似度**，让用户看到最保守的那个数。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// 扫描前统计到的单张照片元信息
#[derive(Debug, Clone)]
struct Entry {
    path: String,
    /// 小写路径（Windows 大小写不敏感，用于去重与关联）
    key: String,
    album_id: i64,
    size_bytes: u64,
    mtime_secs: i64,
    width: Option<u32>,
    height: Option<u32>,
}

/// 组内一张照片（前端展示 + 勾选单位）
#[derive(Debug, Clone, Serialize)]
pub struct DupItem {
    pub path: String,
    pub album_id: i64,
    pub file_name: String,
    pub size_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub modified_secs: i64,
}

/// 一个重复组
#[derive(Debug, Clone, Serialize)]
pub struct DupGroup {
    /// 稳定标识（`exact:<hash>` / `similar:<保留路径>`），前端勾选与刷新用
    pub key: String,
    /// `exact` = 内容哈希一致；`similar` = 向量相似
    pub kind: String,
    /// exact 恒为 1.0；similar 为「保留那张 vs 其余」的最低余弦
    pub similarity: f32,
    /// 默认保留的那张（用户可在组内改选）
    pub keep_path: String,
    /// 若删掉除保留外全部，可释放的字节数
    pub reclaimable_bytes: u64,
    pub items: Vec<DupItem>,
}

/// 一次重复扫描的结果
#[derive(Debug, Clone, Serialize)]
pub struct DedupeReport {
    pub groups: Vec<DupGroup>,
    /// 参与检测的照片总数（全库）
    pub total_photos: usize,
    /// 其中参与相似判定的照片数（有向量且未落入 exact 组）
    pub similar_candidates: usize,
    /// 没有向量、被跳过的照片数（前端据此提示「先生成向量」）
    pub skipped_unencoded: usize,
    /// 精确重复组数
    pub exact_groups: usize,
    /// 相似组数
    pub similar_groups: usize,
    /// 本次使用的阈值（0.80–1.00）
    pub threshold: f32,
    /// 本次使用的向量模型标识（换档后可对照）
    pub embedding_model: Option<String>,
    /// 相似判定被跳过时的原因（无向量 / 无可读模型）
    pub similar_skipped_reason: Option<String>,
    pub elapsed_ms: u64,
    /// HEIC 解码能力（缺 HEIF 扩展时前端提示装扩展）
    pub heic: crate::heic::HeicSupport,
    /// 内容哈希实际计算的文件数（体积预筛后剩下的碰撞候选）
    pub hashed_files: usize,
}

/// 单飞锁：同一时刻只允许一个重复扫描在跑
///
/// 与 `content.rs` 的 `SCAN_ALBUM` 同一思路：前端 `running` 只是 JS 内存状态，
/// 多开窗口 / 页面刷新时互相看不见，只有进程级锁才拦得住。
static DEDUPE_RUNNING: AtomicBool = AtomicBool::new(false);

/// 取消标记（与扫描任务分开，互不干扰）
static DEDUPE_CANCEL: AtomicBool = AtomicBool::new(false);

/// 重复扫描进行中？
pub fn is_running() -> bool {
    DEDUPE_RUNNING.load(Ordering::SeqCst)
}

/// 请求停止当前重复扫描（下一个检查点生效）
pub fn request_cancel() {
    DEDUPE_CANCEL.store(true, Ordering::SeqCst);
    crate::logger::log_info("[dedupe] 收到停止请求，已置取消标记");
}

/// 单飞锁占位（Drop 时释放，异常退出也会解锁）
struct RunGuard;

impl Drop for RunGuard {
    fn drop(&mut self) {
        DEDUPE_RUNNING.store(false, Ordering::SeqCst);
    }
}

fn acquire_run() -> Option<RunGuard> {
    DEDUPE_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| RunGuard)
}

/// 进度事件载荷（前端画阶段条）
#[derive(Clone, Serialize)]
pub struct DedupeProgress {
    /// `collect` / `hash` / `similar` / `done`
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub message: String,
}

// ===========================================================================
// 主流程（阻塞线程内执行）
// ===========================================================================

/// 全库重复扫描：收集 → 精确哈希分组 → 向量相似分组
///
/// - `threshold`：相似度阈值（调用方已 clamp 到 0.70–1.00）
/// - `include_exact` / `include_similar`：两类判据各自可关（关掉省时间）
/// - `on_progress`：阶段进度回调（命令层转成 Tauri 事件）
pub fn scan_library(
    albums: &[(i64, String)],
    embeddings: Vec<(String, Option<i64>, String, Vec<f32>)>,
    embedding_model: Option<String>,
    threshold: f32,
    include_exact: bool,
    include_similar: bool,
    mut on_progress: impl FnMut(&str, usize, usize, &str),
) -> Result<DedupeReport, String> {
    let t0 = std::time::Instant::now();
    let _guard = acquire_run().ok_or_else(|| "已有重复扫描在进行中，请等它结束再试".to_string())?;
    DEDUPE_CANCEL.store(false, Ordering::SeqCst);
    let cancel = |stage: &str| {
        if DEDUPE_CANCEL.load(Ordering::SeqCst) {
            crate::logger::log_info(&format!("[dedupe] 在 {stage} 阶段被用户停止"));
            true
        } else {
            false
        }
    };

    // ---- 1. 收集全库照片 ----
    on_progress("collect", 0, albums.len(), "正在遍历相册目录…");
    let mut entries: Vec<Entry> = Vec::new();
    // 同一路径出现在多个（嵌套）相册目录时，归属取**最深**的相册（最具体）
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (i, (album_id, dir)) in albums.iter().enumerate() {
        if cancel("collect") {
            break;
        }
        let paths = match crate::vision::walk_image_paths(dir) {
            Ok(p) => p,
            Err(e) => {
                crate::logger::log_warn(&format!("[dedupe] 相册 {album_id} 遍历失败，跳过: {e}"));
                on_progress("collect", i + 1, albums.len(), &format!("相册 {album_id} 跳过"));
                continue;
            }
        };
        for p in paths {
            let Some(entry) = build_entry(&p, *album_id) else { continue };
            match seen.get(&entry.key).copied() {
                Some(idx) => {
                    // 已有同路径：谁的相册目录更长（更具体）归谁
                    let prev_dir_len = albums
                        .iter()
                        .find(|(id, _)| *id == entries[idx].album_id)
                        .map(|(_, d)| d.len())
                        .unwrap_or(0);
                    if dir.len() > prev_dir_len {
                        entries[idx].album_id = *album_id;
                    }
                }
                None => {
                    seen.insert(entry.key.clone(), entries.len());
                    entries.push(entry);
                }
            }
        }
        on_progress(
            "collect",
            i + 1,
            albums.len(),
            &format!("已找到 {} 张照片", entries.len()),
        );
    }
    if cancel("collect") {
        return Err("已停止".into());
    }
    let total_photos = entries.len();
    crate::logger::log_info(&format!(
        "[dedupe] 全库收集完成 {total_photos} 张 · {} 个相册",
        albums.len()
    ));

    // ---- 2. 完全重复：体积预筛 + 内容哈希 ----
    let mut groups: Vec<DupGroup> = Vec::new();
    let mut in_exact: HashSet<String> = HashSet::new();
    let mut hashed_files = 0usize;
    if include_exact {
        on_progress("hash", 0, total_photos, "正在比对文件内容…");
        let (exact, hashed) = exact_groups(&entries, &cancel, |done, total| {
            on_progress("hash", done, total, &format!("已比对 {done}/{total} 张"));
        })?;
        hashed_files = hashed;
        for g in &exact {
            for it in &g.items {
                in_exact.insert(it.path.to_lowercase());
            }
        }
        crate::logger::log_info(&format!(
            "[dedupe] 精确重复 {} 组 · 实际计算内容哈希 {hashed_files} 个文件",
            exact.len()
        ));
        groups.extend(exact);
    }

    // ---- 3. 画面相似：向量余弦 ----
    let mut similar_groups = 0usize;
    let mut similar_candidates = 0usize;
    let mut similar_skipped_reason: Option<String> = None;
    let mut skipped_unencoded = 0usize;
    if include_similar {
        // 向量按路径关联到 entry（路径口径与扫描时一致：原图绝对路径）
        let mut vec_by_path: HashMap<String, Vec<f32>> = HashMap::new();
        for (_hash, _album, path, v) in embeddings {
            if v.is_empty() {
                continue;
            }
            vec_by_path.insert(path.to_lowercase(), v);
        }
        let mut cand: Vec<(usize, &Vec<f32>)> = Vec::new();
        for (idx, e) in entries.iter().enumerate() {
            if in_exact.contains(&e.key) {
                continue;
            }
            match vec_by_path.get(&e.key) {
                Some(v) => cand.push((idx, v)),
                None => skipped_unencoded += 1,
            }
        }
        similar_candidates = cand.len();
        if vec_by_path.is_empty() {
            similar_skipped_reason = Some(
                "没有可用的图像向量：请先在「全局扫描入库」里对相册做一次扫描（勾选人物/文档识别或语义向量），再回来做相似检测"
                    .into(),
            );
        } else if cand.len() < 2 {
            similar_skipped_reason = Some(format!(
                "可参与相似判定的照片不足 2 张（共 {} 张，其余 {} 张暂无向量）",
                cand.len(),
                skipped_unencoded
            ));
        } else {
            on_progress("similar", 0, cand.len(), "正在按向量聚类…");
            // 维度不一致（换档残留）按最短维处理并在日志里报数
            let dims: HashSet<usize> = cand.iter().map(|(_, v)| v.len()).collect();
            if dims.len() > 1 {
                crate::logger::log_warn(&format!(
                    "[dedupe] 向量维度不一致 {:?}，按各自维度比较（通常意味着换档后未重建）",
                    dims
                ));
            }
            let clusters = cluster_by_similarity(&cand, threshold, |done, total| {
                on_progress("similar", done, total, &format!("已比较 {done}/{total} 张"));
            });
            if cancel("similar") {
                return Err("已停止".into());
            }
            for members in clusters {
                if members.len() < 2 {
                    continue;
                }
                // 组内最低相似度（相对保留那张）——最保守的展示口径
                let mut items: Vec<DupItem> = members.iter().map(|i| to_item(&entries[*i])).collect();
                rank_items(&mut items);
                let keep = items[0].path.clone();
                let keep_vec = members
                    .iter()
                    .find(|i| entries[**i].path == keep)
                    .and_then(|i| cand.iter().find(|(ci, _)| ci == i).map(|(_, v)| *v));
                let min_sim = match keep_vec {
                    Some(kv) => members
                        .iter()
                        .filter_map(|i| {
                            let e = &entries[*i];
                            if e.path == keep {
                                return None;
                            }
                            cand.iter()
                                .find(|(ci, _)| ci == i)
                                .map(|(_, v)| { cosine(kv, v) })
                        })
                        .fold(1.0f32, |acc, s| acc.min(s)),
                    None => threshold,
                };
                let reclaimable = items.iter().skip(1).map(|i| i.size_bytes).sum();
                groups.push(DupGroup {
                    key: format!("similar:{keep}"),
                    kind: "similar".into(),
                    similarity: (min_sim * 1000.0).round() / 1000.0,
                    keep_path: keep,
                    reclaimable_bytes: reclaimable,
                    items,
                });
                similar_groups += 1;
            }
            crate::logger::log_info(&format!(
                "[dedupe] 相似分组完成 {similar_groups} 组 · 候选 {similar_candidates} 张 · 阈值 {threshold:.2}"
            ));
        }
    }

    // ---- 4. 排序与汇总 ----
    // 先按类型（exact 在前，可直接删）、再按可释放体积降序
    groups.sort_by(|a, b| {
        b.reclaimable_bytes
            .cmp(&a.reclaimable_bytes)
            .then_with(|| a.kind.cmp(&b.kind))
    });
    let exact_n = groups.iter().filter(|g| g.kind == "exact").count();
    let elapsed_ms = t0.elapsed().as_millis() as u64;
    on_progress("done", 1, 1, "扫描完成");
    crate::logger::log_info(&format!(
        "[dedupe] 完成 | 照片 {total_photos} 张 · 组 {} 个（精确 {exact_n} / 相似 {similar_groups}）· 跳过无向量 {skipped_unencoded} · {elapsed_ms}ms",
        groups.len()
    ));

    Ok(DedupeReport {
        groups,
        total_photos,
        similar_candidates,
        skipped_unencoded,
        exact_groups: exact_n,
        similar_groups,
        threshold,
        embedding_model,
        similar_skipped_reason,
        elapsed_ms,
        heic: crate::heic::support(),
        hashed_files,
    })
}

fn build_entry(path: &Path, album_id: i64) -> Option<Entry> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() || md.len() == 0 {
        return None;
    }
    let modified_secs = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (width, height) = match crate::imgio::probe_dimensions(path) {
        Some(d) => (Some(d.0), Some(d.1)),
        None => (None, None),
    };
    Some(Entry {
        path: path.to_string_lossy().into_owned(),
        key: path.to_string_lossy().to_lowercase(),
        album_id,
        size_bytes: md.len(),
        mtime_secs: modified_secs,
        width,
        height,
    })
}

fn to_item(e: &Entry) -> DupItem {
    DupItem {
        path: e.path.clone(),
        album_id: e.album_id,
        file_name: Path::new(&e.path)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size_bytes: e.size_bytes,
        width: e.width,
        height: e.height,
        modified_secs: e.mtime_secs,
    }
}

// ===========================================================================
// 精确重复
// ===========================================================================

/// 内容哈希 → 分组
///
/// 两段式：**先按文件大小分组**，只有「同尺寸出现 ≥2 次」的候选才真读文件算
/// SHA-256。绝大多数图库里尺寸唯一的文件占绝大多数，这一步把全库整读降到
/// 极小的候选集上（返回实际哈希的文件数，供报告与日志核对）。
fn exact_groups(
    entries: &[Entry],
    cancel: &impl Fn(&str) -> bool,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<(Vec<DupGroup>, usize), String> {
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        by_size.entry(e.size_bytes).or_default().push(i);
    }
    let candidates: Vec<(u64, Vec<usize>)> = by_size
        .into_iter()
        .filter(|(_, v)| v.len() >= 2)
        .collect();
    let total = candidates.iter().map(|(_, v)| v.len()).sum::<usize>();
    on_progress(0, total);

    let mut by_content: HashMap<String, Vec<usize>> = HashMap::new();
    let mut hashed = 0usize;
    for (size, idxs) in candidates {
        if cancel("hash") {
            return Err("已停止".into());
        }
        for idx in idxs {
            let e = &entries[idx];
            match sha256_file(Path::new(&e.path), size) {
                Ok(h) => {
                    by_content.entry(h).or_default().push(idx);
                    hashed += 1;
                }
                Err(err) => {
                    // 单个文件读失败不影响整批：记录后跳过（该照片不参与精确分组）
                    crate::logger::log_warn(&format!("[dedupe] 内容哈希失败 {} : {err}", e.path));
                }
            }
            if hashed % 50 == 0 {
                on_progress(hashed.min(total), total);
            }
        }
    }
    on_progress(total, total);

    let mut out: Vec<DupGroup> = Vec::new();
    for (hash, mut members) in by_content {
        if members.len() < 2 {
            continue;
        }
        members.sort_unstable();
        let mut items: Vec<DupItem> = members.iter().map(|i| to_item(&entries[*i])).collect();
        rank_items(&mut items);
        let keep = items[0].path.clone();
        let reclaimable = items.iter().skip(1).map(|i| i.size_bytes).sum();
        out.push(DupGroup {
            key: format!("exact:{}", &hash[..16.min(hash.len())]),
            kind: "exact".into(),
            similarity: 1.0,
            keep_path: keep,
            reclaimable_bytes: reclaimable,
            items,
        });
    }
    Ok((out, hashed))
}

/// 流式 SHA-256（1 MiB 分块，不整文件进内存）
fn sha256_file(path: &Path, expected_len: u64) -> Result<String, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, f);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut read_total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        read_total += n as u64;
    }
    if read_total != expected_len {
        // 边比边改（文件正在被写入）：宁可报错也不要给出假分组
        return Err(format!("文件长度在扫描期间变化（{expected_len} → {read_total}）"));
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// ===========================================================================
// 相似分组
// ===========================================================================

/// 单位向量的余弦（长度不一致时按较短者截断，容忍换档残留）
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut s = 0.0f32;
    for i in 0..n {
        s += a[i] * b[i];
    }
    s
}

/// 按阈值把候选聚成组（连通分量），返回每个组的 entry 下标
fn cluster_by_similarity(
    cand: &[(usize, &Vec<f32>)],
    threshold: f32,
    mut on_progress: impl FnMut(usize, usize),
) -> Vec<Vec<usize>> {
    let n = cand.len();
    if n < 2 {
        return Vec::new();
    }
    // 1) 选方差最大的维度做排序键（区分度越高，窗口内的候选越少）
    let dim = max_variance_dim(cand);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&x, &y| {
        cand[x].1[dim]
            .partial_cmp(&cand[y].1[dim])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.cmp(&y))
    });
    // 2) 由 cos ≥ t 推出的一维窗口上界
    let window = (2.0f32 * (1.0 - threshold)).max(0.0).sqrt() + 1e-6;

    // 3) 只在窗口内两两算点积（rayon 并行；结果按 (i,j) 收集，与线程调度无关）
    use rayon::prelude::*;
    let pairs: Vec<(usize, usize, f32)> = (0..n)
        .into_par_iter()
        .map(|i| {
            // 并行体内只能读进程级原子标记（闭包不能跨线程共享）
            if i % 512 == 0 && DEDUPE_CANCEL.load(Ordering::SeqCst) {
                return Vec::new();
            }
            let (ei, vi) = cand[order[i]];
            let xi = vi[dim];
            let mut local: Vec<(usize, usize, f32)> = Vec::new();
            for j in (i + 1)..n {
                let (ej, vj) = cand[order[j]];
                let dx = vj[dim] - xi;
                // 升序：一旦超出窗口，后面更远，直接跳出（无损，见模块头注释）
                if dx > window {
                    break;
                }
                let s = cosine(vi, vj);
                if s >= threshold {
                    local.push((ei, ej, s));
                }
            }
            local
        })
        .flatten()
        .collect();
    on_progress(n, n);

    // 4) 并查集合并
    let mut uf = UnionFind::new(n);
    let mut idx_of: HashMap<usize, usize> = HashMap::new();
    for (slot, (entry_idx, _)) in cand.iter().enumerate() {
        idx_of.insert(*entry_idx, slot);
    }
    let mut linked = 0usize;
    for (a, b, _) in &pairs {
        if let (Some(&sa), Some(&sb)) = (idx_of.get(a), idx_of.get(b)) {
            if uf.union(sa, sb) {
                linked += 1;
            }
        }
    }
    crate::logger::log_info(&format!(
        "[dedupe] 相似判定：候选 {n} 张 · 真配对 {linked} 条（阈值 {threshold:.2}、窗口 {window:.3}）"
    ));

    let mut buckets: HashMap<usize, Vec<usize>> = HashMap::new();
    for slot in 0..n {
        let root = uf.find(slot);
        buckets.entry(root).or_default().push(cand[slot].0);
    }
    buckets.into_values().filter(|v| v.len() >= 2).collect()
}

/// 方差最大的维度（区分度最高，剪枝窗口里塞的候选最少）
fn max_variance_dim(cand: &[(usize, &Vec<f32>)]) -> usize {
    let n = cand.len();
    let dim = cand.iter().map(|(_, v)| v.len()).min().unwrap_or(0);
    let mut best = 0usize;
    let mut best_var = -1.0f64;
    for d in 0..dim {
        let mut sum = 0.0f64;
        let mut sq = 0.0f64;
        for (_, v) in cand {
            let x = v[d] as f64;
            sum += x;
            sq += x * x;
        }
        let mean = sum / n as f64;
        let var = sq / n as f64 - mean * mean;
        if var > best_var {
            best_var = var;
            best = d;
        }
    }
    best
}

/// 并查集（按秩合并 + 路径压缩）
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self { parent: (0..n).collect(), rank: vec![0; n] }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    /// 返回是否真的发生了合并
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (mut ra, mut rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        if self.rank[ra] < self.rank[rb] {
            std::mem::swap(&mut ra, &mut rb);
        }
        self.parent[rb] = ra;
        if self.rank[ra] == self.rank[rb] {
            self.rank[ra] += 1;
        }
        true
    }
}

/// "原本"排序：分辨率高 → 体积大 → 修改时间早 → 路径序
///
/// 为什么不是"体积最大就留"：iPhone 的 HEIC 原图与导出 JPEG 可能体积相近，
/// 但分辨率差一档（4032×3024 vs 1920×1440），凭体积选会留错。
/// 也不按"最新"选：多份副本里更早的那份更可能是原始导入。
fn rank_items(items: &mut [DupItem]) {
    items.sort_by(|a, b| {
        let pa = a.width.unwrap_or(0) as u64 * a.height.unwrap_or(0) as u64;
        let pb = b.width.unwrap_or(0) as u64 * b.height.unwrap_or(0) as u64;
        pb.cmp(&pa)
            .then_with(|| b.size_bytes.cmp(&a.size_bytes))
            .then_with(|| a.modified_secs.cmp(&b.modified_secs))
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// 供命令层在异步侧起任务时复用
pub type ProgressFn = Arc<dyn Fn(&str, usize, usize, &str) + Send + Sync>;

// ===========================================================================
// 命令层（薄壳；lib.rs 只注册）
// ===========================================================================
pub mod commands {
    use super::*;
    use crate::{logger, require_user, AppState, SessionState};
    use tauri::Emitter;

    /// 全库重复照片扫描（精确内容哈希 + CLIP 向量相似）
    ///
    /// - `threshold`：相似度阈值，前端滑杆 0.80–1.00，后端再 clamp 一次
    /// - 重活（遍历 + 哈希 + 聚类）在阻塞线程执行，不冻结 UI
    /// - 进度通过 `dedupe-progress` 事件上报
    #[tauri::command]
    pub async fn dedupe_scan(
        threshold: f64,
        include_exact: bool,
        include_similar: bool,
        app: tauri::AppHandle,
        session: tauri::State<'_, SessionState>,
    ) -> Result<DedupeReport, String> {
        let _t = log_call!(
            "dedupe_scan",
            &format!("threshold={threshold} exact={include_exact} similar={include_similar}")
        );
        let user_id = require_user(&session)?;
        if !include_exact && !include_similar {
            return Err("请至少勾选一种重复判据（完全重复 / 画面相似）".into());
        }
        if is_running() {
            return Err("已有重复扫描在进行中，请等它结束再试".into());
        }
        let threshold = (threshold as f32).clamp(0.70, 1.0);

        // 相册与向量都在锁内一次性取出（阻塞线程里不再持锁做 IO）
        let (albums, embeddings, model) = {
            use tauri::Manager;
            let state = app.state::<AppState>();
            let db = state.0.lock().map_err(|e| format!("{e}"))?;
            let albums: Vec<(i64, String)> = db
                .get_albums(user_id)
                .map_err(|e| format!("读取相册失败: {e}"))?
                .into_iter()
                .map(|a| (a.id, a.path))
                .collect();
            let model = if include_similar {
                db.dominant_embedding_model(user_id).map_err(|e| e.to_string())?
            } else {
                None
            };
            let embeddings = match (&model, include_similar) {
                (Some((m, _)), true) => db.load_all_embeddings(user_id, m).unwrap_or_default(),
                _ => Vec::new(),
            };
            (albums, embeddings, model.map(|(m, n)| format!("{m}（{n} 条）")))
        };
        if albums.is_empty() {
            return Err("还没有任何相册，请先导入相册再扫描重复".into());
        }
        logger::log_info(&format!(
            "[dedupe] 开始 | 相册 {} 个 · 向量模型 {:?} · 阈值 {threshold:.2} · 精确 {include_exact} · 相似 {include_similar}",
            albums.len(),
            model
        ));

        let app_for_task = app.clone();
        let report = tauri::async_runtime::spawn_blocking(move || {
            scan_library(
                &albums,
                embeddings,
                model,
                threshold,
                include_exact,
                include_similar,
                |stage, done, total, message| {
                    let _ = app_for_task.emit(
                        "dedupe-progress",
                        DedupeProgress {
                            stage: stage.to_string(),
                            done,
                            total,
                            message: message.to_string(),
                        },
                    );
                },
            )
        })
        .await
        .map_err(|e| format!("重复扫描任务失败: {e}"))??;

        logger::log_call_end_with(
            "dedupe_scan",
            _t,
            &format!(
                "OK | groups={} exact={} similar={} photos={} skipped={} ms={}",
                report.groups.len(),
                report.exact_groups,
                report.similar_groups,
                report.total_photos,
                report.skipped_unencoded,
                report.elapsed_ms
            ),
        );
        Ok(report)
    }

    /// 停止当前重复扫描（下一个检查点生效）
    #[tauri::command]
    pub fn dedupe_cancel() -> Result<(), String> {
        request_cancel();
        Ok(())
    }

    /// HEIC/HEIF 解码能力探针（前端据此提示「安装 HEIF 图像扩展」）
    #[tauri::command]
    pub fn heic_support() -> crate::heic::HeicSupport {
        crate::heic::support()
    }
}

// ===========================================================================
// 单元测试
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "pms-dedupe-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    fn entry_of(p: &Path, size: u64) -> Entry {
        Entry {
            path: p.to_string_lossy().into_owned(),
            key: p.to_string_lossy().to_lowercase(),
            album_id: 1,
            size_bytes: size,
            mtime_secs: 0,
            width: Some(100),
            height: Some(100),
        }
    }

    /// 同内容不同名 → 同一组；同大小不同内容 → 不同组
    #[test]
    fn exact_groups_by_content_hash() {
        let dir = tmp_dir("exact");
        let a = write_file(&dir, "a.jpg", b"same-bytes-here");
        let b = write_file(&dir, "b.jpg", b"same-bytes-here");
        // 与上面等长但内容不同
        let c = write_file(&dir, "c.jpg", b"same-bytes-ther");
        // 长度唯一 → 连哈希都不该算
        let d = write_file(&dir, "d.jpg", b"unique-length-1234567890");
        let entries = vec![
            entry_of(&a, 15),
            entry_of(&b, 15),
            entry_of(&c, 15),
            entry_of(&d, 23),
        ];
        let (groups, hashed) = exact_groups(&entries, &|_| false, |_, _| {}).unwrap();
        assert_eq!(groups.len(), 1, "只有 a/b 是一组");
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(hashed, 3, "同尺寸的 3 个文件算了哈希，长度唯一的 d 不算");
        assert_eq!(groups[0].reclaimable_bytes, 15);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 阈值剪枝必须与暴力两两比较结果**完全一致**（无损性回归）
    #[test]
    fn pruning_matches_bruteforce() {
        // 构造 3 簇：每簇 3 个相近向量 + 若干孤立向量
        let mut vecs: Vec<Vec<f32>> = Vec::new();
        let bases: [[f32; 4]; 3] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        for (bi, base) in bases.iter().enumerate() {
            for k in 0..3 {
                let mut v = base.to_vec();
                v[3] = 0.02 * (k as f32 + 1.0) + bi as f32 * 0.01;
                v[bi] += 0.01 * k as f32;
                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                vecs.push(v.iter().map(|x| x / norm).collect());
            }
        }
        for k in 0..5 {
            let mut v = vec![0.1 * k as f32, 0.3, 0.5, 0.8];
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            for x in v.iter_mut() {
                *x /= norm;
            }
            vecs.push(v);
        }
        let cand: Vec<(usize, &Vec<f32>)> = vecs.iter().enumerate().collect();

        for th in [0.80f32, 0.90, 0.95, 0.999] {
            let mut fast = groups_of(cluster_by_similarity(&cand, th, |_, _| {}));
            let mut slow = groups_of(bruteforce_clusters(&cand, th));
            assert_eq!(fast, slow, "阈值 {th} 下剪枝结果应与暴力一致");
        }
    }

    fn groups_of(mut g: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
        for v in g.iter_mut() {
            v.sort_unstable();
        }
        g.sort();
        g
    }

    fn bruteforce_clusters(cand: &[(usize, &Vec<f32>)], th: f32) -> Vec<Vec<usize>> {
        let n = cand.len();
        let mut uf = UnionFind::new(n);
        for i in 0..n {
            for j in (i + 1)..n {
                if cosine(cand[i].1, cand[j].1) >= th {
                    uf.union(i, j);
                }
            }
        }
        let mut buckets: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            let r = uf.find(i);
            buckets.entry(r).or_default().push(cand[i].0);
        }
        buckets.into_values().filter(|v| v.len() >= 2).collect()
    }

    /// 保留策略：分辨率高优先，其次体积，再次更早的修改时间
    #[test]
    fn keeper_prefers_resolution_then_size_then_age() {
        let mk = |path: &str, w: u32, h: u32, size: u64, mtime: i64| DupItem {
            path: path.into(),
            album_id: 1,
            file_name: path.into(),
            size_bytes: size,
            width: Some(w),
            height: Some(h),
            modified_secs: mtime,
        };
        let mut items = vec![
            mk("a_small.jpg", 1920, 1440, 900_000, 10),
            mk("b_heic.heic", 4032, 3024, 2_400_000, 20),
            mk("c_big.jpg", 4032, 3024, 6_000_000, 30),
        ];
        rank_items(&mut items);
        assert_eq!(items[0].path, "c_big.jpg", "同分辨率下体积大的优先");
        let mut items2 = vec![mk("x.jpg", 1920, 1440, 5_000_000, 1), mk("y.heic", 4032, 3024, 2_400_000, 9)];
        rank_items(&mut items2);
        assert_eq!(items2[0].path, "y.heic", "分辨率优先于体积");
    }

    /// 维度不一致时按较短维比较，不 panic
    #[test]
    fn cosine_tolerates_dim_mismatch() {
        assert!((cosine(&[1.0, 0.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!((cosine(&[0.0, 1.0], &[1.0, 0.0])).abs() < 1e-6);
    }

    fn normalize(v: [f32; 4]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    /// 进程级单飞锁在测试之间也会互相干扰（测试默认并行跑），串行化这两个用例
    static E2E_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn e2e_gate() -> std::sync::MutexGuard<'static, ()> {
        E2E_GATE.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 全流程：两个相册 → 精确组 + 相似组 → 默认保留规则 → 报告数字自洽
    #[test]
    fn scan_library_end_to_end() {
        let _gate = e2e_gate();
        let root = tmp_dir("e2e");
        let d1 = root.join("a1");
        let d2 = root.join("a2");
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::create_dir_all(&d2).unwrap();
        // a/b 字节完全相同；c 与 a/b 等长但内容不同（考验体积预筛后的哈希分支）
        let a = write_file(&d1, "IMG_1.jpg", b"identical-bytes-0123");
        let _b = write_file(&d1, "IMG_1_copy.jpg", b"identical-bytes-0123");
        let c = write_file(&d2, "IMG_2.jpg", b"different-bytes-0123");
        let d = write_file(&d2, "IMG_3.jpg", b"another-unique-length-bytes");

        // c 与 d 给一对高相似向量：应被聚成同一相似组
        let emb = vec![
            ("h_c".to_string(), Some(2i64), c.to_string_lossy().into_owned(), normalize([1.0, 0.0, 0.0, 0.0])),
            ("h_d".to_string(), Some(2i64), d.to_string_lossy().into_owned(), normalize([0.98, 0.13, 0.0, 0.0])),
        ];
        let albums = vec![
            (1i64, d1.to_string_lossy().into_owned()),
            (2i64, d2.to_string_lossy().into_owned()),
        ];
        let report = scan_library(&albums, emb, Some("test-model".into()), 0.9, true, true, |_, _, _, _| {})
            .expect("扫描应成功");

        assert_eq!(report.total_photos, 4, "两个相册共 4 张");
        assert_eq!(report.exact_groups, 1, "a/b 是一组精确重复");
        assert_eq!(report.similar_groups, 1, "c/d 是一组相似");
        assert_eq!(report.hashed_files, 3, "同尺寸的 3 个算哈希，长度唯一的 d 不算");
        assert_eq!(report.similar_candidates, 2, "a/b 已归属精确组，不再参与相似判定");
        assert_eq!(
            report.skipped_unencoded, 0,
            "c/d 都有向量；a/b 归属精确组已先被排除，不计入‘未入库’"
        );

        let exact = report.groups.iter().find(|g| g.kind == "exact").unwrap();
        assert_eq!(exact.items.len(), 2);
        assert_eq!(exact.reclaimable_bytes, 20);
        assert!(exact.items.iter().any(|i| i.path == a.to_string_lossy()));

        let sim = report.groups.iter().find(|g| g.kind == "similar").unwrap();
        assert_eq!(sim.items.len(), 2);
        assert!(sim.similarity >= 0.9, "相似度应不低于阈值: {}", sim.similarity);

        // 同一路径不得出现在两个组里（否则一勾选就会重复删除/漏删）
        let mut seen = HashSet::new();
        for g in &report.groups {
            for it in &g.items {
                assert!(seen.insert(it.path.clone()), "路径重复出现在多个组: {}", it.path);
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 关掉两类判据后应拒绕；单飞锁在扫完后必须释放（否则第二次永远被拒）
    #[test]
    fn run_lock_is_released() {
        let _gate = e2e_gate();
        let root = tmp_dir("lock");
        let d1 = root.join("a1");
        std::fs::create_dir_all(&d1).unwrap();
        write_file(&d1, "x.jpg", b"abc");
        let albums = vec![(1i64, d1.to_string_lossy().into_owned())];
        for _ in 0..2 {
            let r = scan_library(&albums, Vec::new(), None, 0.9, true, false, |_, _, _, _| {});
            assert!(r.is_ok(), "连续两次扫描都应成功（锁必须释放）");
        }
        assert!(!is_running());
        let _ = std::fs::remove_dir_all(&root);
    }
}
