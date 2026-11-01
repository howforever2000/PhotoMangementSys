//! 向量索引（FEAT-154）：SoA 扁平存储 + rayon 并行点积
//!
//! ## 为什么需要它
//!
//! 智能搜索每敲一次词，要在一到两万条 512 维向量上做一遍余弦扫描（图像塔 + 文本塔两路）。
//! 旧实现（`rank_by_cosine` / `continue_recall`）有三处代价：
//!
//! 1. **`Vec<Vec<f32>>` 每行一次堆分配**：25k 行 = 25k 个独立分配，遍历时一路指针跳转，
//!    SIMD 无法跨行向量化（只能吃一条 512 float 的行内并行，且被间接寻址打断）。
//! 2. **`f64` 累加**（`(*a * *b) as f64`）：x86 上逐元素 cast，编译器只能出标量指令。
//! 3. **单线程**：12.75M 次乘加全挤在一个核上。
//!
//! 实测（本机 25k×512，真实库）：numpy `f32` 单线程 matvec 22ms，纯标量循环 450ms ——
//! 也就是说「同样的数据、只换成连续 f32 + SIMD」就有 20× 空间，再叠 rayon 分块并行
//! 才能进到个位毫秒。这一层的存在就是为了把这块算力榨干，而不是把成本挪个地方。
//!
//! ## 语义红线
//!
//! 搜索是**排序**问题，结果必须与旧的 `f64` 串行实现逐条同序（`vecindex_matches_f64_baseline`
//! 用同一批随机向量把两者钉住）。f32 累加在 512 维上的误差量级 ~1e-7，远小于阈值步进
//! （0.05），也不会改变排序 —— 但这条等价性必须由测试保证，不能靠"应该没事"。

use rayon::prelude::*;

/// 并行分块的行数：一行 512 float 太小，按行切块会让 rayon 的分裂开销占比过高。
///
/// 取 256 行/块：2.5 万行切成 ~98 块，8 核每核 12 块 —— 负载均衡与开销的折中。
/// 单块 256×512×4B = 512KB，正好在 L2 附近，顺序扫不吃缓存。
const ROWS_PER_CHUNK: usize = 256;

/// 只读向量索引：`hashes[i]` 对应 `data[i*dim .. (i+1)*dim]`
///
/// 构建后不再变更（改向量就重建 + 换版本信号），因此可以 `&self` 并发只读，
/// 不需要任何锁；升级到共享缓存时用 `Arc<VecIndex>`。
#[derive(Debug, Default)]
pub struct VecIndex {
    hashes: Vec<String>,
    dim: usize,
    data: Vec<f32>,
    /// 每行所属的「内容等价组」（FEAT-155）：同一段描述文本 → 同组 → 同向量。
    /// 空表示未分组（图像塔用不到分组）。
    groups: Vec<u32>,
}

/// 一路语义排名的结果 + 本路分布统计（FEAT-155）
///
/// `hits` 里带的是 **rel**（= 余弦 − 本路中位余弦），不是原始余弦。
/// 图像塔与文本塔的余弦绝对值尺度完全不同（本机实测：图像塔中位 0.325、
/// 文本塔中位 0.645），任何跨路比较都必须先减掉各自的中位基线，否则
/// 「2019年6月」这种模板描述永远压过真正像的照片。
#[derive(Debug, Default, Clone)]
pub struct LaneRank {
    /// (photo_hash, rel)，按 rel 降序
    pub hits: Vec<(String, f32)>,
    /// 本路中位余弦（基线；展示与日志用）
    pub median: f32,
    /// 本路最大 rel（= top1 − median），用于把 rel 归一到 0~1 当「相关度」
    pub rel_max: f32,
    /// 参与打分的行数
    pub total: usize,
}

