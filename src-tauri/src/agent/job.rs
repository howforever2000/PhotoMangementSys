//! Agent job 状态机：**前端看到的快照唯一真相**。
//!
//! 为什么快照放在 Rust 而不是 Python（docs/v5.md §6）：
//! Python 进程随时可能被杀（用户杀毒、崩溃、我们自己的 ensure 重启）。
//! 若快照跟着 Python 走，那种时候前端只会看到"永远 running"。
//! 放在 Rust，最坏情况下也能告诉用户"中断在第 3 步，已花 ¥0.02"。
//!
//! 本模块只做状态收敛与合并，不做网络、不碰数据库 —— 成本落库在 `ledger.rs`，
//! 轮询在 `mod.rs`。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

// ---- 状态常量（与 Python 侧 jobs.py 的 TERMINAL 集合保持一致） ----
//
// 有几个常量当前只被终态判定/测试引用（done/budget/handoff 的**写入方**在后续块：
// 预算软闸与 P2 二次装配）。先集中定义成一处，避免后续满代码库拼字面量。
#[allow(dead_code)]
pub const ST_DONE: &str = "done";
#[allow(dead_code)]
pub const ST_BUDGET: &str = "budget";
#[allow(dead_code)]
pub const ST_HANDOFF: &str = "handoff";

/// Rust 侧建 job 后、Python 尚未受理
pub const ST_STARTING: &str = "starting";
pub const ST_RUNNING: &str = "running";
pub const ST_ERROR: &str = "error";
pub const ST_CANCELLED: &str = "cancelled";

/// 终态判定：非终态只有 starting/running。
pub fn is_terminal(status: &str) -> bool {
    !matches!(status, ST_STARTING | ST_RUNNING)
}

/// 一步轨迹
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentStep {
    pub idx: i64,
    pub tool: String,
    pub model: Option<String>,
    pub latency_ms: i64,
    pub cost: f64,
    pub thumb: Option<String>,
    pub note: Option<String>,
}

/// Python `GET /agent/jobs/{id}` 的返回体
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PythonStatus {
    pub status: String,
    pub p1_path: Option<String>,
    pub steps: Vec<AgentStep>,
    pub cost_total: f64,
    pub calls_used: i64,
    pub need_replan: Option<serde_json::Value>,
    pub error: Option<String>,
    // ---- 编译器/执行器产出（块 B 新增） ----
    /// 目标提示词（local 引擎下为 null）
    pub prompt: Option<String>,
    pub prompt_source: Option<String>,
    /// 假设与降级记录（"描述已映射为 strong"、"已剔除禁用词"…）
    pub assumptions: Vec<String>,
    /// 冲突裁决（"已按你说的不出 PDF"）
    pub conflicts: Vec<String>,
    /// P1a 未命中时的行内提示文案
    pub hint: String,
    /// 执行参数预览（高级区显示；local 引擎不显示提示词）
    pub preview: Option<serde_json::Value>,
    /// 产物绝对路径
    pub outputs: Vec<String>,
}

/// 前端看到的 job 快照
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSnapshot {
    pub job_id: i64,
    pub recipe_id: String,
    pub status: String,
    pub p1_path: Option<String>,
    pub steps: Vec<AgentStep>,
    pub cost_total: f64,
    pub calls_used: i64,
    pub need_replan: Option<serde_json::Value>,
    pub error: Option<String>,
    /// 该 job 是否由 Python 侧确认过（false = 还在等受理/已失联）
    pub python_seen: bool,
    pub started_at: i64,
    pub updated_at: i64,
    // ---- 编译器/执行器产出 ----
    pub prompt: Option<String>,
    pub prompt_source: Option<String>,
    pub assumptions: Vec<String>,
    pub conflicts: Vec<String>,
    pub hint: String,
    pub preview: Option<serde_json::Value>,
    pub outputs: Vec<String>,
}

impl AgentSnapshot {
    fn new(job_id: i64, recipe_id: String) -> Self {
        let now = crate::now();
        Self {
            job_id,
            recipe_id,
            status: ST_STARTING.to_string(),
            p1_path: None,
            steps: Vec::new(),
            cost_total: 0.0,
            calls_used: 0,
            need_replan: None,
            error: None,
            python_seen: false,
            started_at: now,
            updated_at: now,
            prompt: None,
            prompt_source: None,
            assumptions: Vec::new(),
            conflicts: Vec::new(),
            hint: String::new(),
            preview: None,
            outputs: Vec::new(),
        }
    }
}

struct Entry {
    snap: AgentSnapshot,
    cancel: Arc<AtomicBool>,
}

