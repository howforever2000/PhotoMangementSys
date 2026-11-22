//! 回收站批量删除（分块 + 多线程 + 进度上报）
//!
//! ══════════════════════════════════════════════════════════════════════════
//! 为什么需要这一层（BUG：删除 2654 张耗时 211 秒且界面全程无反馈）
//! ──────────────────────────────────────────────────────────────────────────
//! 旧实现是 `for p in paths { trash::delete(p) }`：每张照片都新建一次
//! `IFileOperation` 并提交一次 Shell 操作（实测 ≈80ms/张）。而 trash 库自带
//! `delete_all`，一次 IFileOperation 就能提交整批 —— 这是数量级差异的来源。
//!
//! 同时把删除拆成「块」并交给若干 **独立 OS 线程**：
//! 1. 块内一次 `delete_all`，块间并行；
//! 2. 每块完成发一条进度事件，前端不再"点了没反应"；
//! 3. 每块用 `catch_unwind` 包住 —— COM 公寓冲突、Shell 异常都只让**这一块**
//!    失败，不会像旧实现那样一张照片 panic 掉整批。
//!
//! ══════════════════════════════════════════════════════════════════════════
//! COM 公寓纪律（0x80010106 的根因，务必照做）
//! ──────────────────────────────────────────────────────────────────────────
//! `trash` 5.x 默认用 `coinit_apartmentthreaded`：它在**调用线程**上
//! `CoInitializeEx(STA)`，如果该线程已被初始化成 MTA，就返回
//! `RPC_E_CHANGED_MODE(0x80010106)` 并直接 `panic!`。
//!
//! 所以这里**绝不在 tokio 阻塞池线程上删文件**（池线程可能被 HEIC/WIC 等
//! 模块初始化成 MTA 并复用），而是每次删除自己开干净线程：新线程没有 COM 状态，
//! trash 的 STA 初始化必然成功。`heic.rs` 那边也同步改成"谁初始化谁反初始化"，
//! 从源头不让池线程带污。
//!
//! 判定成功的口径：**以文件是否还在为准**（`delete_all` 只给整块结论，且
//! `PerformOperations` 被用户/系统中断时仍可能返回成功码）。块内若有残留，
//! 再逐张兜底重试一次，尽量把能删的删掉，并把真删不掉的如实上报。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// 单块照片数：一次 `IFileOperation` 提交多少张
///
/// 太小则 Shell 调用次数回升，太大则单块耗时长、进度粒度粗、失败重试代价高。
/// 64 张在实测里既能压住调用次数，又能让进度条每 64 张动一次。
const CHUNK_SIZE: usize = 64;

/// 并行 worker 上限
///
/// Shell 的回收站操作本身有全局串行化倾向，开太多线程只是排队；
/// 4 路足够把"提交一次等一次"的等待重叠掉。
const MAX_WORKERS: usize = 4;

/// 删除进度（`delete-progress` 事件载荷；数字是**照片张数**）
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct TrashDeleteProgress {
    /// 已处理张数（成功 + 失败）
    pub done: usize,
    /// 总张数
    pub total: usize,
    /// 已成功移入回收站
    pub ok: usize,
    /// 已判定失败
    pub failed: usize,
}

/// 批量删除结果
#[derive(Debug, Clone, Default)]
pub struct TrashDeleteOutcome {
    /// 成功移入回收站张数
    pub deleted: usize,
    /// 真删不掉的路径（文件仍在原处）
    pub failed_paths: Vec<String>,
    /// 实际使用的 worker 数（日志用）
    pub workers: usize,
    /// 实际分块数（日志用）
    pub chunks: usize,
    /// 总耗时（毫秒）
    pub elapsed_ms: u64,
}