impl VecIndex {
    /// 从「(photo_hash, 已 L2 归一化的 512 维向量)」批量构建
    ///
    /// - 维度以**第一条非空向量**为基准，其余维度不符的行直接跳过（换档残渣 / 脏数据，
    ///   与旧实现的 `if v.len() != q.len() { return None }` 同口径）
    /// - 空向量跳过
    pub fn from_rows<I>(rows: I) -> Self
    where
        I: IntoIterator<Item = (String, Vec<f32>)>,
    {
        let mut it = rows.into_iter();
        let mut idx = VecIndex::default();
        for (hash, v) in it.by_ref() {
            if v.is_empty() {
                continue;
            }
            idx.dim = v.len();
            idx.hashes.push(hash);
            idx.data.extend_from_slice(&v);
            break;
        }
        if idx.dim > 0 {
            for (hash, v) in it {
                idx.push_if_dim(hash, &v);
            }
        }
        idx
    }

    /// 追加一行（维度不符或为空则忽略），末尾按需扩容
    pub fn push_if_dim(&mut self, hash: String, v: &[f32]) {
        self.push_if_dim_grouped(hash, v, u32::MAX)
    }

    /// 从「(hash, 向量, 组 id)」批量构建（描述塔用它做同文本折叠）
    pub fn from_rows_grouped<I>(rows: I) -> Self
    where
        I: IntoIterator<Item = (String, Vec<f32>, u32)>,
    {
        let mut idx = VecIndex::default();
        for (hash, v, g) in rows {
            if idx.dim == 0 {
                if v.is_empty() {
                    continue;
                }
                idx.dim = v.len();
            }
            idx.push_if_dim_grouped(hash, &v, g);
        }
        idx
    }

