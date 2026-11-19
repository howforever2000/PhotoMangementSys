//! 分段并行下载（模型下载器的「快路径」）
//!
//! 为什么需要：创意工坊的本地增强模型走 GitHub Release 直链，**单连接**实测只有
//! 0.06~0.14 MB/s（340MB 的 GFPGAN ≈ 94 分钟）。同一台机器实测：
//!
//! | 通道 | 单连接 | 4 连接 |
//! |---|---|---|
//! | GitHub 直连 | 0.06~0.14 MB/s | 无改善（限速在服务端） |
//! | ghfast.top 加速 | 0.30~0.33 MB/s | **0.77 MB/s** |
//! | ModelScope | 4.1 MB/s | — |
//!
//! 即：**并发分段能把代理通道的吞吐抬到单连接的 2.3 倍**，而 8 连接会被镜像 429 拦截，
//! 所以并发上限固定为 4（见 `SEG_MAX_CONNS`）。
//!
//! 职责边界：本模块只管「把一个已确定大小的文件切成若干段、并发拉取、按序拼回」，
//! 不关心模型注册表、进度 UI、源治理与落位策略 —— 那些留在 `model_dl.rs`。
//! 这样分段逻辑可以独立构造 `Segment` 做纯函数测试，不必拉起 AppHandle 或网络。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use futures_util::StreamExt;

/// 触发分段并行的最小体积。
///
/// 低于此值时分段带来的握手开销大于收益（小文件本来就秒下完），继续走单流路径。
pub const SEG_MIN_BYTES: u64 = 16 * 1024 * 1024;

/// 单文件并发连接数上限。
///
/// 实测 ghfast.top 在 8 路并发时开始返回 429（Too Many Requests），4 路稳定且吞吐最高。
pub const SEG_MAX_CONNS: usize = 4;

/// 单段最小体积：避免把 20MB 切成 4 个 5MB 之外的碎段，也保证 4~16MB 区间的文件只有 1~2 段。
const SEG_MIN_CHUNK: u64 = 4 * 1024 * 1024;

/// 分段下载被用户取消时的固定错误文案（上层据此保留断点，不删分片）
const CANCELLED: &str = "已取消（已保留断点，可再次点击下载续传）";

/// 一段待下载区间（闭区间字节范围 + 负责它的源）
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// 段序号（决定拼回顺序与分片文件名）
    pub index: usize,
    /// 起始字节（含）
    pub start: u64,
    /// 结束字节（含）
    pub end: u64,
    /// 该段的下载地址（可来自不同镜像）
    pub url: String,
}