/// 批量移入回收站（多线程 + 分块 + 逐块进度回调）
///
/// - `on_progress`：每块完成回调一次（跨线程调用，故要求 `Sync`）
/// - 返回时 `deleted + failed_paths.len() == paths.len()`
pub fn delete_paths_parallel<F>(paths: Vec<String>, on_progress: F) -> TrashDeleteOutcome
where
    F: Fn(TrashDeleteProgress) + Sync,
{
    let t0 = Instant::now();
    let total = paths.len();
    if total == 0 {
        return TrashDeleteOutcome { workers: 0, chunks: 0, elapsed_ms: 0, ..Default::default() };
    }

    let chunks = chunk_paths(&paths);
    let chunk_count = chunks.len();
    let workers = worker_count(chunk_count);

    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let ok = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= chunk_count {
                    break;
                }
                let (chunk_ok, chunk_failed) = delete_chunk(&chunks[i]);
                let done_now = done.fetch_add(chunks[i].len(), Ordering::Relaxed) + chunks[i].len();
                let ok_now = ok.fetch_add(chunk_ok, Ordering::Relaxed) + chunk_ok;
                let failed_now =
                    failed.fetch_add(chunk_failed.len(), Ordering::Relaxed) + chunk_failed.len();
                if !chunk_failed.is_empty() {
                    if let Ok(mut f) = failures.lock() {
                        f.extend(chunk_failed);
                    }
                }
                on_progress(TrashDeleteProgress {
                    done: done_now.min(total),
                    total,
                    ok: ok_now,
                    failed: failed_now,
                });
            });
        }
    });

    let failed_paths = failures.into_inner().unwrap_or_default();
    TrashDeleteOutcome {
        deleted: total.saturating_sub(failed_paths.len()),
        failed_paths,
        workers,
        chunks: chunk_count,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    }
}

/// 按 `CHUNK_SIZE` 切块（纯函数，便于单测）
fn chunk_paths(paths: &[String]) -> Vec<Vec<String>> {
    paths.chunks(CHUNK_SIZE).map(|c| c.to_vec()).collect()
}

/// worker 数：核数上限内、且不超过块数
fn worker_count(chunk_count: usize) -> usize {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    cores.clamp(1, MAX_WORKERS).min(chunk_count.max(1))
}

/// 删一块：先批量、再兜底逐张，最后按「文件是否还在」结算
fn delete_chunk(chunk: &[String]) -> (usize, Vec<String>) {
    // 1. 一次 IFileOperation 提交整块。panic 只影响本块（com 冲突 / shell 异常）
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let items: Vec<PathBuf> = chunk.iter().map(PathBuf::from).collect();
        trash::delete_all(items)
    }));

    // 2. 结算：还在 = 没删掉。批量接口只给整块结论，这里逐张核对
    let mut failed: Vec<String> = chunk.iter().filter(|p| Path::new(p).exists()).cloned().collect();

    // 3. 兜底：批量失败/部分失败时逐张重试（不存在的路径会保持失败，如实上报）
    if !failed.is_empty() {
        failed = failed.into_iter().filter(|p| !delete_one(p)).collect();
    }

    (chunk.len().saturating_sub(failed.len()), failed)
}

/// 单张兜底删除（同样 panic 隔离）；返回是否已不在原处
fn delete_one(path: &str) -> bool {
    let p = Path::new(path);
    let _ = catch_unwind(AssertUnwindSafe(|| trash::delete(p)));
    !p.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("C:\\p{i}.jpg")).collect()
    }

    /// 切块：整块 + 尾块都要覆盖到，且不丢不重
    #[test]
    fn chunk_paths_covers_all_without_loss() {
        let p = paths(150);
        let chunks = chunk_paths(&p);
        assert_eq!(chunks.len(), 3, "150 张按 64 切应得 3 块");
        assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), 150);
        assert_eq!(chunks[2].len(), 22, "尾块应是剩余 22 张");
    }

    /// worker 数不超过块数，也不会是 0
    #[test]
    fn worker_count_never_exceeds_chunks() {
        assert_eq!(worker_count(0), 1);
        assert_eq!(worker_count(1), 1);
        assert!(worker_count(2) <= 2);
        assert!(worker_count(100) <= MAX_WORKERS);
    }

    /// 空输入不启动线程，返回零结果
    #[test]
    fn empty_input_is_noop() {
        let r = delete_paths_parallel(Vec::new(), |_| {});
        assert_eq!(r.deleted, 0);
        assert_eq!(r.chunks, 0);
        assert!(r.failed_paths.is_empty());
    }
}
