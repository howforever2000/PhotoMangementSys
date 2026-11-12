//! 创意工坊（FEAT-150）· 图像压缩后台任务状态机
//!
//! 背景：原 `image_compress`（`tools.rs`）是**阻塞单命令**——前端 `await invoke`
//! 期间组件一卸载（切到别的页面），返回无处可收，进度、结果全丢；且无法中途停止。
//!
//! 本模块把压缩任务状态**提升到后端进程**（`tauri::State`）：命令改为「启动即返回」，
//! 前端任意时刻（含重新进入创意工坊）通过 `get_compress_job` 拉快照恢复完整视图；
//! `cancel_compress_job` 置取消标记，任务在下一张开始前提前结束，**已产出的部分结果
//! 保留在快照里**。
//!
//! ## 分层（CS1）
//!
//! - **状态机**（本文件）：只管「谁在跑 / 跑到哪 / 结果」，不关心压缩算法本身。
//! - **服务层**（复用 `tools::compress_one` / `tools::parse_aspect`）：单张解码→裁切→
//!   缩放→编码→落盘。
//! - **命令层**（本文件 `start_/get_/cancel_/clear_compress_job`）：鉴权、入口日志、
//!   spawn_blocking。
//!
//! 与 `test_scan_job::ScanJobState` 同构（同为进程级、`Arc<Mutex>`+`AtomicBool`+代次号），
//! 但刻意独立：压缩是纯本地 IO/CPU，不碰数据库、不并发多线程，语义比扫描简单得多。
//!
//! ## 取消粒度
//!
//! 取消只在**单张之间**生效：`compress_one` 内部（一次解码+阶梯编码）不可中断，
//! 但这已足够——单张通常 < 1s，用户能看到「停止后很快停在已完成的张数上」。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::tools::{compress_one, parse_aspect, CompressFailure, CompressOptions, CompressedItem};

/// 任务运行状态（前端据此决定进度条 / 按钮）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompressJobStatus {
    /// 空闲（从未运行 / 已清理）
    #[default]
    Idle,
    /// 运行中
    Running,
    /// 已完成
    Done,
    /// 失败（原因见 `error`）
    Failed,
    /// 用户停止后结束（`items` 为已完成部分）
    Cancelled,
}

/// 状态快照（`get_compress_job` 返回值；前端渲染的全部依据）
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompressJobSnapshot {
    pub status: CompressJobStatus,
    /// 输出目录
    pub dest_dir: String,
    /// 本次待压缩总张数
    pub total: usize,
    /// 已完成张数（= items.len()）
    pub done: usize,
    /// 已完成的产物（取消时保留部分）
    pub items: Vec<CompressedItem>,
    /// 失败项
    pub failed: Vec<CompressFailure>,
    pub total_before: u64,
    pub total_after: u64,
    /// 正在处理的源文件（用于「正在压缩：xxx」文案）
    pub current: String,
    pub error: String,
    /// 任务开始 / 结束（Unix 秒；0 = 未开始/未结束）
    pub started_at: i64,
    pub finished_at: i64,
    /// 是否被用户中途停止
    pub cancelled: bool,
}

/// 内部可变状态（`Mutex` 保护；任务体每张完成都更新一次）
#[derive(Debug, Default)]
pub struct CompressJobInner {
    pub snapshot: CompressJobSnapshot,
    /// 取消标记：任务体每张处理前检查
    pub cancel: Arc<AtomicBool>,
    /// 自增代次号：防止旧任务的收尾覆盖新任务状态
    pub job_id: u64,
}

/// 后台任务状态（`tauri::State` 托管；生命周期与进程一致）
#[derive(Default, Clone)]
pub struct CompressJobState(pub Arc<Mutex<CompressJobInner>>);

impl CompressJobState {
    /// 可 move 进 `'static` 任务闭包的句柄
    pub fn handle(&self) -> CompressJobState {
        self.clone()
    }

    /// 读取快照（克隆，避免持锁跨命令返回）
    pub fn snapshot(&self) -> CompressJobSnapshot {
        self.0.lock().map(|g| g.snapshot.clone()).unwrap_or_default()
    }

    /// 启动新任务前的准入检查与状态重置
    ///
    /// 返回 `(job_id, cancel_flag)`；已有任务运行中 → Err（同一时刻只允许一个）。
    pub fn begin(
        &self,
        dest_dir: &str,
        total: usize,
    ) -> Result<(u64, Arc<AtomicBool>), String> {
        let mut g = self.0.lock().map_err(|e| format!("任务状态锁失败: {e}"))?;
        if g.snapshot.status == CompressJobStatus::Running {
            return Err("已有压缩任务进行中，请等待完成或点击「停止」后再开始".into());
        }
        g.job_id += 1;
        g.snapshot = CompressJobSnapshot {
            status: CompressJobStatus::Running,
            dest_dir: dest_dir.to_string(),
            total,
            started_at: now_secs(),
            ..Default::default()
        };
        g.cancel = Arc::new(AtomicBool::new(false));
        Ok((g.job_id, g.cancel.clone()))
    }