impl Segment {
    /// 该段应写入的字节数
    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// 把 `[0, total)` 均分成若干段，并把段**轮转分配**给各候选源。
///
/// 轮转而不是「每段都用第一个源」：多镜像并存时段分散到不同域名，才能同时吃到
/// 多条链路的带宽（实测 ghfast + gh-proxy 相加即可超过任一单源）。
///
/// 纯函数、无副作用，便于单测覆盖「无缝无重叠」这个关键不变量。
pub fn plan_segments(total: u64, urls: &[String]) -> Vec<Segment> {
    if total == 0 || urls.is_empty() {
        return Vec::new();
    }
    // 段数：受并发上限、候选源数量与最小段体积共同约束，至少 1 段
    let by_size = (total / SEG_MIN_CHUNK).max(1) as usize;
    let count = SEG_MAX_CONNS.min(by_size).max(1);
    // 按段数均分；余数摊给前面的段，保证拼回来正好 total 字节
    let base = total / count as u64;
    let rem = total % count as u64;
    let mut out = Vec::with_capacity(count);
    let mut start = 0u64;
    for i in 0..count {
        let len = base + if (i as u64) < rem { 1 } else { 0 };
        if len == 0 {
            continue;
        }
        out.push(Segment {
            index: out.len(),
            start,
            end: start + len - 1,
            url: urls[i % urls.len()].clone(),
        });
        start += len;
    }
    out
}

/// 分片文件名（沿用以 `.dl_` 开头的约定，会被 `model_dl::cleanup_partials` 一并清理）
pub fn part_path(dir: &Path, key: &str, ext: &str, index: usize) -> PathBuf {
    dir.join(format!(".dl_{key}_seg{index}.{ext}"))
}

/// 下载并拼接全部分片，返回拼好的文件路径。
///
/// - `on_bytes(delta)`：每写入一块回调一次，由调用方聚合成进度事件；
/// - `cancel()`：返回 true 时中止（已写分片保留，下次点击可续传）。
pub async fn fetch_and_concat(
    client: &reqwest::Client,
    plan: Vec<Segment>,
    dir: &Path,
    key: &str,
    ext: &str,
    cancel: Arc<dyn Fn() -> bool + Send + Sync>,
    on_bytes: Arc<dyn Fn(u64) + Send + Sync>,
) -> Result<PathBuf, String> {
    let parts: Vec<PathBuf> = plan
        .iter()
        .map(|s| part_path(dir, key, ext, s.index))
        .collect();

    let jobs = plan.iter().zip(parts.iter()).map(|(seg, path)| {
        let client = client.clone();
        let seg = seg.clone();
        let path = path.clone();
        let cancel = cancel.clone();
        let on_bytes = on_bytes.clone();
        async move { fetch_one(&client, &seg, &path, cancel, on_bytes).await }
    });
    let results = join_all(jobs).await;

    // 逐个检查：任一段失败都不拼接（否则会拼出「看似完整、实际缺块」的坏文件）
    for r in results {
        r?;
    }

    let merged = dir.join(format!(".dl_{key}_merged.{ext}"));
    concat_parts(&parts, &merged)?;
    for p in &parts {
        let _ = std::fs::remove_file(p);
    }
    Ok(merged)
}

/// 单段下载。已有同名分片且长度正确时直接跳过（断点续传）。
async fn fetch_one(
    client: &reqwest::Client,
    seg: &Segment,
    path: &Path,
    cancel: Arc<dyn Fn() -> bool + Send + Sync>,
    on_bytes: Arc<dyn Fn(u64) + Send + Sync>,
) -> Result<(), String> {
    let want = seg.len();
    let have = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if have == want {
        on_bytes(want); // 已完成的断点也计入进度，避免续传时进度条从 0 跳
        return Ok(());
    }
    // 长度不符（上次中断 / 文件已被替换）→ 整段重下，绝不拼接不一致的分片
    if have > 0 {
        let _ = std::fs::remove_file(path);
    }
    if cancel() {
        return Err(CANCELLED.into());
    }

    let resp = client
        .get(&seg.url)
        .header("Range", format!("bytes={}-{}", seg.start, seg.end))
        .send()
        .await
        .map_err(|e| format!("分段请求失败（{}）: {e}", seg.url))?;
    let status = resp.status().as_u16();
    if status != 206 {
        // 200 = 服务器忽略了 Range（会返回整个文件），此时分段没有意义，
        // 交上层回落单流；其它状态码同样视为该源不可用于分段。
        return Err(format!(
            "源不支持分段（HTTP {status}，期望 206）: {}",
            seg.url
        ));
    }

    let mut file = std::fs::File::create(path).map_err(|e| format!("创建分片失败: {e}"))?;
    let mut got = 0u64;
    let mut stream = resp.bytes_stream();
    use std::io::Write as _;
    while let Some(chunk) = stream.next().await {
        if cancel() {
            let _ = file.sync_all();
            return Err(CANCELLED.into());
        }
        let c = chunk.map_err(|e| format!("分段下载中断: {e}"))?;
        // 防御：镜像若超发预算字节，截断而不写坏后续段
        let remain = want - got;
        let take = if c.len() as u64 > remain {
            &c[..remain as usize]
        } else {
            &c[..]
        };
        file.write_all(take).map_err(|e| format!("写分片失败: {e}"))?;
        got += take.len() as u64;
        on_bytes(take.len() as u64);
        if got >= want {
            break;
        }
    }
    let _ = file.sync_all();
    if got != want {
        return Err(format!(
            "分段不完整：期望 {want} 字节，实际 {got} 字节（{}）",
            seg.url
        ));
    }
    Ok(())
}

/// 按序把分片拼成完整文件（1MB 缓冲，避免为大文件分配大块内存）
fn concat_parts(parts: &[PathBuf], out: &Path) -> Result<(), String> {
    use std::io::{Read as _, Write as _};
    let mut dst = std::fs::File::create(out).map_err(|e| format!("创建合并文件失败: {e}"))?;
    let mut buf = vec![0u8; 1024 * 1024];
    for p in parts {
        let mut src =
            std::fs::File::open(p).map_err(|e| format!("打开分片失败 {}: {e}", p.display()))?;
        loop {
            let n = src
                .read(&mut buf)
                .map_err(|e| format!("读取分片失败 {}: {e}", p.display()))?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])
                .map_err(|e| format!("写入合并文件失败: {e}"))?;
        }
    }
    let _ = dst.sync_all();
    Ok(())
}

