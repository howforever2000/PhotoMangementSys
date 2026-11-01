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
//! ## 性能：三段均并行，热循环走 SIMD（实测数据）
//!
//! 全库扫描分三段，每段的瓶颈不同（实测：2.5 万张真实图库 / 14 线程 / release）：
//!
//! | 阶段 | 原实现 | 现在 | 实测 |
//! |------|--------|------|------|
//! | 遍历 + stat | 逐相册串行 + **每张多开一次文件探尺寸** | 相册级 rayon 并行，尺寸延到入组时再探 | 6970 张：19.15s → 0.13s（含去掉无谓探尺寸） |
//! | 精确重复 SHA-256 | 逐文件串行 | rayon 并行 | 1154 个文件：25.39s → 2.51s（10.1×） |
//! | 画面相似（点积） | rayon 并行（已是） | + 摊平矩阵 + AVX2/FMA | 24905 张：单线程 76.1s → 14 线程 12.1s → AVX2 8.0s |
//!
//! ### 关于「一维窗口剪枝」——保留但它**不是**性能支柱
//!
//! 对单位向量有严格上界：
//!
//! ```text
//! cos(a,b) ≥ t  ⇒  ‖a-b‖² = 2-2cos(a,b) ≤ 2-2t
//!               ⇒  |a_d - b_d| ≤ ‖a-b‖ ≤ √(2-2t)   （任意维度 d）
//! ```
//!
//! 于是可以选方差最大的那一维排序、只比较窗口内的邻居（数学上无损）。
//! **但实测（24905 张真实 CLIP 向量，t=0.9）只剪掉 0.0%**：512 维单位向量单个
//! 坐标的分布极窄（≈1/√D ≈ 0.044），而窗口有 0.447，几乎全部点对都落在窗内。
//! 它只在「某一维特别有区分度」的数据上有用（合成数据实测有效），所以留着（成本
//! 很低），但**真正的提速来自并行与 SIMD，而不是剪枝**。日志会打出实际比较对数，
//! 以便以后用真实数据核对这个结论是否仍成立。
//!
//! 要再上一档就得上分块（tiling）：当前瓶颈已经是**内存带宽**（每对要扫 4KB，
//! 3.1 亿对 = 620GB），按 256×256 块做矩阵乘法式分块能把这部分降一个量级。
//!
//! ## 分组语义：完全链接，不是「串成一串」
//!
//! 一开始用的是并查集连通分量（传递闭包）。用真实图库实测后被推翻了：
//! 24905 张、阈值 0.90 下有 29.3 万条 ≥0.90 的边，连通分量把 9328 张串成一组 ——
//! 「留一张删 9327 张」在清理场景里毫无意义，而且链式传递会把明显不同的照片拉进来。
//!
//! 现在用**完全链接（complete-linkage）**：组内**任意两张**都必须 ≥ 阈值。
//! 同一库实测：0.90 → 5029 组/最大 148 张；0.95 → 4497 组/最大 41 张；
//! 0.99 → 3097 组/最大 10 张。
//!
//! 组内展示的 `similarity` 取**保留那张与其余每一张的最低余弦**（最保守的口径），
//! 让用户看到的是组里最不像的那一对有多像。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use sha2::{Digest, Sha256};

/// 扫描前统计到的单张照片元信息
#[derive(Debug, Clone)]
struct Entry {
    path: String,
    /// 小写路径（Windows 大小写不敏感；用于与向量表按原图路径关联）
    key: String,
    /// **真实文件身份** = `canonicalize(相册根)` + 相对路径（小写、分隔符统一）
    ///
    /// 与 `key` 的差别就是「同一张照片被两个嵌套相册各扫一遍」这一类问题的根：
    /// `key` 是盘上那个字符串本身，同一个文件经嵌套相册 / junction / 8.3 短名 /
    /// 斜杠方向 / 盘符大小写不同时会得到不同字符串，于是被当成两张照片。
    /// 这里改走真实路径（相册根只 canonicalize 一次，代价与相册数同阶）。
    ident: String,
    album_id: i64,
    size_bytes: u64,
    mtime_secs: i64,
}

/// 组内一张照片（前端展示 + 勾选单位）
#[derive(Debug, Clone, Serialize)]
pub struct DupItem {
    pub path: String,
    /// 文件所在文件夹的完整路径（前端在缩略图下方显示「地址」）
    pub dir: String,
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

/// 分阶段耗时（毫秒）
#[derive(Debug, Clone, Default, Serialize)]
pub struct StageMs {
    /// 遍历相册目录 + stat（收集候选）
    pub collect: u64,
    /// 精确重复：体积预筛后的 SHA-256
    pub hash: u64,
    /// 画面相似：向量聚类
    pub similar: u64,
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
    /// 分阶段耗时（FEAT-143 性能基线：定位到底慢在遍历 / 哈希 / 聚类哪一段）
    pub stage_ms: StageMs,
    /// HEIC 解码能力（缺 HEIF 扩展时前端提示装扩展）
    pub heic: crate::heic::HeicSupport,
    /// 内容哈希实际计算的文件数（体积预筛后剩下的碰撞候选）
    pub hashed_files: usize,
    /// 因为「指向同一个真实文件」而被合并掉的条目数（0 表示没有同址重复）
    pub collapsed_same_file: usize,
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
    on_progress: &(impl Fn(&str, usize, usize, &str) + Sync),
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

    // ---- 1. 收集全库照片（并行：遍历 + stat 是纯 IO，逐相册串行时 IO 队列填不满）----
    on_progress("collect", 0, albums.len(), "正在遍历相册目录…");
    let t_collect = std::time::Instant::now();
    let entries: Vec<Entry> = collect_entries(albums, &|done, total, found| {
        on_progress("collect", done, total, &format!("已找到 {found} 张照片"));
    });
    if cancel("collect") {
        return Err("已停止".into());
    }
    let collect_ms = t_collect.elapsed().as_millis() as u64;
    let total_photos = entries.len();
    crate::logger::log_info(&format!(
        "[dedupe] 全库收集完成 {total_photos} 张 · {} 个相册 · {collect_ms}ms",
        albums.len()
    ));

