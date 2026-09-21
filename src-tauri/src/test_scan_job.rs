//! 相册扫描分组工具：后台任务状态机（BUG-2026-0921-006）
//!
//! 问题背景：原实现把「扫描 / 解析地名 / 组织移动」做成**同步命令**（前端 await
//! invoke），任务状态只活在前端页面组件里。用户切到别的页面（组件卸载）或中途
//! 退出，任务就「消失」了 —— 实际后端线程可能还在跑，但界面只剩空态，进度、
//! 结果全部丢掉；同时一页 3 个按钮彼此阻塞，扫描期间不能做任何其他操作。
//!
//! 本模块把任务状态**提升到后端进程**（`tauri::State`），命令改为「启动即返回」，
//! 前端任何时刻（含重新进入页面）通过 `get_scan_job` 拉取快照即可恢复完整视图。
//!
//! 与既有实现的解耦：
//!   - 任务体复用 `test_scan::commands::*_job`，本模块只管「谁在跑 / 跑到哪 / 进度」
//!   - 不落库、不碰相册管理（沿用「扫描测试工具」定位）
//!   - 进度同时通过 `test-scan-progress` 事件推送（实时），并留存快照（恢复）
//!
//! 并发模型：同一时刻只允许**一个**任务（扫描 / 解析 / 移动共用一个槽位）。
//! 三者操作同一批文件，并发执行会互相踩；互斥同时在 UI 层禁用按钮。

use std::sync::{Arc, Mutex};

use serde::Serialize;

/// 任务阶段（前端据此决定按钮文案 / 进度条标题）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobPhase {
    /// 扫描：提取拍摄时间 + GPS
    Scan,
    /// 解析地名：GPS 聚类 → 本地省/市（未命中才联网）
    Resolve,
    /// 组织移动：按年 / 地点建目录并移动
    Organize,
}

impl JobPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            JobPhase::Scan => "scan",
            JobPhase::Resolve => "resolve",
            JobPhase::Organize => "organize",
        }
    }
}

/// 任务运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// 空闲（从未运行 / 已清理）
    Idle,
    /// 运行中
    Running,
    /// 已完成
    Done,
    /// 失败（原因见 `error`）
    Failed,
    /// 用户请求停止后结束
    Cancelled,
}

/// 进度快照（前端进度条 + 状态文案）
#[derive(Debug, Clone, Serialize)]
pub struct JobProgress {
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub file_name: String,
    pub message: String,
    /// 每秒处理量（已扫描张数 / 已耗时秒），运行中实时估算
    pub rate: f64,
    /// 预计剩余秒数（total / rate - 已耗时；无法估算为 null）
    pub eta_sec: Option<f64>,
}

/// 组织移动结果（任务完成后回填，供前端展示报告卡片）
#[derive(Debug, Clone, Serialize)]
pub struct OrganizeSummary {
    pub total: usize,
    pub moved: usize,
    pub conflict: usize,
    pub no_time: usize,
    pub no_place: usize,
    pub failed: usize,
    pub target_root: String,
    pub folders: Vec<String>,
    /// 用户中途取消（计数为「已处理部分」的统计）
    #[serde(default)]
    pub cancelled: bool,
}

/// 任务状态快照（`get_scan_job` 返回值；前端渲染的全部依据）
#[derive(Debug, Clone, Serialize)]
pub struct JobSnapshot {
    pub status: JobStatus,
    pub phase: Option<JobPhase>,
    /// 本次任务的目标目录
    pub dir: String,
    pub recursive: bool,
    /// 任务使用的并行度（扫描阶段生效）
    pub threads: usize,
    pub progress: Option<JobProgress>,
    /// scan 阶段结果照片数 / resolve 阶段已解析数（轻量指标，避免整表塞进快照）
    pub photo_count: usize,
    pub place_count: usize,
    /// 组织移动报告（仅 organize 完成时有值）
    pub organize: Option<OrganizeSummary>,
    pub error: String,
    /// 任务开始 / 结束时间（Unix 秒；0 = 未开始/未结束）
    pub started_at: i64,
    pub finished_at: i64,
}

impl Default for JobSnapshot {
    fn default() -> Self {
        Self {
            status: JobStatus::Idle,
            phase: None,
            dir: String::new(),
            recursive: false,
            threads: 0,
            progress: None,
            photo_count: 0,
            place_count: 0,
            organize: None,
            error: String::new(),
            started_at: 0,
            finished_at: 0,
        }
    }
}