/// 进度节流器：并发 4 路每次 64KB 回调会把事件刷爆，这里统一按 150ms 节流。
pub struct ProgressThrottle {
    last: Mutex<Instant>,
    interval: Duration,
}

impl Default for ProgressThrottle {
    fn default() -> Self {
        Self::new(Duration::from_millis(150))
    }
}

impl ProgressThrottle {
    pub fn new(interval: Duration) -> Self {
        Self {
            // 初值取过去时刻，保证第一次回调立即上报（点击后界面马上有反应）
            last: Mutex::new(Instant::now() - interval),
            interval,
        }
    }

    /// 到点则返回 true（调用方据此真正 emit）
    pub fn should_emit(&self) -> bool {
        let Ok(mut last) = self.last.lock() else {
            return true;
        };
        if last.elapsed() >= self.interval {
            *last = Instant::now();
            true
        } else {
            false
        }
    }
}

/// 累加器：多线程共享的已下载字节数
#[derive(Default)]
pub struct ByteCounter(AtomicU64);

impl ByteCounter {
    pub fn add(&self, n: u64) -> u64 {
        self.0.fetch_add(n, Ordering::Relaxed) + n
    }
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("https://m{i}.example.com/f")).collect()
    }

    /// 关键不变量：分段必须**无缝、无重叠、合计等于总长**，否则拼出来的文件是坏的。
    #[test]
    fn segments_cover_total_without_gap_or_overlap() {
        for total in [16u64 * 1024 * 1024, 69_539_503, 340_299_087, 1_000_000_001] {
            let plan = plan_segments(total, &urls(3));
            assert!(!plan.is_empty(), "total={total} 应产出分段");
            assert_eq!(plan[0].start, 0, "首段必须从 0 开始");
            assert_eq!(
                plan.last().unwrap().end,
                total - 1,
                "末段必须收在 total-1"
            );
            let mut cursor = 0u64;
            for seg in &plan {
                assert_eq!(seg.start, cursor, "段之间不得有空洞或重叠");
                assert!(seg.len() > 0, "段不得为空");
                cursor = seg.end + 1;
            }
            assert_eq!(cursor, total, "各段长度之和必须等于总长");
            let sum: u64 = plan.iter().map(|s| s.len()).sum();
            assert_eq!(sum, total);
        }
    }

    /// 段会轮转分配到不同源 —— 多镜像并存时才能同时吃多条链路带宽。
    #[test]
    fn segments_rotate_across_sources() {
        let plan = plan_segments(340_299_087, &urls(2));
        assert_eq!(plan.len(), SEG_MAX_CONNS, "大文件应铺满并发上限");
        // 轮转：4 段 / 2 源 → 0,1,0,1
        assert_eq!(plan[0].url, urls(2)[0]);
        assert_eq!(plan[1].url, urls(2)[1]);
        assert_eq!(plan[2].url, urls(2)[0]);
        assert_eq!(plan[3].url, urls(2)[1]);
    }

    /// 小文件不该被切碎：16MB 以下没有分段收益（上层走单流，这里只保证纯函数不越界）。
    #[test]
    fn small_file_single_segment() {
        let plan = plan_segments(1024, &urls(2));
        assert_eq!(plan.len(), 1);
        assert_eq!((plan[0].start, plan[0].end), (0, 1023));
    }

    /// 防御：总长为 0 或没有候选源时不得产出分段（避免上层拿到空计划去拼接）。
    #[test]
    fn empty_inputs_yield_no_plan() {
        assert!(plan_segments(0, &urls(2)).is_empty());
        assert!(plan_segments(1024, &[]).is_empty());
    }

    /// 并发上限固定为 4：实测 8 路会被 ghfast 429 拦截，改大之前必须先重测。
    #[test]
    fn concurrency_cap_is_four() {
        assert_eq!(SEG_MAX_CONNS, 4);
        assert_eq!(plan_segments(u64::MAX / 2, &urls(3)).len(), 4);
    }
}