    fn push_if_dim_grouped(&mut self, hash: String, v: &[f32], g: u32) {
        if self.dim == 0 || v.len() != self.dim {
            return;
        }
        self.hashes.push(hash);
        self.data.extend_from_slice(v);
        if g != u32::MAX && self.groups.len() == self.hashes.len() - 1 {
            self.groups.push(g);
        } else if !self.groups.is_empty() {
            // 混入未分组的行：分组信息作废，退回「不折叠」
            self.groups.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 索引占用的堆字节数（日志/诊断用：100MB 级的缓存要看得到）
    pub fn heap_bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<f32>()
            + self.hashes.iter().map(|h| h.len()).sum::<usize>()
    }

    /// 余弦 topK：`q` 必须已 L2 归一化（服务端产出的向量都是），点积即余弦
    ///
    /// - 并行：按 [`ROWS_PER_CHUNK`] 行切块，先块内过滤 `>= min_sim` 再合并
    /// - 排序：`sort_unstable_by` 降序；同分时按索引升序（稳定可比，单测要它）
    /// - 返回只 clone topK 的 hash（不 clone 全库）
    ///
    /// FEAT-155 起**生产路径已全部走 [`Self::rank_rel`]**：绝对余弦当召回闸会把
    /// 相关命中筛掉（BUG-2026-1101-007）。这个函数保留为「串行 f64 基准的对照对象」
    /// （`vecindex_matches_f64_baseline` / `bench_real_db`），改 rank_rel 的并行与
    /// 累加细节时要靠它守住等价性，不要删。
    #[allow(dead_code)]
    pub fn rank(&self, q: &[f32], limit: usize, min_sim: f32) -> Vec<(String, f32)> {
        if self.is_empty() || limit == 0 || q.len() != self.dim || self.dim == 0 {
            return Vec::new();
        }
        let dim = self.dim;
        let mut scored: Vec<(f32, u32)> = self
            .data
            .par_chunks(dim * ROWS_PER_CHUNK)
            .enumerate()
            .fold(Vec::new, |mut acc, (bi, block)| {
                let base = bi * ROWS_PER_CHUNK;
                for (j, row) in block.chunks_exact(dim).enumerate() {
                    let dot = dot_f32(row, q);
                    if dot >= min_sim {
                        acc.push((dot, (base + j) as u32));
                    }
                }
                acc
            })
            .reduce(Vec::new, |mut a, mut b| {
                if a.len() < b.len() {
                    std::mem::swap(&mut a, &mut b);
                }
                a.append(&mut b);
                a
            });
        // 分块归并后整体排序：分数降序，同分按行号升序（与串行版逐条同序）
        scored.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.truncate(limit);
        scored
            .into_iter()
            .map(|(s, i)| (self.hashes[i as usize].clone(), s))
            .collect()
    }
    /// 相对相关度排名（FEAT-155）：**两路语义的唯一召回入口**
    ///
    /// 与 `rank` 的区别是三个：
    /// 1. **不靠库内固定阈值召回**：所有行都参与打分（并行且无分支），再由 rel 下限过滤。
    ///    旧实现把绝对余弦当召回闸（0.30/0.45），而相关命中的绝对区间随 query 漂移
    ///    （本机实测：「自行车」最像的那张只有 0.4501），任何固定阈值不是漏就是滥。
    /// 2. **rel = cos − 本路中位余弦**：图像塔与文本塔的余弦尺度不可比，减掉各自基线
    ///    后才可放在同一张 RRF 表里比。
    /// 3. **同组折叠**：`max_per_group > 0` 时，同一组（同一段描述文本）最多进这么多张。
    ///    不折叠的话，一段模板描述（实测「2019年6月」96 张、「document paper 文档」
    ///    958 张）会把整个候选池占满，翻页看到的全是同一句话的同分项。
    ///
    /// `min_lift`：本路 rel 峰值低于它时整路不参与（告诉调用方「这条 query 在
    /// 这一路根本没有可信命中」），避免把「全库最像的噪声」当成命中推上去。
    pub fn rank_rel(
        &self,
        q: &[f32],
        limit: usize,
        rel_floor: f32,
        max_per_group: usize,
        min_lift: f32,
    ) -> LaneRank {
        let empty = LaneRank { hits: Vec::new(), median: 0.0, rel_max: 0.0, total: 0 };
        if self.is_empty() || limit == 0 || q.len() != self.dim || self.dim == 0 {
            return empty;
        }
        let dim = self.dim;
        // 全量打分（不过滤）：中位数基线需要完整分布
        let scored: Vec<(f32, u32)> = self
            .data
            .par_chunks(dim * ROWS_PER_CHUNK)
            .enumerate()
            .fold(Vec::new, |mut acc, (bi, block)| {
                let base = bi * ROWS_PER_CHUNK;
                for (j, row) in block.chunks_exact(dim).enumerate() {
                    acc.push((dot_f32(row, q), (base + j) as u32));
                }
                acc
            })
            .reduce(Vec::new, |mut a, mut b| {
                if a.len() < b.len() {
                    std::mem::swap(&mut a, &mut b);
                }
                a.append(&mut b);
                a
            });
        let total = scored.len();
        let median = median_of_dots(&scored);
        let rel_max = scored.iter().map(|(s, _)| *s).fold(f32::NEG_INFINITY, f32::max) - median;
        if !(rel_max >= min_lift) {
            // NaN 也走这里（`!(x >= y)` 对 NaN 为真），不会把脏数据当命中
            return LaneRank { hits: Vec::new(), median, rel_max, total };
        }
        let mut rel: Vec<(f32, u32)> = scored
            .into_iter()
            .filter_map(|(s, i)| {
                let r = s - median;
                if r >= rel_floor {
                    Some((r, i))
                } else {
                    None
                }
            })
            .collect();
        // 降序；rel 相同（同一段描述文本 → 同一向量）时按行号升序，结果可复现
        rel.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut hits = Vec::new();
        if max_per_group > 0 && self.groups.len() == self.hashes.len() {
            let mut per: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for (r, i) in rel {
                let g = self.groups[i as usize];
                let c = per.entry(g).or_insert(0);
                if *c >= max_per_group {
                    continue;
                }
                *c += 1;
                hits.push((self.hashes[i as usize].clone(), r));
                if hits.len() >= limit {
                    break;
                }
            }
        } else {
            for (r, i) in rel.into_iter().take(limit) {
                hits.push((self.hashes[i as usize].clone(), r));
            }
        }
        LaneRank { hits, median, rel_max, total }
    }
}

/// 余弦中位数（偶数个取中间两值均值，与 numpy 的 median 同口径）
fn median_of_dots(scored: &[(f32, u32)]) -> f32 {
    let n = scored.len();
    if n == 0 {
        return 0.0;
    }
    let mut v: Vec<f32> = scored.iter().map(|(s, _)| *s).collect();
    let mid = n / 2;
    v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
    if n % 2 == 1 {
        v[mid]
    } else {
        let lo = v[..mid].iter().copied().fold(f32::NEG_INFINITY, f32::max);
        (lo + v[mid]) / 2.0
    }
}

/// 定长点积（f32 累加，交编译器做 SIMD）
///
/// 为什么不用 `zip().map().sum()`：那样会退化成对 `Iterator` 的逐元素闭包调用，
/// 且累加顺序被 `Sum` 固定成串行 —— 这里用 4 路累加器打散依赖链，让 f32 乘法
/// 可以 4 宽流水，实测比朴素写法快约 2.2×（512 维：1.9µs → 0.86µs/行）。
#[inline]
fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut s0 = 0.0f32;
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    let mut s3 = 0.0f32;
    let (a4, b4) = (a, b);
    let chunks = n / 4;
    for i in 0..chunks {
        let o = i * 4;
        s0 += a4[o] * b4[o];
        s1 += a4[o + 1] * b4[o + 1];
        s2 += a4[o + 2] * b4[o + 2];
        s3 += a4[o + 3] * b4[o + 3];
    }
    let mut s = (s0 + s1) + (s2 + s3);
    for i in (chunks * 4)..n {
        s += a4[i] * b4[i];
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 极简线性同余，测试里要「可复现的随机」而不是密码学随机
    struct Lcg(u64);
    impl Lcg {
        fn next_f32(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f32 / (u32::MAX >> 1) as f32) * 2.0 - 1.0
        }
    }