/// 任务内部可变状态（`Mutex` 保护，多线程扫描时每个完成项都要更新）
#[derive(Debug)]
pub struct JobInner {
    pub snapshot: JobSnapshot,
    /// 取消标记：扫描线程池每张处理前检查
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// 自增代次号：防止旧任务的收尾覆盖新任务状态
    pub job_id: u64,
    /// 任务开始时刻（用于速率 / ETA 估算）
    pub start_instant: Option<std::time::Instant>,
}

/// 后台任务状态（`tauri::State` 托管）
///
/// 生命周期与进程一致：页面组件卸载、路由切换都不影响 —— 这就是
/// 「退出后任务不消失」的实现基础。
///
/// 内部用 `Arc<Mutex<..>>` 而非裸 `Mutex`：任务体跑在 `spawn_blocking` 线程上，
/// 需要一份可 move 进 `'static` 闭包的句柄（`tauri::State` 本身借用生命周期，
/// 不能被 move）。克隆 `ScanJobState` 只是克隆一个 Arc，代价可忽略。
#[derive(Default, Clone)]
pub struct ScanJobState(pub Arc<Mutex<JobInner>>);

impl Default for JobInner {
    fn default() -> Self {
        Self {
            snapshot: JobSnapshot::default(),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            job_id: 0,
            start_instant: None,
        }
    }
}

/// 为 `tauri::State<ScanJobState>` 提供「克隆成 owned 句柄」的能力
impl ScanJobState {
    /// 可 move 进 `'static` 任务闭包的句柄
    pub fn handle(&self) -> ScanJobState {
        self.clone()
    }
}

impl ScanJobState {
    /// 读取快照（克隆，避免持锁跨命令返回）
    pub fn snapshot(&self) -> JobSnapshot {
        self.0
            .lock()
            .map(|g| g.snapshot.clone())
            .unwrap_or_default()
    }

    /// 当前是否有任务在跑
    #[allow(dead_code)] // 预留：与前端「禁用按钮」逻辑同源的后端查询入口
    pub fn is_running(&self) -> bool {
        self.0
            .lock()
            .map(|g| g.snapshot.status == JobStatus::Running)
            .unwrap_or(false)
    }

    /// 启动新任务前的准入检查与状态重置
    ///
    /// 返回 `(job_id, cancel_flag)`：`job_id` 供收尾时校验「我是否仍是最新任务」，
    /// `cancel` 交给任务体轮询。已有任务运行中 → 返回 Err（同一时刻只允许一个）。
    pub fn begin(
        &self,
        phase: JobPhase,
        dir: &str,
        recursive: bool,
        threads: usize,
    ) -> Result<(u64, Arc<std::sync::atomic::AtomicBool>), String> {
        let mut g = self.0.lock().map_err(|e| format!("任务状态锁失败: {e}"))?;
        if g.snapshot.status == JobStatus::Running {
            return Err(format!(
                "已有任务进行中（{}），请等待完成或点击「停止」后再开始",
                g.snapshot.phase.map(|p| p.as_str()).unwrap_or("未知")
            ));
        }
        g.job_id += 1;
        g.snapshot = JobSnapshot {
            status: JobStatus::Running,
            phase: Some(phase),
            dir: dir.to_string(),
            recursive,
            threads,
            started_at: now_secs(),
            ..Default::default()
        };
        g.cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        g.start_instant = Some(std::time::Instant::now());
        Ok((g.job_id, g.cancel.clone()))
    }

    /// 校验任务代次（旧任务的收尾不应覆盖新任务）
    #[allow(dead_code)] // 预留：多任务队列化后需要按代次丢弃过期收尾
    pub fn is_current(&self, job_id: u64) -> bool {
        self.0.lock().map(|g| g.job_id == job_id).unwrap_or(false)
    }