    /// 记录当前正在处理的源文件
    pub fn set_current(&self, job_id: u64, src: &str) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            g.snapshot.current = src.to_string();
        }
    }

    /// 追加一张成功产物（累加统计；快照里即是「部分结果」）
    pub fn push_item(&self, job_id: u64, item: CompressedItem) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            g.snapshot.total_before += item.before;
            g.snapshot.total_after += item.after;
            g.snapshot.items.push(item);
            g.snapshot.done = g.snapshot.items.len();
        }
    }

    /// 追加一项失败
    pub fn push_failed(&self, job_id: u64, failure: CompressFailure) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            g.snapshot.failed.push(failure);
        }
    }

    /// 任务成功收尾
    pub fn finish_ok(&self, job_id: u64) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            g.snapshot.status = CompressJobStatus::Done;
            g.snapshot.done = g.snapshot.items.len();
            g.snapshot.current.clear();
            g.snapshot.finished_at = now_secs();
        }
    }

    /// 任务失败收尾
    pub fn finish_err(&self, job_id: u64, err: &str) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            g.snapshot.status = CompressJobStatus::Failed;
            g.snapshot.error = err.to_string();
            g.snapshot.finished_at = now_secs();
        }
    }

    /// 任务取消收尾（保留 `items` 部分结果）
    pub fn finish_cancelled(&self, job_id: u64) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            g.snapshot.status = CompressJobStatus::Cancelled;
            g.snapshot.cancelled = true;
            g.snapshot.done = g.snapshot.items.len();
            g.snapshot.current.clear();
            g.snapshot.finished_at = now_secs();
        }
    }

    /// 请求取消（前端「停止」按钮）
    pub fn cancel(&self) -> bool {
        match self.0.lock() {
            Ok(g) => {
                if g.snapshot.status != CompressJobStatus::Running {
                    return false;
                }
                g.cancel.store(true, Ordering::SeqCst);
                true
            }
            Err(_) => false,
        }
    }

    /// 清空记录（仅非运行中允许）
    pub fn clear(&self) -> Result<(), String> {
        let mut g = self.0.lock().map_err(|e| format!("任务状态锁失败: {e}"))?;
        if g.snapshot.status == CompressJobStatus::Running {
            return Err("任务运行中，无法清除记录".into());
        }
        let keep_id = g.job_id;
        g.snapshot = CompressJobSnapshot::default();
        g.job_id = keep_id;
        Ok(())
    }
}

/* ============================ 命令层 ============================ */

/// 启动后台压缩：启动即返回，前端用 `get_compress_job` 轮询快照
#[tauri::command]
pub async fn start_compress_job(
    paths: Vec<String>,
    dest_dir: String,
    opts: CompressOptions,
    state: tauri::State<'_, CompressJobState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<(), String> {
    let _t = log_call!(
        "start_compress_job",
        &format!(
            "n={} dest={dest_dir} target_kb={:?} q={:?}",
            paths.len(),
            opts.target_kb,
            opts.quality
        )
    );
    crate::require_user(&session)?;
    if paths.is_empty() {
        return Err("没有可压缩的照片".into());
    }
    let st = state.handle();
    let (job_id, cancel) = st.begin(&dest_dir, paths.len())?;
    let st2 = st.clone();
    tauri::async_runtime::spawn_blocking(move || {
        run_compress_job(st2, job_id, cancel, paths, dest_dir, opts);
    });
    Ok(())
}

/// 读取压缩任务快照
#[tauri::command]
pub fn get_compress_job(state: tauri::State<'_, CompressJobState>) -> CompressJobSnapshot {
    state.snapshot()
}

/// 请求停止当前压缩任务
#[tauri::command]
pub fn cancel_compress_job(state: tauri::State<'_, CompressJobState>) -> Result<(), String> {
    if state.cancel() {
        crate::logger::log_info("compress_job | 收到停止请求，已置取消标记");
        Ok(())
    } else {
        Err("当前没有运行中的压缩任务".into())
    }
}

/// 清空任务记录（非运行中）
#[tauri::command]
pub fn clear_compress_job(state: tauri::State<'_, CompressJobState>) -> Result<(), String> {
    state.clear()
}

/* ============================ 任务体 ============================ */

fn run_compress_job(
    st: CompressJobState,
    job_id: u64,
    cancel: Arc<AtomicBool>,
    paths: Vec<String>,
    dest_dir: String,
    opts: CompressOptions,
) {
    let dest = PathBuf::from(&dest_dir);
    if let Err(e) = std::fs::create_dir_all(&dest) {
        st.finish_err(job_id, &format!("创建输出目录失败: {e}"));
        return;
    }
    let aspect = match opts.crop_aspect.as_deref().map(str::trim) {
        None | Some("") | Some("none") => None,
        Some(s) => match parse_aspect(s) {
            Some(v) => Some(v),
            None => {
                st.finish_err(job_id, &format!("裁切比例无法识别: {s}"));
                return;
            }
        },
    };

    let mut used: HashSet<String> = HashSet::new();
    for src in &paths {
        if cancel.load(Ordering::SeqCst) {
            crate::logger::log_info(&format!(
                "[compress_job] 用户停止，已完成 {}/{}",
                st.snapshot().done,
                paths.len()
            ));
            st.finish_cancelled(job_id);
            return;
        }
        st.set_current(job_id, src);
        match compress_one(Path::new(src), &dest, &opts, aspect, &mut used) {
            Ok(item) => st.push_item(job_id, item),
            Err(reason) => {
                crate::logger::log_warn(&format!("[compress_job] 压缩失败 {src}: {reason}"));
                st.push_failed(
                    job_id,
                    CompressFailure {
                        src: src.clone(),
                        reason,
                    },
                );
            }
        }
    }
    let snap = st.snapshot();
    crate::logger::log_info(&format!(
        "[compress_job] 完成：成功 {} / 失败 {}，{} KB → {} KB",
        snap.items.len(),
        snap.failed.len(),
        snap.total_before / 1024,
        snap.total_after / 1024
    ));
    st.finish_ok(job_id);
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