    // ---- 2. 完全重复：体积预筛 + 内容哈希 ----
    let mut groups: Vec<DupGroup> = Vec::new();
    let mut in_exact: HashSet<String> = HashSet::new();
    let mut hashed_files = 0usize;
    let mut hash_ms = 0u64;
    /// 同一真实文件被合并掉的条目数（全流程累计）
    let mut collapsed_same_file = 0usize;
    if include_exact {
        on_progress("hash", 0, total_photos, "正在比对文件内容…");
        let t_hash = std::time::Instant::now();
        let (exact, hashed, collapsed) = exact_groups(&entries, &|done, total| {
            on_progress("hash", done, total, &format!("已比对 {done}/{total} 张"));
        })?;
        hash_ms = t_hash.elapsed().as_millis() as u64;
        hashed_files = hashed;
        collapsed_same_file += collapsed;
        for g in &exact {
            for it in &g.items {
                in_exact.insert(it.path.to_lowercase());
            }
        }
        crate::logger::log_info(&format!(
            "[dedupe] 精确重复 {} 组 · 实际计算内容哈希 {hashed_files} 个文件 · {hash_ms}ms",
            exact.len()
        ));
        groups.extend(exact);
    }
    if cancel("hash") {
        return Err("已停止".into());
    }

    // ---- 3. 画面相似：向量余弦 ----
    let mut similar_groups = 0usize;
    let mut similar_candidates = 0usize;
    let mut similar_skipped_reason: Option<String> = None;
    let mut skipped_unencoded = 0usize;
    let mut similar_ms = 0u64;
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
            let t_sim = std::time::Instant::now();
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
                // 同一真实文件只留一张（否则用户会看到同一张照片并列成“重复”）
                let merged = collapse_same_target(&mut items);
                if merged > 0 {
                    collapsed_same_file += merged;
                    if items.len() < 2 {
                        continue;
                    }
                }
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
            similar_ms = t_sim.elapsed().as_millis() as u64;
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
        "[dedupe] 完成 | 照片 {total_photos} 张 · 组 {} 个（精确 {exact_n} / 相似 {similar_groups}）· 跳过无向量 {skipped_unencoded} · 同址合并 {collapsed_same_file} · 总 {elapsed_ms}ms（遍历 {collect_ms} / 哈希 {hash_ms} / 相似 {similar_ms}）",
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
        stage_ms: StageMs { collect: collect_ms, hash: hash_ms, similar: similar_ms },
        heic: crate::heic::support(),
        hashed_files,
        collapsed_same_file,
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
    let raw = path.to_string_lossy().into_owned();
    Some(Entry {
        key: raw.to_lowercase(),
        // 占位：真正的 ident 由 collect_entries 按「相册真实根 + 相对路径」重算
        ident: normalize_path_key(&raw),
        path: raw,
        album_id,
        size_bytes: md.len(),
        mtime_secs: modified_secs,
    })
}

/// 路径字符串归一：分隔符统一成 `\`、去尾分隔符、小写
///
/// 只归一“写法”，不碰文件系统 —— 真正的同一文件判定靠 canonicalize。
fn normalize_path_key(raw: &str) -> String {
    raw.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// 相册根的「真实路径」键
///
/// canonicalize 会把 junction / 符号链 / 8.3 短名 / 盘符大小写 / `\\?\` 前缀
/// 统统解成同一种写法；每个相册只调一次，代价与相册数同阶（不是与照片数）。
/// 解不开（权限 / 网络盘掉线）就退回字符串归一 —— 宁可少合并，不能漏照片。
fn canonical_dir_key(dir: &str) -> String {
    match std::fs::canonicalize(dir) {
        Ok(p) => normalize_path_key(&p.to_string_lossy()),
        Err(_) => normalize_path_key(dir),
    }
}

/// 文件身份 = 真实相册根 + 相对路径
fn file_ident(root: &Path, root_key: &str, file: &Path) -> String {
    match file.strip_prefix(root) {
        Ok(rel) => {
            let rel = normalize_path_key(&rel.to_string_lossy());
            if rel.is_empty() {
                root_key.to_string()
            } else {
                format!("{root_key}\\{rel}")
            }
        }
        // 走目录时理论上一定拿得到相对路径；真拿不到就退回全路径归一键
        Err(_) => normalize_path_key(&file.to_string_lossy()),
    }
}

/// 单个文件的真实目标键（组内二次合并用）
fn real_target_key(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(p) => normalize_path_key(&p.to_string_lossy()),
        Err(_) => normalize_path_key(path),
    }
}

/// 组内按「真实文件」再合一次：指向同一个文件的条目只保留排在前面的那个
///
/// 为什么收集阶段已经去过重还要再合一次：收集阶段只能保证「同一相册根下的写法
/// 一致」，而一条记录可能经由别的相册根进来（junction / 网络盘影射 / 短名）。
/// 这一步只对**已经入组**的少量照片调 canonicalize，成本可忽略。
fn collapse_same_target(items: &mut Vec<DupItem>) -> usize {
    let before = items.len();
    let mut seen: HashSet<String> = HashSet::with_capacity(before);
    items.retain(|it| seen.insert(real_target_key(&it.path)));
    before - items.len()
}

/// 并行收集全库照片（FEAT-143 性能优化）
///
/// 原实现是「逐相册 walkdir → 逐张 stat → 逐张读文件头探尺寸」全串行：
/// 2.5 万张实测这一步是主要瓶颈，且其中**探尺寸那一次 open + 4KB 读是白花花的**——
/// 全库只有极少一部分照片最终会进重复组，尺寸只为「保留哪张」的排序服务。
/// 现在：
/// 1. `albums.par_iter()` 并行走目录（rayon 线程数 = 核数，不会把磁盘打爆）；
/// 2. 展平后 `par_iter()` 并行 stat；
/// 3. 串行合并去重；同一文件出现在嵌套相册时归属取**最深**的那个（最具体）。
///
/// 去重口径是[`Entry::ident`]（真实路径身份），不是路径字符串：
/// 「相册 A 包含相册 B、两个都被扫」这类嵌套场景下同一个文件只会留一条。
fn collect_entries(
    albums: &[(i64, String)],
    on_progress: &(impl Fn(usize, usize, usize) + Sync),
) -> Vec<Entry> {
    use rayon::prelude::*;

    /// 一个相册的遍历结果（带上真实根键，供并行 stat 时算文件身份）
    struct AlbumWalk {
        album_id: i64,
        /// 相册路径字符串长度（嵌套时按“更长 = 更具体”判归属）
        dir_len: usize,
        root: std::path::PathBuf,
        root_key: String,
        paths: Vec<std::path::PathBuf>,
    }

    // 1. 并行遍历：rayon 的 collect 保序，结果与相册入参顺序一致
    let per_album: Vec<AlbumWalk> = albums
        .par_iter()
        .map(|(album_id, dir)| {
            let root = std::path::PathBuf::from(dir);
            let root_key = canonical_dir_key(dir);
            if DEDUPE_CANCEL.load(Ordering::Relaxed) {
                return AlbumWalk { album_id: *album_id, dir_len: dir.len(), root, root_key, paths: Vec::new() };
            }
            match crate::vision::walk_image_paths(dir) {
                Ok(paths) => AlbumWalk { album_id: *album_id, dir_len: dir.len(), root, root_key, paths },
                Err(e) => {
                    crate::logger::log_warn(&format!("[dedupe] 相册 {album_id} 遍历失败，跳过: {e}"));
                    AlbumWalk { album_id: *album_id, dir_len: dir.len(), root, root_key, paths: Vec::new() }
                }
            }
        })
        .collect();

    // 2. 并行 stat（每张一次 metadata；不在这里读文件头）
    let flat: Vec<(i64, usize, &std::path::Path, &std::path::Path, &String)> = per_album
        .iter()
        .flat_map(|w| {
            w.paths
                .iter()
                .map(move |p| (w.album_id, w.dir_len, w.root.as_path(), p.as_path(), &w.root_key))
        })
        .collect();
    let built: Vec<(i64, usize, Entry)> = flat
        .par_iter()
        .filter_map(|(id, dl, root, p, root_key)| {
            build_entry(p, *id).map(|mut e| {
                // 身份：真实相册根 + 相对路径（展示仍用盘上的原字符串）
                e.ident = file_ident(root, root_key, p);
                (*id, *dl, e)
            })
        })
        .collect();

    // 3. 串行合并（保序 + 最深相册归属）
    let mut entries: Vec<Entry> = Vec::with_capacity(built.len());
    let mut seen: HashMap<String, usize> = HashMap::with_capacity(built.len());
    let mut owner_dir_len: Vec<usize> = Vec::with_capacity(built.len());
    let mut collapsed = 0usize;
    for (album_id, dir_len, mut entry) in built {
        match seen.get(&entry.ident).copied() {
            Some(idx) => {
                collapsed += 1;
                if dir_len > owner_dir_len[idx] {
                    entry.album_id = album_id;
                    entries[idx].album_id = album_id;
                    owner_dir_len[idx] = dir_len;
                }
            }
            None => {
                entry.album_id = album_id;
                seen.insert(entry.ident.clone(), entries.len());
                entries.push(entry);
                owner_dir_len.push(dir_len);
            }
        }
    }
    if collapsed > 0 {
        crate::logger::log_info(&format!(
            "[dedupe] 同址归并 {collapsed} 条（嵌套相册 / 同一文件多种写法），实际照片 {} 张",
            entries.len()
        ));
    }
    on_progress(albums.len(), albums.len(), entries.len());
    entries
}

/// Entry → 前端展示项（**尺寸在这里才读**，只对真的进了组的照片付这一次 IO）
fn to_item(e: &Entry) -> DupItem {
    let (width, height) = match crate::imgio::probe_dimensions(Path::new(&e.path)) {
        Some(d) => (Some(d.0), Some(d.1)),
        None => (None, None),
    };
    DupItem {
        path: e.path.clone(),
        dir: Path::new(&e.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        album_id: e.album_id,
        file_name: Path::new(&e.path)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size_bytes: e.size_bytes,
        width,
        height,
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
///
/// 返回值第三项：组内「指向同一真实文件」而被合并掉的条目数（正常应为 0）。
fn exact_groups(
    entries: &[Entry],
    on_progress: &(impl Fn(usize, usize) + Sync),
) -> Result<(Vec<DupGroup>, usize, usize), String> {
    use rayon::prelude::*;
    use std::sync::atomic::AtomicUsize;

    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        by_size.entry(e.size_bytes).or_default().push(i);
    }
    // 待哈希列表：(下标, 期望体积)；只含「同体积出现 ≥2 次」的候选
    let todo: Vec<(usize, u64)> = by_size
        .into_iter()
        .filter(|(_, v)| v.len() >= 2)
        .flat_map(|(size, v)| v.into_iter().map(move |i| (i, size)))
        .collect();
    let total = todo.len();
    on_progress(0, total);

    // 并行哈希：SHA-256 是 CPU 密集 + 顺序读盘，两两阻塞在 rayon 线程上
    // （rayon 线程数 = 核数，不会像手写线程池那样把磁盘打爆）
    let done = AtomicUsize::new(0);
    let hashed_rows: Vec<(String, usize)> = todo
        .par_iter()
        .filter_map(|(idx, size)| {
            if DEDUPE_CANCEL.load(Ordering::Relaxed) {
                return None;
            }
            let e = &entries[*idx];
            let r = match sha256_file(Path::new(&e.path), *size) {
                Ok(h) => Some((h, *idx)),
                Err(err) => {
                    // 单个文件读失败不影响整批：记录后跳过（该照片不参与精确分组）
                    crate::logger::log_warn(&format!("[dedupe] 内容哈希失败 {} : {err}", e.path));
                    None
                }
            };
            // 进度：每 64 张报一次（多线程下只能靠原子计数粗略节流）
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 64 == 0 || n == total {
                on_progress(n, total);
            }
            r
        })
        .collect();
    if DEDUPE_CANCEL.load(Ordering::SeqCst) {
        return Err("已停止".into());
    }
    on_progress(total, total);

    let hashed = hashed_rows.len();
    let mut by_content: HashMap<String, Vec<usize>> = HashMap::new();
    for (hash, idx) in hashed_rows {
        by_content.entry(hash).or_default().push(idx);
    }

    let mut out: Vec<DupGroup> = Vec::new();
    let mut collapsed = 0usize;
    for (hash, mut members) in by_content {
        if members.len() < 2 {
            continue;
        }
        members.sort_unstable();
        let mut items: Vec<DupItem> = members.iter().map(|i| to_item(&entries[*i])).collect();
        rank_items(&mut items);
        // 同一真实文件只留一张：嵌套相册 / junction / 短名会让同一个文件出现两条
        let merged = collapse_same_target(&mut items);
        if merged > 0 {
            collapsed += merged;
            crate::logger::log_info(&format!(
                "[dedupe] 精确组 {} 内合并了 {merged} 条同址条目",
                &hash[..16.min(hash.len())]
            ));
            if items.len() < 2 {
                continue;
            }
        }
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
    Ok((out, hashed, collapsed))
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

/// 点积快速路径：8 路累加 + 固定长度块（消掉边界检查，编译器可自动向量化）
///
/// 为什么值得单写：相似判定是全库唯一的热循环（2.5 万张 = 3.1 亿次点积 × 512 维 ≈
/// 1.6e11 次乘加）。原实现是逐元素累加的单累加器循环，存在两个问题：
/// ① 每次下标访问都带边界检查；② 单累加器形成串行依赖链，CPU 的 FMA 吞吐用不上。
/// 拆成 8 条独立累加链后，向量化与流水线都能吃满（release 构建效果最明显）。
#[inline]
fn dot_fast(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let a = &a[..n];
    let b = &b[..n];
    let mut acc = [0.0f32; 8];
    let mut ca = a.chunks_exact(8);
    let mut cb = b.chunks_exact(8);
    for (x, y) in ca.by_ref().zip(cb.by_ref()) {
        acc[0] += x[0] * y[0];
        acc[1] += x[1] * y[1];
        acc[2] += x[2] * y[2];
        acc[3] += x[3] * y[3];
        acc[4] += x[4] * y[4];
        acc[5] += x[5] * y[5];
        acc[6] += x[6] * y[6];
        acc[7] += x[7] * y[7];
    }
    let mut s = (acc[0] + acc[1]) + (acc[2] + acc[3]) + (acc[4] + acc[5]) + (acc[6] + acc[7]);
    for (x, y) in ca.remainder().iter().zip(cb.remainder().iter()) {
        s += x * y;
    }
    s
}

/// AVX2 + FMA 点积（8 路 float 一条指令喂两条累加链）
///
/// 为什么手写：`dot_fast` 的 8 累加器写法在 **release** 下仍然只跑到约 2 GMAC/s
/// （≈ 标量吞吐），因为 Rust 默认以 baseline x86-64 为目标，不会自动发 AVX2。
/// 而相似判定是全库最贵的循环（2.5 万张 = 3.1 亿对 × 512 维 ≈ 1.6e11 次乘加），
/// 这里用「运行时探测 + 独立 target_feature 函数」拿到 AVX2，且不影响在老 CPU /
/// 非 x86 上运行（不支持就回落 `dot_fast`）。
///
/// # Safety
/// 调用前必须确认 `is_x86_feature_detected!("avx2")` 与 `("fma")` 都为真。
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
unsafe fn dot_avx2(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let n = a.len().min(b.len());
    let mut i = 0usize;
    let mut acc0 = _mm256_setzero_ps();
    let mut acc1 = _mm256_setzero_ps();
    // 一次 16 个元素：两条独立累加链，避开 FMA 的 4 周期延迟
    while i + 16 <= n {
        acc0 = _mm256_fmadd_ps(
            _mm256_loadu_ps(a.as_ptr().add(i)),
            _mm256_loadu_ps(b.as_ptr().add(i)),
            acc0,
        );
        acc1 = _mm256_fmadd_ps(
            _mm256_loadu_ps(a.as_ptr().add(i + 8)),
            _mm256_loadu_ps(b.as_ptr().add(i + 8)),
            acc1,
        );
        i += 16;
    }
    while i + 8 <= n {
        acc0 = _mm256_fmadd_ps(
            _mm256_loadu_ps(a.as_ptr().add(i)),
            _mm256_loadu_ps(b.as_ptr().add(i)),
            acc0,
        );
        i += 8;
    }
    let acc = _mm256_add_ps(acc0, acc1);
    let mut s = _mm256_castps256_ps128(acc);
    let hi = _mm256_extractf128_ps(acc, 1);
    s = _mm_add_ps(s, hi);
    // 4 路水平求和：先把高两路折到低两路，再把 lane1 加到 lane0
    // （漏掉后一步会只累加一半通道 → 点积偏小、全部漏配，实测 16581 → 0 条就是这 bug）
    let mut s = _mm_add_ps(s, _mm_movehl_ps(s, s));
    s = _mm_add_ss(s, _mm_shuffle_ps(s, s, 0x55));
    let mut sum = _mm_cvtss_f32(s);
    // 收尾（512 维通常整除，但不能假设）
    while i < n {
        sum += a[i] * b[i];
        i += 1;
    }
    sum
}

/// 本机是否可用 AVX2+FMA（只探测一次）
#[inline]
fn use_avx2() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use std::sync::OnceLock;
        static FLAG: OnceLock<bool> = OnceLock::new();
        *FLAG.get_or_init(|| {
            std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
        })
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// 按阈值把候选聚成组（完全链接），返回每个组的 entry 下标
fn cluster_by_similarity(
    cand: &[(usize, &Vec<f32>)],
    threshold: f32,
    mut on_progress: impl FnMut(usize, usize),
) -> Vec<Vec<usize>> {
    let n = cand.len();
    if n < 2 {
        return Vec::new();
    }
    // 0) 摊平成连续矩阵：nxD 的 f32 一条连续内存。
    //    原实现每个向量各自一次堆分配，热循环里每对都要跳两块互不相邻的内存，
    //    L2/L3 命中率差；摊平后顺序扫描，硬件预取器能吃到整条 cache line。
    let dims = cand.iter().map(|(_, v)| v.len()).min().unwrap_or(0);
    let mut flat: Vec<f32> = Vec::with_capacity(n * dims);
    for (_, v) in cand {
        flat.extend_from_slice(&v[..dims]);
    }

    // 1) 选方差最大的维度做排序键（区分度越高，窗口内的候选越少）
    let dim = max_variance_dim_flat(&flat, n, dims);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&x, &y| {
        flat[x * dims + dim]
            .partial_cmp(&flat[y * dims + dim])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.cmp(&y))
    });
    // 2) 由 cos ≥ t 推出的一维窗口上界
    //
    // ⚠️ 实测（2.5 万张真实 CLIP 向量，阈值 0.9）：这条剪枝**只剪掉 0.0%**。
    // 原因：512 维单位向量任一坐标的分布极窄（≈1/√D ≈0.044），而窗口有 0.447，
    // 几乎全部点对都落在窗口内。它只在「某一维特别有区分度」的数据上有用
    // （如合成数据），保留是因为成本极低、且对这类数据确实有效，不当作性能支柱。
    let window = (2.0f32 * (1.0 - threshold)).max(0.0).sqrt() + 1e-6;

    // 3) 只在窗口内两两算点积（rayon 并行；结果按 (i,j) 收集，与线程调度无关）
    use rayon::prelude::*;
    let compared = std::sync::atomic::AtomicUsize::new(0);
    let avx2 = use_avx2();
    let pairs: Vec<(usize, usize)> = (0..n)
        .into_par_iter()
        .map(|i| {
            // 并行体内只能读进程级原子标记（闭包不能跨线程共享）
            if i % 512 == 0 && DEDUPE_CANCEL.load(Ordering::SeqCst) {
                return Vec::new();
            }
            let ei = cand[order[i]].0;
            let ai = &flat[order[i] * dims..order[i] * dims + dims];
            let xi = ai[dim];
            let mut local: Vec<(usize, usize)> = Vec::new();
            let mut local_cmp = 0usize;
            for j in (i + 1)..n {
                let oj = order[j];
                let bj = &flat[oj * dims..oj * dims + dims];
                let dx = bj[dim] - xi;
                // 升序：一旦超出窗口，后面更远，直接跳出（无损）
                if dx > window {
                    break;
                }
                local_cmp += 1;
                // SAFETY: avx2 已在循环外做过运行时特性探测
                let sim = if avx2 { unsafe { dot_avx2(ai, bj) } } else { dot_fast(ai, bj) };
                if sim >= threshold {
                    local.push((ei, cand[oj].0));
                }
            }
            compared.fetch_add(local_cmp, Ordering::Relaxed);
            local
        })
        .flatten()
        .collect();
    on_progress(n, n);

    // 4) 完全链接聚类（**不是**并查集连通分量）
    //
    // 为什么不能用并查集（实测教训）：真实图库 24905 张、阈值 0.90 时有 29.3 万条
    // 真配对，连通分量直接"串成一串"——最大一组 9328 张、覆盖 18786 张照片。
    // 那对清理界面毫无意义（"留一张删 9327 张"），还会因为链式传递把明显不同的
    // 照片并进来。清理工具的语义必须是「组内任意两张都真的像」，
    // 所以改为完全链接（complete-linkage）贪心：只把**与组内每个成员都有边**的点加进来。
    let mut adj: HashMap<usize, HashSet<usize>> = HashMap::with_capacity(n);
    for (a, b) in &pairs {
        adj.entry(*a).or_default().insert(*b);
        adj.entry(*b).or_default().insert(*a);
    }
    let linked = pairs.len();
    let cmp = compared.load(Ordering::Relaxed);
    let all_pairs = n * (n.saturating_sub(1)) / 2;
    crate::logger::log_info(&format!(
        "[dedupe] 相似判定：候选 {n} 张 · 实际比较 {cmp} 对（全量 {all_pairs} 对，剪掉 {:.1}%）· 真配对 {linked} 条（阈值 {threshold:.2}、窗口 {window:.3}）",
        if all_pairs == 0 { 0.0 } else { 100.0 * (1.0 - cmp as f64 / all_pairs as f64) }
    ));

    let mut groups = complete_linkage_groups(&adj);
    crate::logger::log_info(&format!(
        "[dedupe] 完全链接分组：{} 组 · 最大 {} 张（连通分量口径会明显更大）",
        groups.len(),
        groups.iter().map(|g| g.len()).max().unwrap_or(0)
    ));
    // 保证输出顺序确定（同一批照片、同一阈值 → 同一结果，便于排查与比对）
    for g in groups.iter_mut() {
        g.sort_unstable();
    }
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    groups
}

/// 完全链接贪心聚类：组内**任意两张**都必须 ≥ 阈值
///
/// 输入是相似图的邻接表（只看「≥ 阈值」的边，所以邻接表本身就是全部候选边）。
/// 贪心顺序：度数高的点先成组（更容易长成真正的近重复簇），
/// 然后只吸收「与当前组内每个成员都有边」的邻居 —— 这一步是链式传递的刹车。
///
/// 复杂度：边数 × 组大小。真实数据里近重复簇都很小（实测 p90=5），
/// 所以它比全量两两验证便宜得多。
fn complete_linkage_groups(adj: &HashMap<usize, HashSet<usize>>) -> Vec<Vec<usize>> {
    let mut nodes: Vec<usize> = adj.keys().copied().collect();
    // 度数降序：先处理"人缘最好"的点，簇质量更稳定；同度数用下标保证确定性
    nodes.sort_by(|a, b| {
        adj[b]
            .len()
            .cmp(&adj[a].len())
            .then_with(|| a.cmp(b))
    });
    let mut used: HashSet<usize> = HashSet::with_capacity(nodes.len());
    let mut out: Vec<Vec<usize>> = Vec::new();
    for u in nodes {
        if used.contains(&u) {
            continue;
        }
        used.insert(u);
        let mut cluster = vec![u];
        let mut cand: Vec<usize> = adj.get(&u).map(|s| s.iter().copied().collect()).unwrap_or_default();
        cand.sort_by(|a, b| {
            adj.get(b)
                .map(|s| s.len())
                .unwrap_or(0)
                .cmp(&adj.get(a).map(|s| s.len()).unwrap_or(0))
                .then_with(|| a.cmp(b))
        });
        for v in cand {
            if used.contains(&v) {
                continue;
            }
            // 完全链接判据：v 与簇内**每一个**成员都必须有边
            let all_linked = cluster.iter().all(|m| {
                adj.get(m).map(|s| s.contains(&v)).unwrap_or(false)
            });
            if all_linked {
                cluster.push(v);
                used.insert(v);
            }
        }
        if cluster.len() >= 2 {
            out.push(cluster);
        }
    }
    out
}

/// 方差最大的维度（区分度最高，剪枝窗口里塞的候选最少）——直接扫摊平矩阵
fn max_variance_dim_flat(flat: &[f32], n: usize, dims: usize) -> usize {
    if n == 0 || dims == 0 {
        return 0;
    }
    let mut best = 0usize;
    let mut best_var = -1.0f64;
    for d in 0..dims {
        let mut sum = 0.0f64;
        let mut sq = 0.0f64;
        for i in 0..n {
            let x = flat[i * dims + d] as f64;
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
                &|stage, done, total, message| {
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
            ident: normalize_path_key(&p.to_string_lossy()),
            album_id: 1,
            size_bytes: size,
            mtime_secs: 0,
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
        let (groups, hashed, collapsed) = exact_groups(&entries, &|_, _| {}).unwrap();
        assert_eq!(groups.len(), 1, "只有 a/b 是一组");
        assert_eq!(groups[0].items.len(), 2);
        assert_eq!(hashed, 3, "同尺寸的 3 个文件算了哈希，长度唯一的 d 不算");
        assert_eq!(groups[0].reclaimable_bytes, 15);
        assert_eq!(collapsed, 0, "四份不同文件没有可合并的同址项");
        assert_eq!(groups[0].items[0].dir, dir.to_string_lossy(), "每项都要带地址（所在文件夹）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 嵌套相册场景：相册 A 包含相册 B，两个都入库 → 同一个文件只能算一张
    #[test]
    fn nested_album_same_file_counted_once() {
        let root = tmp_dir("nested");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        write_file(&root, "top.jpg", b"top-level-photo");
        write_file(&sub, "inner.jpg", b"inner-photo-bytes");
        let albums = vec![
            (1i64, root.to_string_lossy().into_owned()),
            (2i64, sub.to_string_lossy().into_owned()),
        ];
        let entries = collect_entries(&albums, &|_, _, _| {});
        assert_eq!(entries.len(), 2, "同一张 sub/inner.jpg 不得在 A/B 两个相册里各算一次");
        // 归属取更具体的子相册
        let inner = entries
            .iter()
            .find(|e| e.path.to_lowercase().ends_with("inner.jpg"))
            .expect("inner.jpg 应在结果里");
        assert_eq!(inner.album_id, 2, "同一文件的归属应取最深的相册");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 组内合并：同一个文件被两种写法指向时，组里只留一张
    #[test]
    fn collapse_same_target_keeps_one() {
        let dir = tmp_dir("collapse");
        let a = write_file(&dir, "a.jpg", b"abc");
        let other = write_file(&dir, "b.jpg", b"abc");
        let dir_str = dir.to_string_lossy().into_owned();
        let mut items = vec![
            DupItem {
                path: a.to_string_lossy().into_owned(),
                dir: dir_str.clone(),
                album_id: 1,
                file_name: "a.jpg".into(),
                size_bytes: 3,
                width: None,
                height: None,
                modified_secs: 0,
            },
            // 同一文件的另一种写法（大小写 + 斜杠方向不同）
            DupItem {
                path: a.to_string_lossy().replace('\\', "/").to_uppercase(),
                dir: dir_str.clone(),
                album_id: 2,
                file_name: "A.JPG".into(),
                size_bytes: 3,
                width: None,
                height: None,
                modified_secs: 0,
            },
            DupItem {
                path: other.to_string_lossy().into_owned(),
                dir: dir_str,
                album_id: 1,
                file_name: "b.jpg".into(),
                size_bytes: 3,
                width: None,
                height: None,
                modified_secs: 0,
            },
        ];
        let dropped = collapse_same_target(&mut items);
        assert_eq!(dropped, 1, "同址的那条被合并掉");
        assert_eq!(items.len(), 2, "两份不同文件仍保留");
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
            let fast = groups_of(cluster_by_similarity(&cand, th, |_, _| {}));
            let slow = groups_of(bruteforce_clusters(&cand, th));
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

    /// 暴力参照：不剪枝、不 SIMD，直接两两算全部边，再用**同一套**完全链接聚类
    /// （这样这个用例只验证「剪枝/向量化是否改变了结果」）
    fn bruteforce_clusters(cand: &[(usize, &Vec<f32>)], th: f32) -> Vec<Vec<usize>> {
        let n = cand.len();
        let mut adj: HashMap<usize, HashSet<usize>> = HashMap::new();
        for i in 0..n {
            for j in (i + 1)..n {
                if cosine(cand[i].1, cand[j].1) >= th {
                    adj.entry(cand[i].0).or_default().insert(cand[j].0);
                    adj.entry(cand[j].0).or_default().insert(cand[i].0);
                }
            }
        }
        complete_linkage_groups(&adj)
    }

    /// 保留策略：分辨率高优先，其次体积，再次更早的修改时间
    #[test]
    fn keeper_prefers_resolution_then_size_then_age() {
        let mk = |path: &str, w: u32, h: u32, size: u64, mtime: i64| DupItem {
            path: path.into(),
            dir: String::new(),
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

    /// AVX2 点积与标量点积必须一致（含非 8 倍数长度与短向量）
    #[test]
    fn avx2_dot_matches_scalar() {
        if !use_avx2() {
            eprintln!("本机无 AVX2，跳过（回落路径由 dot_fast 覆盖）");
            return;
        }
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
        };
        for len in [1usize, 7, 8, 15, 16, 17, 63, 512, 513] {
            let a: Vec<f32> = (0..len).map(|_| next()).collect();
            let b: Vec<f32> = (0..len).map(|_| next()).collect();
            let expect = cosine(&a, &b);
            // SAFETY: 上面已确认 avx2+fma 可用
            let got = unsafe { dot_avx2(&a, &b) };
            assert!(
                (got - expect).abs() <= 1e-4 * expect.abs().max(1.0),
                "len={len}: avx2={got} scalar={expect}"
            );
            // dot_fast 与 scalar 也应一致
            assert!(
                (dot_fast(&a, &b) - expect).abs() <= 1e-4 * expect.abs().max(1.0),
                "len={len}: dot_fast={} scalar={expect}",
                dot_fast(&a, &b)
            );
        }
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
        let report = scan_library(&albums, emb, Some("test-model".into()), 0.9, true, true, &|_, _, _, _| {})
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

    /// 真实图库性能基准（默认忽略）：前后对比（串行 vs 并行）
    ///
    /// `PMS_BENCH_DIRS="D:\a;D:\b" cargo test --lib -- --ignored --nocapture bench_real_library`
    #[test]
    #[ignore = "性能基准：需要 PMS_BENCH_DIRS 指向真实相册目录"]
    fn bench_real_library() {
        let Ok(raw) = std::env::var("PMS_BENCH_DIRS") else {
            eprintln!("未设置 PMS_BENCH_DIRS，跳过");
            return;
        };
        let albums: Vec<(i64, String)> = raw
            .split(';')
            .map(|d| d.trim())
            .filter(|d| !d.is_empty() && Path::new(d).is_dir())
            .enumerate()
            .map(|(i, d)| (i as i64 + 1, d.to_string()))
            .collect();
        eprintln!(
            "[bench] 相册 {} 个 · rayon 线程 {}",
            albums.len(),
            rayon::current_num_threads()
        );

        // ---- A. 收集阶段：旧（串行 + 每张探尺寸）vs 串行不探尺寸 vs 并行 ----
        let t = std::time::Instant::now();
        let mut old_entries: Vec<Entry> = Vec::new();
        let mut old_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (album_id, dir) in albums.iter() {
            if let Ok(paths) = crate::vision::walk_image_paths(dir) {
                for p in paths {
                    // 用 build_entry 保持同一套准入规则，再额外做旧版的冗余探尺寸
                    let Some(mut e) = build_entry(&p, *album_id) else { continue };
                    let _ = crate::imgio::probe_dimensions(&p);
                    if old_seen.insert(e.key.clone()) {
                        e.album_id = *album_id;
                        old_entries.push(e);
                    }
                }
            }
        }
        let old_collect = t.elapsed();
        let serial_n = old_entries.len();

        // 串行但**提前去掉探尺寸**（把「并行提速」与「少做无谓 IO」拆开看）
        let t = std::time::Instant::now();
        let mut serial_lean: Vec<Entry> = Vec::new();
        let mut lean_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (album_id, dir) in albums.iter() {
            if let Ok(paths) = crate::vision::walk_image_paths(dir) {
                for p in paths {
                    let Some(e) = build_entry(&p, *album_id) else { continue };
                    if lean_seen.insert(e.key.clone()) {
                        serial_lean.push(e);
                    }
                }
            }
        }
        let lean_collect = t.elapsed();

        let t = std::time::Instant::now();
        let entries = collect_entries(&albums, &|_, _, _| {});
        let new_collect = t.elapsed();
        eprintln!(
            "[bench collect] 旧串行(含探尺寸)={:?} · 串行不探尺寸={:?} · 新并行={:?} · 照片 {}/{}/{} · 并行提速(对比串行精简) {:.2}x",
            old_collect,
            lean_collect,
            new_collect,
            serial_n,
            serial_lean.len(),
            entries.len(),
            lean_collect.as_secs_f64() / new_collect.as_secs_f64().max(1e-9)
        );
        assert!(
            entries.len() <= serial_n,
            "新口径（真实路径身份）只能比旧的字符串口径合并得更多，不能变多：{} vs {}",
            entries.len(),
            serial_n
        );

        // ---- B. 哈希阶段：串行 vs 并行（同候选集）----
        let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            by_size.entry(e.size_bytes).or_default().push(i);
        }
        let todo: Vec<(usize, u64)> = by_size
            .into_iter()
            .filter(|(_, v)| v.len() >= 2)
            .flat_map(|(size, v)| v.into_iter().map(move |i| (i, size)))
            .collect();
        eprintln!("[bench hash] 体积碰撞候选 {} 个（全库 {}）", todo.len(), entries.len());

        let t = std::time::Instant::now();
        let mut serial_rows: Vec<(String, usize)> = Vec::new();
        for (idx, size) in &todo {
            let e = &entries[*idx];
            if let Ok(h) = sha256_file(Path::new(&e.path), *size) {
                serial_rows.push((h, *idx));
            }
        }
        let serial_hash = t.elapsed();

        let t = std::time::Instant::now();
        let (groups, hashed, _collapsed) = exact_groups(&entries, &|_, _| {}).expect("并行哈希应成功");
        let new_hash = t.elapsed();
        eprintln!(
            "[bench hash] 旧串行={:?} · 新并行={:?} · 写盘哈希 {} vs {} · 组 {} · 提速 {:.2}x",
            serial_hash,
            new_hash,
            serial_rows.len(),
            hashed,
            groups.len(),
            serial_hash.as_secs_f64() / new_hash.as_secs_f64().max(1e-9)
        );

        // 正确性：并行结果的组数必须与串行哈希算出来的分组一致
        let mut m: HashMap<&str, usize> = HashMap::new();
        for (h, _) in &serial_rows {
            *m.entry(h.as_str()).or_insert(0) += 1;
        }
        let serial_groups = m.values().filter(|n| **n >= 2).count();
        assert_eq!(serial_groups, groups.len(), "并行与串行的分组结果必须一致");
    }

    /// 真实 CLIP 向量的聚类基准（默认忽略）：
    /// `PMS_BENCH_VEC=C:\...\vectors.bin cargo test --lib -- --ignored --nocapture bench_real_vectors`
    /// 文件格式：u32 n · u32 d · n*d 个小端 f32（已 L2 归一化）
    #[test]
    #[ignore = "性能基准：需要 PMS_BENCH_VEC 向量文件"]
    fn bench_real_vectors() {
        let Ok(path) = std::env::var("PMS_BENCH_VEC") else {
            eprintln!("未设置 PMS_BENCH_VEC，跳过");
            return;
        };
        let bytes = std::fs::read(&path).expect("读取向量文件失败");
        let n = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let d = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let body = &bytes[8..];
        let mut vecs: Vec<Vec<f32>> = Vec::with_capacity(n);
        for i in 0..n {
            let off = i * d * 4;
            let mut v = Vec::with_capacity(d);
            for k in 0..d {
                let p = off + k * 4;
                v.push(f32::from_le_bytes(body[p..p + 4].try_into().unwrap()));
            }
            vecs.push(v);
        }
        let cand: Vec<(usize, &Vec<f32>)> = vecs.iter().enumerate().collect();
        eprintln!(
            "[bench] 真实向量 n={n} dim={d} · rayon 线程 {}",
            rayon::current_num_threads()
        );
        let one_only = std::env::var("PMS_BENCH_ONE").is_ok();
        let thr: f32 = std::env::var("PMS_BENCH_TH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.9);
        eprintln!("[bench] 阈值 {thr:.2}");
        let modes: Vec<usize> = if one_only { vec![1, 0] } else { vec![0] };
        for th in modes {
            let t = std::time::Instant::now();
            let groups = if th == 1 {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .unwrap()
                    .install(|| cluster_by_similarity(&cand, thr, |_, _| {}))
            } else {
                cluster_by_similarity(&cand, thr, |_, _| {})
            };
            let label = if th == 1 {
                "1".to_string()
            } else {
                format!("auto({})", rayon::current_num_threads())
            };
            let mut sizes: Vec<usize> = groups.iter().map(|g| g.len()).collect();
            sizes.sort_unstable();
            let total: usize = sizes.iter().sum();
            eprintln!(
                "[bench] workers={label} groups={} 覆盖照片={total} 组大小 max={} p50={} p90={} elapsed={:?}",
                groups.len(),
                sizes.last().copied().unwrap_or(0),
                sizes.get(sizes.len() / 2).copied().unwrap_or(0),
                sizes.get(sizes.len() * 9 / 10).copied().unwrap_or(0),
                t.elapsed()
            );
        }
    }

    /// 相似聚类的线程缩放基准（合成向量，隔离磁盘 IO）
    #[test]
    #[ignore = "性能基准：合成 2 万 × 512 维向量"]
    fn bench_cluster_scaling() {
        let n: usize = std::env::var("PMS_BENCH_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20000);
        let d = 512usize;
        // 确定性伪随机（xorshift64），不引入 rand 依赖
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f32 / (1u64 << 53) as f32
        };
        let mut vecs: Vec<Vec<f32>> = (0..n)
            .map(|_| {
                let mut v: Vec<f32> = (0..d).map(|_| next() * 2.0 - 1.0).collect();
                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                for x in v.iter_mut() {
                    *x /= norm;
                }
                v
            })
            .collect();
        // 造 100 组近重复，确保结果里真有组（不是空跑）
        for g in 0..100usize {
            let base = vecs[g * 7].clone();
            for k in 1..3 {
                let idx = (g * 7 + k) % n;
                let mut v: Vec<f32> = base.iter().map(|x| x + 0.005 * k as f32).collect();
                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                for x in v.iter_mut() {
                    *x /= norm;
                }
                vecs[idx] = v;
            }
        }
        let cand: Vec<(usize, &Vec<f32>)> = vecs.iter().enumerate().collect();
        for workers in [1usize, 0] {
            let t = std::time::Instant::now();
            let groups = if workers == 1 {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .unwrap()
                    .install(|| cluster_by_similarity(&cand, 0.9, |_, _| {}))
            } else {
                cluster_by_similarity(&cand, 0.9, |_, _| {})
            };
            let label = if workers == 1 {
                "1".to_string()
            } else {
                format!("auto({})", rayon::current_num_threads())
            };
            eprintln!(
                "[bench] n={n} dim={d} workers={label} groups={} elapsed={:?}",
                groups.len(),
                t.elapsed()
            );
        }
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
            let r = scan_library(&albums, Vec::new(), None, 0.9, true, false, &|_, _, _, _| {});
            assert!(r.is_ok(), "连续两次扫描都应成功（锁必须释放）");
        }
        assert!(!is_running());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真实图库的同址去重校验（默认忽略）：
    ///
    /// `PMS_DB="C:\Users\...\photos.db" cargo test --lib -- --ignored --nocapture verify_real_library_same_path`
    ///
    /// 钉死的是 BUG-2026-1101-003 的不变量：
    /// ① 同一个真实文件（嵌套相册 / junction / 写法差异）不能在结果里出现两次；
    /// ② 每项都带地址（`dir`），前端才能“照片底显示地址”；
    /// ③ 保留项必须在组里，可释放体积与逐项求和一致。
    #[test]
    #[ignore = "真实图库校验：需要 PMS_DB 指向 photos.db"]
    fn verify_real_library_same_path() {
        let _gate = e2e_gate();
        let db_path = std::env::var("PMS_DB").unwrap_or_else(|_| {
            let appdata = std::env::var("APPDATA").unwrap_or_default();
            format!("{appdata}\\com.photomanagementsys.app\\photos.db")
        });
        let conn = rusqlite::Connection::open(&db_path).expect("打开 photos.db 失败");
        let mut stmt = conn.prepare("select id, path from albums").unwrap();
        let albums: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        eprintln!("[verify] DB {db_path} · 相册 {} 个", albums.len());

        let t = std::time::Instant::now();
        let report = scan_library(&albums, Vec::new(), None, 0.98, true, false, &|_, _, _, _| {})
            .expect("扫描应成功");
        eprintln!(
            "[verify] 耗时 {:?} · 照片 {} 张 · 组 {} 个 · 哈希 {} 个文件 · 同址合并 {} 条",
            t.elapsed(),
            report.total_photos,
            report.groups.len(),
            report.hashed_files,
            report.collapsed_same_file
        );

        let mut reclaim = 0u64;
        for g in &report.groups {
            let mut seen_path = HashSet::new();
            let mut seen_addr = HashSet::new();
            for it in &g.items {
                assert!(
                    seen_path.insert(it.path.to_lowercase()),
                    "组 {} 内出现同一路径两次：{}",
                    g.key,
                    it.path
                );
                assert!(!it.dir.is_empty(), "条目缺地址：{}", it.path);
                let addr = format!("{}|{}", it.dir.to_lowercase(), it.file_name.to_lowercase());
                assert!(
                    seen_addr.insert(addr),
                    "组 {} 内出现同一地址两次：{}",
                    g.key,
                    it.path
                );
            }
            assert!(
                g.items.iter().any(|it| it.path == g.keep_path),
                "组 {} 的 keep_path 不在 items 里",
                g.key
            );
            let sum: u64 = g.items.iter().filter(|it| it.path != g.keep_path).map(|it| it.size_bytes).sum();
            assert_eq!(sum, g.reclaimable_bytes, "组 {} 的可释放体积不自洽", g.key);
            reclaim += g.reclaimable_bytes;
        }
        eprintln!("[verify] 可释放合计 {:.1} GB", reclaim as f64 / 1e9);
    }
}