    /// 更新进度（扫描 / 解析 / 移动共用；按 current 单调递增以免并行乱序回退）
    pub fn set_progress(&self, job_id: u64, p: JobProgress) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            // 并行扫描时完成顺序不确定 → 进度取「已完成数」而非到达顺序，
            // 用 max 保证进度条不回退
            let cur = g.snapshot.progress.as_ref().map(|x| x.current).unwrap_or(0);
            let mut p = p;
            if p.current < cur {
                p.current = cur;
            }
            g.snapshot.progress = Some(p);
        }
    }

    /// 记录已完成张数（并行扫描：每张完成即 +1，由任务体累加后调用）
    pub fn mark_done(
        &self,
        job_id: u64,
        phase: JobPhase,
        current: usize,
        total: usize,
        file_name: &str,
        message: &str,
        elapsed_secs: f64,
    ) {
        if elapsed_secs <= 0.0 {
            return;
        }
        let rate = current as f64 / elapsed_secs;
        let eta = if rate > 0.0 && total > current {
            Some((total - current) as f64 / rate)
        } else if current >= total {
            Some(0.0)
        } else {
            None
        };
        self.set_progress(
            job_id,
            JobProgress {
                phase: phase.as_str().into(),
                current,
                total,
                file_name: file_name.to_string(),
                message: message.to_string(),
                rate,
                eta_sec: eta,
            },
        );
    }

    /// 任务成功收尾（保留结果供前端展示；`status` 置 done）
    pub fn finish_ok<T: FnOnce(&mut JobSnapshot)>(&self, job_id: u64, apply: T) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            g.snapshot.status = JobStatus::Done;
            g.snapshot.finished_at = now_secs();
            apply(&mut g.snapshot);
        }
    }

    /// 扫描任务收尾：写入照片统计 + 把进度条补满（避免节流吞掉最后一帧）
    pub fn finish_scan_ok(&self, job_id: u64, total: usize, with_gps: usize, with_place: usize) {
        self.finish_ok(job_id, |s| {
            s.photo_count = total;
            s.place_count = with_place;
            let prev = s.progress.as_ref();
            s.progress = Some(JobProgress {
                phase: JobPhase::Scan.as_str().into(),
                current: total,
                total,
                file_name: "完成".into(),
                message: format!("扫描完成：{total} 张（GPS {with_gps}）"),
                rate: prev.map(|p| p.rate).unwrap_or(0.0),
                eta_sec: Some(0.0),
            });
        });
    }

    /// 解析地名任务收尾
    pub fn finish_resolve_ok(&self, job_id: u64, total: usize, with_place: usize) {
        self.finish_ok(job_id, |s| {
            s.photo_count = total;
            s.place_count = with_place;
            let prev = s.progress.as_ref();
            s.progress = Some(JobProgress {
                phase: JobPhase::Resolve.as_str().into(),
                current: total,
                total,
                file_name: "完成".into(),
                message: format!("地名解析完成：{with_place}/{total} 张有地点"),
                rate: prev.map(|p| p.rate).unwrap_or(0.0),
                eta_sec: Some(0.0),
            });
        });
    }

    /// 组织移动任务收尾（含报告）
    pub fn finish_organize_ok(&self, job_id: u64, summary: OrganizeSummary) {
        self.finish_ok(job_id, |s| {
            s.photo_count = summary.total;
            s.progress = Some(JobProgress {
                phase: JobPhase::Organize.as_str().into(),
                current: summary.total,
                total: summary.total,
                file_name: "完成".into(),
                message: format!("组织移动完成：已移动 {}", summary.moved),
                rate: 0.0,
                eta_sec: Some(0.0),
            });
            s.organize = Some(summary);
        });
    }

    /// 任务失败收尾
    pub fn finish_err(&self, job_id: u64, err: &str) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            g.snapshot.status = JobStatus::Failed;
            g.snapshot.error = err.to_string();
            g.snapshot.finished_at = now_secs();
        }
    }

    /// 任务取消收尾
    pub fn finish_cancelled(&self, job_id: u64) {
        if let Ok(mut g) = self.0.lock() {
            if g.job_id != job_id {
                return;
            }
            g.snapshot.status = JobStatus::Cancelled;
            g.snapshot.finished_at = now_secs();
        }
    }

    /// 请求取消（前端「停止」按钮）
    pub fn cancel(&self) -> bool {
        match self.0.lock() {
            Ok(g) => {
                if g.snapshot.status != JobStatus::Running {
                    return false;
                }
                g.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                true
            }
            Err(_) => false,
        }
    }

    /// 清空记录（仅非运行中允许；前端「清除记录」）
    pub fn clear(&self) -> Result<(), String> {
        let mut g = self.0.lock().map_err(|e| format!("任务状态锁失败: {e}"))?;
        if g.snapshot.status == JobStatus::Running {
            return Err("任务运行中，无法清除记录".into());
        }
        let keep_id = g.job_id; // 保持代次，避免清理动作本身让旧任务收尾「复活」
        g.snapshot = JobSnapshot::default();
        g.job_id = keep_id;
        g.start_instant = None;
        Ok(())
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