    fn norm(v: &mut [f32]) {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 1e-9 {
            for x in v.iter_mut() {
                *x /= n;
            }
        }
    }

    /// 旧实现原样照抄：f64 累加 + 串行 + 全量排序（等价性基线）
    fn baseline(rows: &[(String, Vec<f32>)], q: &[f32], limit: usize, min_sim: f64) -> Vec<(String, f64)> {
        let mut scored: Vec<(f64, usize)> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, (_h, v))| {
                if v.len() != q.len() || v.is_empty() {
                    return None;
                }
                let dot = v.iter().zip(q.iter()).map(|(a, b)| (*a * *b) as f64).sum::<f64>();
                if dot >= min_sim {
                    Some((dot, i))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
            .into_iter()
            .map(|(s, i)| (rows[i].0.clone(), s))
            .collect()
    }

    /// 等价性红线：并行 f32 版与串行 f64 版 **逐条同序**（分数允许 1e-5 级浮点差）
    #[test]
    fn vecindex_matches_f64_baseline() {
        let mut rng = Lcg(0x5EED_154);
        let dim = 64;
        let n = 1500;
        let rows: Vec<(String, Vec<f32>)> = (0..n)
            .map(|i| {
                let mut v: Vec<f32> = (0..dim).map(|_| rng.next_f32()).collect();
                norm(&mut v);
                (format!("h{i:04}"), v)
            })
            .collect();
        let mut q: Vec<f32> = (0..dim).map(|_| rng.next_f32()).collect();
        norm(&mut q);

        for &min in &[0.0f64, 0.3, 0.45, 0.9] {
            for &limit in &[1usize, 7, 100, 5000] {
                let want = baseline(&rows, &q, limit, min);
                let idx = VecIndex::from_rows(rows.clone());
                let got = idx.rank(&q, limit, min as f32);
                assert_eq!(got.len(), want.len(), "min={min} limit={limit} 条数不一致");
                for (g, w) in got.iter().zip(want.iter()) {
                    assert_eq!(g.0, w.0, "min={min} limit={limit} 顺序不一致");
                    assert!(
                        (g.1 as f64 - w.1).abs() < 1e-5,
                        "min={min} limit={limit} 分数偏差过大: {} vs {}",
                        g.1,
                        w.1
                    );
                }
            }
        }
    }

    #[test]
    fn vecindex_skips_wrong_dim_and_empty() {
        let mut idx = VecIndex::from_rows(vec![
            ("a".into(), vec![1.0, 0.0, 0.0]),
            ("bad_dim".into(), vec![1.0, 0.0]),
            ("empty".into(), vec![]),
            ("b".into(), vec![0.0, 1.0, 0.0]),
        ]);
        assert_eq!(idx.len(), 2, "维度不符与空向量必须跳过");
        assert_eq!(idx.dim(), 3);
        let out = idx.rank(&[1.0, 0.0, 0.0], 10, 0.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "a");

        // 空索引 / 空查询 / limit=0 都是空结果，不 panic
        idx = VecIndex::default();
        assert!(idx.rank(&[1.0], 10, 0.0).is_empty());
        let idx = VecIndex::from_rows(vec![("a".into(), vec![1.0, 0.0])]);
        assert!(idx.rank(&[1.0], 10, 0.0).is_empty(), "查询维度不符应返回空");
        assert!(idx.rank(&[1.0, 0.0], 0, 0.0).is_empty(), "limit=0 应返回空");
    }

    /// 同分按行号升序（稳定可比）：否则同一查询两次的结果顺序可能不同
    #[test]
    fn vecindex_ties_are_stable_by_row_index() {
        let idx = VecIndex::from_rows(vec![
            ("a".into(), vec![1.0, 0.0]),
            ("b".into(), vec![1.0, 0.0]),
            ("c".into(), vec![1.0, 0.0]),
        ]);
        let out = idx.rank(&[1.0, 0.0], 3, 0.0);
        assert_eq!(out.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(), vec!["a", "b", "c"]);
    }

    /// 相对相关度：rel = cos − 本路中位余弦，且命中按 rel 降序
    #[test]
    fn rank_rel_uses_lane_median_as_baseline() {
        // 5 行：q = e0，余弦 = 第一分量 → 1.0 / 0.6 / 0.4 / 0.2 / 0.0
        let rows: Vec<(String, Vec<f32>)> = [
            ("a", 1.0f32),
            ("b", 0.6),
            ("c", 0.4),
            ("d", 0.2),
            ("e", 0.0),
        ]
        .iter()
        .map(|(h, x)| {
            let mut v = vec![0.0f32; 2];
            v[0] = *x;
            v[1] = (1.0 - x * x).sqrt();
            (h.to_string(), v)
        })
        .collect();
        let idx = VecIndex::from_rows(rows);
        let out = idx.rank_rel(&[1.0, 0.0], 10, 0.0, 0, 0.0);
        assert!((out.median - 0.4).abs() < 1e-6, "中位数应为 0.4，实际 {}", out.median);
        assert!((out.rel_max - 0.6).abs() < 1e-6, "rel_max 应为 1.0-0.4=0.6");
        assert_eq!(out.hits[0].0, "a");
        assert!((out.hits[0].1 - 0.6).abs() < 1e-6);
        // rel 下限过滤：只留 rel >= 0.2 的（a 0.6 / b 0.2）
        let out = idx.rank_rel(&[1.0, 0.0], 10, 0.2, 0, 0.0);
        assert_eq!(out.hits.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        // 本路峰值太低 → 整路不参与
        let out = idx.rank_rel(&[1.0, 0.0], 10, 0.0, 0, 0.61);
        assert!(out.hits.is_empty(), "min_lift 未生效");
        assert!((out.rel_max - 0.6).abs() < 1e-6, "整路关闭时也要回报 rel_max");
    }

    /// 同组折叠：同一段描述文本（同向量）最多入池 max_per_group 张
    #[test]
    fn rank_rel_folds_same_group() {
        let v_same = vec![1.0f32, 0.0];
        let v_other = vec![0.0f32, 1.0];
        let rows = vec![
            ("t1".to_string(), v_same.clone(), 0u32),
            ("t2".to_string(), v_same.clone(), 0u32),
            ("t3".to_string(), v_same.clone(), 0u32),
            ("o1".to_string(), v_other.clone(), 1u32),
        ];
        let idx = VecIndex::from_rows_grouped(rows);
        let out = idx.rank_rel(&[1.0, 0.0], 10, 0.0, 2, 0.0);
        let hashes: Vec<&str> = out.hits.iter().map(|(h, _)| h.as_str()).collect();
        assert_eq!(hashes, vec!["t1", "t2"], "同组必须只取 2 张");
        let out = idx.rank_rel(&[1.0, 0.0], 10, 0.0, 0, 0.0);
        assert_eq!(out.hits.len(), 3, "max_per_group=0 时不折叠");
    }

    /// 真实库交叉校验（手动跑，FEAT-155）：
    ///
    /// `cargo test --lib probe_real_lanes -- --ignored --nocapture`
    ///
    /// 读 `%APPDATA%\com.photomanagementsys.app\photos.db` 的**只读副本**，跑**线上同一套**
    /// `VecIndex::rank_rel`，打印两路 (hash, rel)，与 `python/bench/eval_search_quality.py`
    /// 的离线模拟逐条对比 —— 评测台是模拟的，必须用真实现钉住“两边没漂”。
    /// 查询向量由 `python/bench/export_rust_probe.py` 调真实文本塔编码后落盘。
    #[test]
    #[ignore]
    fn probe_real_lanes() {
        use crate::db::embedding::f32_vec_from_blob;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("仓库根")
            .to_path_buf();
        let probe = root.join(".pi").join("search_eval").join("rust_probe.json");
        let Ok(raw) = std::fs::read_to_string(&probe) else {
            eprintln!("[probe] 跳过：找不到 {}", probe.display());
            return;
        };
        let v: serde_json::Value = serde_json::from_str(&raw).expect("probe json");
        let appdata = std::env::var("APPDATA").unwrap_or_default();
        let db_path = format!(r"{appdata}\com.photomanagementsys.app\photos.db");
        if !std::path::Path::new(&db_path).is_file() {
            eprintln!("[probe] 跳过：找不到 {db_path}");
            return;
        }
        let conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("只读打开");

        // 图像塔（与 content::image_index 同 SQL）
        let mut stmt = conn
            .prepare(
                "SELECT photo_hash, embedding FROM photo_embeddings
                 WHERE user_id = 1 AND model = 'chinese-clip-vit-b16'",
            )
            .expect("img stmt");
        let img_rows: Vec<(String, Vec<f32>)> = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, f32_vec_from_blob(&r.get::<_, Vec<u8>>(1)?)))
            })
            .expect("img query")
            .filter_map(Result::ok)
            .collect();
        // 描述塔（与 db::embedding::load_text_embeddings_grouped 同口径）
        let mut stmt = conn
            .prepare(
                "SELECT t.photo_hash, t.text, t.embedding FROM photo_text_embeddings t
                 JOIN photo_content_scan p ON p.photo_hash = t.photo_hash
                 WHERE p.user_id = 1 AND t.kind = 'desc' AND t.model = 'chinese-clip-vit-b16'",
            )
            .expect("desc stmt");
        let raw_desc: Vec<(String, String, Vec<f32>)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    f32_vec_from_blob(&r.get::<_, Vec<u8>>(2)?),
                ))
            })
            .expect("desc query")
            .filter_map(Result::ok)
            .collect();
        let mut gmap: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let desc_rows: Vec<(String, Vec<f32>, u32)> = raw_desc
            .into_iter()
            .map(|(h, t, v)| {
                let next = gmap.len() as u32;
                let g = *gmap.entry(t).or_insert(next);
                (h, v, g)
            })
            .collect();
        eprintln!("[probe] 图像 {} 条 · 描述 {} 条（{} 组）", img_rows.len(), desc_rows.len(), gmap.len());

        let img_idx = crate::vecindex::VecIndex::from_rows(img_rows);
        let desc_idx = crate::vecindex::VecIndex::from_rows_grouped(desc_rows);
        for q in v["queries"].as_array().expect("queries") {
            let text = q["q"].as_str().unwrap_or_default();
            let vec: Vec<f32> = q["vec"]
                .as_array()
                .expect("vec")
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect();
            let kind = crate::content::query_kind(text);
            let fold = match kind {
                crate::content::QueryKind::Object => 2,
                _ => 0,
            };
            let img = img_idx.rank_rel(
                &vec,
                crate::content::SEMANTIC_IMG_CANDIDATES,
                crate::content::SEMANTIC_REL_FLOOR_DEFAULT as f32,
                0,
                crate::content::SEMANTIC_LANE_MIN_LIFT as f32,
            );
            let desc = desc_idx.rank_rel(
                &vec,
                crate::content::SEMANTIC_DESC_CANDIDATES,
                crate::content::SEMANTIC_REL_FLOOR_DEFAULT as f32,
                fold,
                crate::content::SEMANTIC_LANE_MIN_LIFT as f32,
            );
            eprintln!(
                "PROBE\t{text}\t{}\timg_median={:.6}\timg_relmax={:.6}\tdesc_median={:.6}\tdesc_relmax={:.6}",
                kind.as_str(),
                img.median,
                img.rel_max,
                desc.median,
                desc.rel_max
            );
            for (lane, r) in [("img", &img), ("desc", &desc)] {
                for (h, rel) in r.hits.iter().take(200) {
                    eprintln!("LANE\t{text}\t{lane}\t{h}\t{rel:.6}");
                }
            }
        }
    }

    /// 大库下的吞吐守卫：25k×512 必须远低于一秒（并行生效的弱断言，
    /// 主要防止「误改成串行后仍无人发现」）
    ///
    /// 阈值取 -1.0 是为了让随机向量也全量命中（512 维均匀随机向量的余弦都在 0 附近），
    /// 这样测到的是「全部行都参与排序」的最坏情况，而不是空集。
    #[test]
    fn vecindex_rank_25k_is_fast() {
        let dim = 512;
        let n = 25_000;
        let mut data = Vec::with_capacity(n * dim);
        let mut rng = Lcg(42);
        for _ in 0..n * dim {
            data.push(rng.next_f32());
        }
        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            let s = i * dim;
            let mut v = data[s..s + dim].to_vec();
            norm(&mut v);
            rows.push((format!("h{i}"), v));
        }
        let idx = VecIndex::from_rows(rows);
        let mut q: Vec<f32> = (0..dim).map(|_| rng.next_f32()).collect();
        norm(&mut q);
        let t0 = std::time::Instant::now();
        let out = idx.rank(&q, 600, -1.0);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        eprintln!("[vecindex] 25k x 512 rank -> {} 命中（全量参与）, {:.1} ms", out.len(), ms);
        assert_eq!(out.len(), 600);
        assert!(ms < 500.0, "25k x 512 排名用了 {ms:.1} ms，疑似并行失效");

        // FEAT-155：rank_rel 多一次「全量打分 + 中位数选择 + 折叠」，不得把搜索拖慢
        let t0 = std::time::Instant::now();
        let rel = idx.rank_rel(&q, 300, 0.005, 2, 0.02);
        let ms_rel = t0.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "[vecindex] 25k x 512 rank_rel -> {} 命中 · 中位 {:.4} · 峰值rel {:.4}, {:.1} ms",
            rel.hits.len(),
            rel.median,
            rel.rel_max,
            ms_rel
        );
        assert!(!rel.hits.is_empty());
        assert!(ms_rel < 500.0, "rank_rel 用了 {ms_rel:.1} ms，疑似并行失效");
    }

    /// 真实库对照（手动跑）：`cargo test --lib bench_real_db -- --ignored --nocapture`
    ///
    /// 读 `%APPDATA%\com.photomanagementsys.app\photos.db` 的 **只读副本**（不碰生产库），
    /// 对比三种实现的真实耗时：旧串行 f64 / f32 串行 / f32 并行。
    /// 库里没有向量时直接跳过（CI 上无意义）。
    #[test]
    #[ignore]
    fn bench_real_db() {
        let db_path = std::env::var("PMS_BENCH_DB").unwrap_or_else(|_| {
            let appdata = std::env::var("APPDATA").unwrap_or_default();
            format!(r"{appdata}\com.photomanagementsys.app\photos.db")
        });
        if !std::path::Path::new(&db_path).is_file() {
            eprintln!("[bench] 跳过：找不到 {db_path}");
            return;
        }
        let conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("只读打开");
        let mut stmt = conn
            .prepare(
                "SELECT photo_hash, embedding FROM photo_embeddings
                 WHERE user_id = 1 AND model = 'chinese-clip-vit-b16'",
            )
            .expect("prepare");
        let t_load = std::time::Instant::now();
        let rows: Vec<(String, Vec<f32>)> = stmt
            .query_map([], |r| {
                let h: String = r.get(0)?;
                let blob: Vec<u8> = r.get(1)?;
                Ok((h, crate::db::embedding::f32_vec_from_blob(&blob)))
            })
            .expect("query")
            .filter_map(Result::ok)
            .collect();
        let load_ms = t_load.elapsed().as_secs_f64() * 1000.0;
        eprintln!("[bench] 读库+解码 {} 条：{load_ms:.1} ms", rows.len());
        if rows.is_empty() {
            return;
        }
        let mut q = rows[0].1.clone();
        norm(&mut q);

        let t = std::time::Instant::now();
        let serial = baseline(&rows, &q, 600, 0.45);
        let serial_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = std::time::Instant::now();
        let idx = VecIndex::from_rows(rows);
        let build_ms = t.elapsed().as_secs_f64() * 1000.0;

        // 串行 f32（临时把线程池压到 1 线程，模拟「只有单核」的下限）
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let t = std::time::Instant::now();
        let one = pool.install(|| idx.rank(&q, 600, 0.45));
        let one_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = std::time::Instant::now();
        let par = idx.rank(&q, 600, 0.45);
        let par_ms = t.elapsed().as_secs_f64() * 1000.0;

        eprintln!(
            "[bench] 旧串行 f64 {serial_ms:.1} ms | f32 单线程 {one_ms:.1} ms | f32 并行 {par_ms:.1} ms | 建索引 {build_ms:.1} ms"
        );
        eprintln!(
            "[bench] 命中数：旧 {} / 单线程 {} / 并行 {}；加速比 {:.1}x",
            serial.len(),
            one.len(),
            par.len(),
            serial_ms / par_ms.max(0.001)
        );
        assert_eq!(par.len(), one.len());

        // ---- 回表对照：旧形态（两路各 2000 条）vs 新形态（并集一次 1200 条）----
        // SQL 形状与 `lookup_hits_keyed` 一致（分片 500 + 扫描表/缩略图表各一次）
        let hash_of = |i: usize| -> String {
            let row: (String,) = conn
                .query_row(
                    "SELECT photo_hash FROM photo_embeddings WHERE user_id=1 AND model='chinese-clip-vit-b16' LIMIT 1 OFFSET ?1",
                    [i as i64],
                    |r| Ok((r.get(0)?,)),
                )
                .expect("取 hash");
            row.0
        };
        let sample: Vec<String> = (0..2000).map(hash_of).collect();
        let lookup = |hashes: &[String]| -> usize {
            let mut n = 0usize;
            for chunk in hashes.chunks(500) {
                let ph = vec!["?"; chunk.len()].join(",");
                let sql_scan = format!(
                    "SELECT p.photo_hash FROM photo_content_scan p
                     LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
                     WHERE p.user_id = ?1 AND p.photo_hash IN ({ph})"
                );
                let sql_thumb = format!(
                    "SELECT t.photo_hash FROM photo_thumb_cache t
                     LEFT JOIN albums a ON a.id = t.album_id
                     WHERE t.user_id = ?1 AND t.photo_hash IN ({ph})"
                );
                for sql in [sql_scan, sql_thumb] {
                    let mut st = conn.prepare(&sql).expect("prepare lookup");
                    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&1i64];
                    for h in chunk {
                        params.push(h);
                    }
                    n += st.query_map(params.as_slice(), |_| Ok(())).expect("query").count();
                }
            }
            n
        };
        let t = std::time::Instant::now();
        let n_old = lookup(&sample) + lookup(&sample);
        let old_lookup_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = std::time::Instant::now();
        let n_new = lookup(&sample[..1200]);
        let new_lookup_ms = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "[bench] 回表：旧（2000×2 路）{old_lookup_ms:.1} ms / {} 行 | 新（并集 1200 一次）{new_lookup_ms:.1} ms / {} 行",
            n_old, n_new
        );
    }
}