/// 进程级 job 表。
#[derive(Default)]
pub struct AgentJobs {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    next_id: i64,
    jobs: HashMap<i64, Entry>,
}

impl AgentJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// 建 job 并返回 (job_id, 取消标记)。
    pub fn begin(&self, recipe_id: &str) -> (i64, Arc<AtomicBool>) {
        let mut g = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(), // 被 poison 也不能让任务系统停摆
        };
        g.next_id += 1;
        let job_id = g.next_id;
        let cancel = Arc::new(AtomicBool::new(false));
        g.jobs.insert(
            job_id,
            Entry {
                snap: AgentSnapshot::new(job_id, recipe_id.to_string()),
                cancel: cancel.clone(),
            },
        );
        (job_id, cancel)
    }

    /// Python 已受理：把 starting 推成 running（否则前端在首个轮询回来前看到的是“启动中”）。
    pub fn mark_running(&self, job_id: i64) -> Option<AgentSnapshot> {
        let mut g = self.inner.lock().ok()?;
        let e = g.jobs.get_mut(&job_id)?;
        if e.snap.status == ST_STARTING {
            e.snap.status = ST_RUNNING.to_string();
            e.snap.updated_at = crate::now();
        }
        Some(e.snap.clone())
    }

    pub fn snapshot(&self, job_id: i64) -> Option<AgentSnapshot> {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.jobs.get(&job_id).map(|e| e.snap.clone()))
    }

    /// 全部 job（按 job_id 升序），前端启动时靠它恢复面板状态。
    pub fn all(&self) -> Vec<AgentSnapshot> {
        let mut v: Vec<_> = self
            .inner
            .lock()
            .map(|g| g.jobs.values().map(|e| e.snap.clone()).collect())
            .unwrap_or_default();
        v.sort_by_key(|s| s.job_id);
        v
    }

    /// 是否有 job 在跑（单飞约束：同一时刻只允许一个）。
    pub fn any_running(&self) -> Option<i64> {
        self.inner.lock().ok().and_then(|g| {
            g.jobs
                .values()
                .find(|e| !is_terminal(&e.snap.status))
                .map(|e| e.snap.job_id)
        })
    }

    /// 合并 Python 的状态。
    ///
    /// 两条防回退规则：
    /// 1. 本端已收敛为终态（如 SidecarDown）时，不接受"又变回 running"——那是
    ///    迟到的轮询结果，接受它会让前端看到一个永远不会结束的进度；
    /// 2. 状态只前进不后退（done 不会被后面的 running 覆盖）。
    pub fn merge(&self, job_id: i64, st: &PythonStatus) -> Option<AgentSnapshot> {
        let mut g = self.inner.lock().ok()?;
        let e = g.jobs.get_mut(&job_id)?;
        if is_terminal(&e.snap.status) && !is_terminal(&st.status) {
            return Some(e.snap.clone());
        }
        e.snap.status = st.status.clone();
        e.snap.p1_path = st.p1_path.clone();
        e.snap.steps = st.steps.clone();
        e.snap.cost_total = st.cost_total;
        e.snap.calls_used = st.calls_used;
        e.snap.need_replan = st.need_replan.clone();
        e.snap.error = st.error.clone();
        e.snap.prompt = st.prompt.clone();
        e.snap.prompt_source = st.prompt_source.clone();
        e.snap.assumptions = st.assumptions.clone();
        e.snap.conflicts = st.conflicts.clone();
        e.snap.hint = st.hint.clone();
        e.snap.preview = st.preview.clone();
        e.snap.outputs = st.outputs.clone();
        e.snap.python_seen = true;
        e.snap.updated_at = crate::now();
        Some(e.snap.clone())
    }

    /// 本端强制收敛（超时、服务失联、用户取消）。
    ///
    /// 刻意**不**在内部 abort 轮询任务：这个方法会被轮询任务自己调用，
    /// abort 自己会让后面那行 emit 直接不执行（前端就丢了最后一次状态）。
    /// 轮询循环自己带退出条件（取消标记 + 终态判定），不会撞。
    pub fn finish(&self, job_id: i64, status: &str, error: Option<String>) -> Option<AgentSnapshot> {
        let mut g = self.inner.lock().ok()?;
        let e = g.jobs.get_mut(&job_id)?;
        if is_terminal(&e.snap.status) {
            return Some(e.snap.clone());
        }
        e.snap.status = status.to_string();
        if error.is_some() {
            e.snap.error = error;
        }
        e.snap.updated_at = crate::now();
        Some(e.snap.clone())
    }

    /// 置取消标记；返回是否存在该 job。
    pub fn request_cancel(&self, job_id: i64) -> bool {
        if let Ok(g) = self.inner.lock() {
            if let Some(e) = g.jobs.get(&job_id) {
                e.cancel.store(true, Ordering::SeqCst);
                return true;
            }
        }
        false
    }

    /// 取消标记是否已置位（测试与后续 needReplan 流程用；
    /// 轮询循环自己持有 `Arc`，不必绕这里）
    #[allow(dead_code)]
    pub fn is_cancelled(&self, job_id: i64) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.jobs.get(&job_id).map(|e| e.cancel.load(Ordering::SeqCst)))
            .unwrap_or(false)
    }

    /// 清掉超过 `keep` 个的终态 job（内存卫生；运行中的永不淘汰）。
    pub fn prune(&self, keep: usize) {
        if let Ok(mut g) = self.inner.lock() {
            if g.jobs.len() <= keep {
                return;
            }
            let mut finished: Vec<(i64, i64)> = g
                .jobs
                .values()
                .filter(|e| is_terminal(&e.snap.status))
                .map(|e| (e.snap.started_at, e.snap.job_id))
                .collect();
            finished.sort();
            let over = g.jobs.len() - keep;
            for (_, jid) in finished.into_iter().take(over) {
                g.jobs.remove(&jid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(cost: f64) -> PythonStatus {
        PythonStatus {
            status: ST_RUNNING.into(),
            cost_total: cost,
            ..Default::default()
        }
    }

    #[test]
    fn begin_increments_and_is_not_terminal() {
        let jobs = AgentJobs::new();
        let (a, _) = jobs.begin("style.warm_dusk");
        let (b, _) = jobs.begin("card.magazine_3up");
        assert_ne!(a, b);
        assert_eq!(jobs.snapshot(a).unwrap().status, ST_STARTING);
        assert!(!is_terminal(&jobs.snapshot(a).unwrap().status));
        assert!(jobs.any_running().is_some());
    }

    #[test]
    fn merge_carries_cost_and_steps() {
        let jobs = AgentJobs::new();
        let (id, _) = jobs.begin("r");
        let mut st = running(0.12);
        st.steps.push(AgentStep {
            idx: 1,
            tool: "plan".into(),
            ..Default::default()
        });
        st.p1_path = Some("P1a".into());
        let snap = jobs.merge(id, &st).unwrap();
        assert_eq!(snap.status, ST_RUNNING);
        assert_eq!(snap.steps.len(), 1);
        assert!((snap.cost_total - 0.12).abs() < 1e-9);
        assert_eq!(snap.p1_path.as_deref(), Some("P1a"));
        assert!(snap.python_seen);
    }

    #[test]
    fn terminal_state_cannot_be_resurrected_by_late_poll() {
        let jobs = AgentJobs::new();
        let (id, _) = jobs.begin("r");
        jobs.finish(id, ST_ERROR, Some("服务失联".into()));
        // 迟到的轮询说还在 running —— 必须被忽略
        let snap = jobs.merge(id, &running(0.5)).unwrap();
        assert_eq!(snap.status, ST_ERROR, "终态不得被迟到的 running 覆盖");
        assert_eq!(snap.error.as_deref(), Some("服务失联"));
    }

    #[test]
    fn finish_is_idempotent() {
        let jobs = AgentJobs::new();
        let (id, _) = jobs.begin("r");
        jobs.finish(id, ST_DONE, None);
        let snap = jobs.finish(id, ST_ERROR, Some("不该覆盖".into())).unwrap();
        assert_eq!(snap.status, ST_DONE);
        assert!(snap.error.is_none());
    }

    #[test]
    fn cancel_flag_roundtrip() {
        let jobs = AgentJobs::new();
        let (id, flag) = jobs.begin("r");
        assert!(!jobs.is_cancelled(id));
        assert!(jobs.request_cancel(id));
        assert!(flag.load(Ordering::SeqCst));
        assert!(jobs.is_cancelled(id));
        assert!(!jobs.request_cancel(9999), "不存在的 job 返回 false");
    }

    #[test]
    fn prune_keeps_running_and_newest_finished() {
        let jobs = AgentJobs::new();
        let (run_id, _) = jobs.begin("running");
        let mut done = Vec::new();
        for i in 0..5 {
            let (id, _) = jobs.begin(&format!("d{i}"));
            jobs.finish(id, ST_DONE, None);
            done.push(id);
        }
        jobs.prune(3);
        let ids: Vec<i64> = jobs.all().iter().map(|s| s.job_id).collect();
        assert!(ids.contains(&run_id), "运行中的 job 不得被淘汰");
        assert!(ids.len() <= 3, "应淘汰到只剩 3 个，实得 {ids:?}");
        assert!(ids.contains(done.last().unwrap()), "最新的终态 job 应保留");
    }
}
