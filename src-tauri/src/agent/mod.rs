//! Agent 命令层：身份 → 开关 → 单飞 → 提交 → 轮询桥接 → 快照。
//!
//! 分工（docs/v5.md §1 的能力归属表，一个能力只出现一次）：
//! - 登录态 / 开关 / 预算硬闸 / 快照真相 / 台账：**本层（Rust）**
//! - 编排循环 / 图像算法 / 出网：**Python 侧车**
//!
//! 前端只与这五个命令打交道，`invoke` 名字与契约保持稳定 —— 后续接规划层、
//! 接图像算法都不改这里的方法名（只往 submit 体里加字段）。

pub mod client;
pub mod job;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::Deserialize;
use tauri::{Emitter, Manager};

use crate::logger;
use crate::sidecar::{SidecarSpec, SidecarState};
use job::{AgentJobs, AgentSnapshot};

/// 必须与 `python/agent/__init__.py` 的 `API_VERSION` 一致；
/// 不一致时 `ensure` 会把遗留实例杀掉重起（避免新旧契约混跑）。
pub const API_VERSION: u64 = 1;

/// 轮询间隔（docs/v5.md §6：400ms）
const POLL_INTERVAL_MS: u64 = 400;
/// 连续失败多少次判定服务失联
const MAX_POLL_FAILS: u32 = 3;
/// 内存里保留多少条历史 job
const PRUNE_KEEP: usize = 20;

/// 开关的设置键。**默认关**（`agent.enabled=false`）：
/// 关闭时工坊退化为 v0.4.5 的工具台，不残留任何依赖。
pub const KEY_ENABLED: &str = "agent.enabled";

/// 侧车描述。打包后位于 `resource_dir/agent/agent-server.exe`。
pub static AGENT_SPEC: SidecarSpec = SidecarSpec {
    name: "Agent",
    resource_subdir: "agent",
    exe_file: "agent-server.exe",
    dev_entry: "python/agent_server.py",
    instance_file: "agent-instance.json",
    health_path: "/health",
    log_name: "agent-server.log",
    api_version: API_VERSION,
    env: agent_env,
};

fn agent_env(port: u16, secret: &str) -> Vec<(String, String)> {
    vec![
        ("AGENT_PORT".to_string(), port.to_string()),
        ("AGENT_TOKEN".to_string(), secret.to_string()),
        // 子进程 stdout/stderr 走管道，Windows 默认 GBK 会把中文日志写成乱码；
        // 强制 UTF-8 后 Rust 侧 `from_utf8_lossy` 才能读出人话（排障文案靠它）。
        ("PYTHONIOENCODING".to_string(), "utf-8".to_string()),
        ("PYTHONUTF8".to_string(), "1".to_string()),
    ]
}

// ------------------------------------------------------------------ 开关

fn enabled(app: &tauri::AppHandle) -> bool {
    let st = app.state::<crate::AppState>();
    let Ok(db) = st.0.lock() else {
        return false;
    };
    matches!(db.get_setting(KEY_ENABLED), Ok(Some(v)) if v == "1" || v.eq_ignore_ascii_case("true"))
}

#[tauri::command]
pub fn agent_get_enabled(app: tauri::AppHandle) -> bool {
    enabled(&app)
}

#[tauri::command]
pub fn agent_set_enabled(app: tauri::AppHandle, enabled_flag: bool) -> Result<bool, String> {
    let st = app.state::<crate::AppState>();
    let db = st.0.lock().map_err(|e| e.to_string())?;
    db.set_setting(KEY_ENABLED, if enabled_flag { "1" } else { "0" })
        .map_err(|e| format!("保存 Agent 开关失败: {e}"))?;
    logger::log_info(&format!("agent | 开关已设置为 {enabled_flag}"));
    Ok(enabled_flag)
}

// ------------------------------------------------------------------ 命令

/// 确保侧车就绪并返回基址。工坊页进入时后台调用（不阻塞 UI）。
#[tauri::command]
pub async fn agent_ensure(
    app: tauri::AppHandle,
    sc: tauri::State<'_, SidecarState>,
) -> Result<String, String> {
    if !enabled(&app) {
        return Err("Agent 未开启（设置 → 创意工坊 → 启用 AI 创作）".to_string());
    }
    sc.ensure(&app).await
}

/// 提交任务。**一次把执行所需全部输入给全**，Python 执行期不回连 Rust。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn agent_submit(
    app: tauri::AppHandle,
    session: tauri::State<'_, crate::SessionState>,
    jobs: tauri::State<'_, AgentJobs>,
    sc: tauri::State<'_, SidecarState>,
    req: AgentRequest,
) -> Result<AgentSnapshot, String> {
    let uid = crate::require_user(&session)?;
    if !enabled(&app) {
        return Err("Agent 未开启（设置 → 创意工坊 → 启用 AI 创作）".to_string());
    }
    if let Some(running) = jobs.any_running() {
        return Err(format!(
            "已有 Agent 任务在运行（job #{running}），请等它结束或先取消"
        ));
    }
    let started = std::time::Instant::now();

    let base = sc.ensure(&app).await?;
    let token = sc.secret();

    let (job_id, cancel) = jobs.begin(&req.recipe_id);
    let body = serde_json::json!({
        "jobId": job_id,
        "recipeId": req.recipe_id,
        "userMsg": req.user_msg,
        "paths": req.paths,
        "tier": req.tier,
        "outputDir": req.output_dir,
        // 后续块会在这里补：provider(含 per-job 明文凭据) / planner / priceTable /
        // candidates（Rust 装配好的候选池）/ budget。字段只增不改，端点形状不变。
    });

    match client::submit(&base, &token, &body).await {
        Ok(py_job_id) => {
            if py_job_id != job_id {
                logger::log_warn(&format!(
                    "agent | Python 返回的 jobId({py_job_id}) 与本地({job_id}) 不一致"
                ));
            }
            logger::log_info(&format!(
                "agent | submit ok job={job_id} recipe={} uid={uid} {}ms",
                req.recipe_id,
                started.elapsed().as_millis()
            ));
        }
        Err(e) => {
            let snap = jobs.finish(job_id, job::ST_ERROR, Some(e.clone()));
            if let Some(s) = snap {
                emit(&app, &s);
            }
            return Err(e);
        }
    }

    let snap = jobs
        .mark_running(job_id)
        .ok_or_else(|| "内部错误：job 状态丢失".to_string())?;
    emit(&app, &snap);

    let handle = spawn_poller(app.clone(), base, token, job_id, cancel);
    // 句柄不保存：轮询循环自带退出条件（取消标记 / 终态 / 连续失败）。
    // 把它 abort 掉反而会让最后一次状态 emit 丢在取消路径上。
    drop(handle);

    Ok(snap)
}

/// 全部 job 快照（前端重开面板时靠它恢复，切页/关窗不丢）。
#[tauri::command]
pub fn agent_jobs_snapshot(
    session: tauri::State<'_, crate::SessionState>,
    jobs: tauri::State<'_, AgentJobs>,
) -> Result<Vec<AgentSnapshot>, String> {
    crate::require_user(&session)?;
    Ok(jobs.all())
}

/// 取消。**双保险**：① 立刻把本地快照收敛为 cancelled（UI 300ms 内可见）；
/// ② 置取消标记 + POST 通知 Python；③ 轮询循环看到标记也会自己退出。
#[tauri::command]
pub async fn agent_cancel(
    app: tauri::AppHandle,
    session: tauri::State<'_, crate::SessionState>,
    jobs: tauri::State<'_, AgentJobs>,
    sc: tauri::State<'_, SidecarState>,
    job_id: i64,
) -> Result<(), String> {
    crate::require_user(&session)?;
    if !jobs.request_cancel(job_id) {
        return Err(format!("job #{job_id} 不存在"));
    }
    if let Some(snap) = jobs.finish(job_id, job::ST_CANCELLED, Some("已取消".to_string())) {
        emit(&app, &snap);
    }
    if let (Some(base), token) = (sc.base(), sc.secret()) {
        if let Err(e) = client::cancel(&base, &token, job_id).await {
            // 取消请求失败不影响本地已收敛的状态，只留痕
            logger::log_warn(&format!("agent | 通知 Python 取消失败（本地已取消）：{e}"));
        }
    }
    logger::log_info(&format!("agent | job {job_id} 已取消"));
    Ok(())
}

// ------------------------------------------------------------------ 轮询

fn spawn_poller(
    app: tauri::AppHandle,
    base: String,
    token: String,
    job_id: i64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut fails = 0u32;
        loop {
            // ① 用户取消（本地已收敛，这里只需要停）
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            // ② 本端已收敛为终态（可能是取消/失联），无需再问
            let jobs = app.state::<AgentJobs>();
            if let Some(s) = jobs.snapshot(job_id) {
                if job::is_terminal(&s.status) {
                    break;
                }
            }

            match client::status(&base, &token, job_id).await {
                Ok(v) => {
                    fails = 0;
                    let parsed: job::PythonStatus = match serde_json::from_value(v) {
                        Ok(p) => p,
                        Err(e) => {
                            let s = jobs.finish(
                                job_id,
                                job::ST_ERROR,
                                Some(format!("Agent 返回体不符合契约：{e}")),
                            );
                            if let Some(s) = s {
                                emit(&app, &s);
                            }
                            break;
                        }
                    };
                    if let Some(snap) = jobs.merge(job_id, &parsed) {
                        emit(&app, &snap);
                        if job::is_terminal(&snap.status) {
                            logger::log_info(&format!(
                                "agent | job {job_id} 终态={} 成本={:.4} 步数={}",
                                snap.status,
                                snap.cost_total,
                                snap.steps.len()
                            ));
                            break;
                        }
                    } else {
                        break; // job 被 prune 掉了
                    }
                }
                Err(client::StatusErr::NotFound) => {
                    // Python 重启过：task 已蒸发。已花的钱在逐步合并时已经进快照了。
                    let s = jobs.finish(
                        job_id,
                        job::ST_ERROR,
                        Some("Agent 服务重启过，任务已中断（已产生的成本已记录）".to_string()),
                    );
                    if let Some(s) = s {
                        emit(&app, &s);
                    }
                    logger::log_warn(&format!("agent | job {job_id} 丢失（Python 侧 404）"));
                    break;
                }
                Err(e) => {
                    fails += 1;
                    logger::log_warn(&format!(
                        "agent | 轮询失败 {fails}/{MAX_POLL_FAILS}: {e}"
                    ));
                    if fails >= MAX_POLL_FAILS {
                        let s = jobs.finish(
                            job_id,
                            job::ST_ERROR,
                            Some(format!("Agent 服务无响应，任务已中断：{e}")),
                        );
                        if let Some(s) = s {
                            emit(&app, &s);
                        }
                        break;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
        }
        app.state::<AgentJobs>().prune(PRUNE_KEEP);
    })
}

fn emit(app: &tauri::AppHandle, snap: &AgentSnapshot) {
    if let Err(e) = app.emit("agent-progress", snap) {
        // 窗口已关（后台运行）或事件系统不可用：不是错误，但值得留痕
        logger::log_warn(&format!("agent | 进度事件发送失败（窗口可能已关闭）：{e}"));
    }
}

// ------------------------------------------------------------------ 请求体

/// 前端提交的请求。字段只增不改；`serde(default)` 让旧前端不会因为新增字段 500。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentRequest {
    /// 配方 id（如 `style.warm_dusk`）
    pub recipe_id: String,
    /// 用户附加描述（可为空 = P0 纯预设路径）
    pub user_msg: String,
    /// PhotoTray 手选的图片路径（手选永远优先置顶）
    pub paths: Vec<String>,
    /// 档位：draft / standard / hq / local
    pub tier: String,
    /// 产物目录（空则用 app_data_dir/agent-out）
    pub output_dir: Option<String>,
}

impl Default for AgentRequest {
    fn default() -> Self {
        Self {
            recipe_id: String::new(),
            user_msg: String::new(),
            paths: Vec::new(),
            tier: "local".to_string(),
            output_dir: None,
        }
    }
}

// ------------------------------------------------------------------ 跨进程契约测试

/// Rust 客户端 ↔ Python 侧车的真实往返。
///
/// 为什么值得跑真的进程：`PythonStatus` 靠 `rename_all = "camelCase"` 反序列化，
/// 字段名一对不上**不会报错**，只会静默变成 None/0（“进度永远空”）。
/// 单元测试 mock 不出来这种错位，只有真实响应能。
///
/// 找不到 Python 或缺依赖时**跳过**（不把环境问题当成代码失败）。
#[cfg(test)]
mod contract_tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    fn candidate_interpreters() -> Vec<PathBuf> {
        let root = crate::devpath::dev_project_dir();
        let mut v = Vec::new();
        let venv = root
            .join("python")
            .join(".venv-agent")
            .join("Scripts")
            .join("python.exe");
        if venv.is_file() {
            v.push(venv);
        }
        for name in ["python", "python3"] {
            v.push(PathBuf::from(name));
        }
        v
    }

    fn has_deps(py: &Path) -> bool {
        Command::new(py)
            .args(["-c", "import fastapi, uvicorn, httpx"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn kill_tree(pid: u32) {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .creation_flags(0x0800_0000)
                .output();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
        }
    }

    #[tokio::test]
    async fn client_roundtrip_against_real_python() {
        let root = crate::devpath::dev_project_dir();
        let script = root.join("python").join("agent_server.py");
        if !script.is_file() {
            eprintln!("[skip] 找不到 python/agent_server.py（非源码布局？）");
            return;
        }
        let Some(py) = candidate_interpreters().into_iter().find(|p| has_deps(p)) else {
            eprintln!(
                "[skip] 没有可用的 Python 依赖。先执行：\n  \
                 python -m pip install -r python/requirements-agent.txt"
            );
            return;
        };

        let port = crate::sidecar::pick_free_port();
        let token = "contract-test-token";
        let mut child = Command::new(&py)
            .arg(&script)
            .env("AGENT_PORT", port.to_string())
            .env("AGENT_TOKEN", token)
            .env("PYTHONIOENCODING", "utf-8")
            .current_dir(root.join("python"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("起 python 侧车失败");
        let pid = child.id();
        let base = format!("http://127.0.0.1:{port}");

        let result = async {
            let http = reqwest::Client::new();
            let mut up = false;
            for _ in 0..80 {
                tokio::time::sleep(Duration::from_millis(150)).await;
                if let Ok(r) = http.get(format!("{base}/health")).send().await {
                    if r.status().is_success() {
                        up = true;
                        break;
                    }
                }
            }
            assert!(up, "/health 未就绪（python={py:?}）");

            // ① submit → jobId
            let body = serde_json::json!({"jobId": 1, "recipeId": "style.warm_dusk"});
            let id = client::submit(&base, token, &body).await.expect("submit 失败");
            assert_eq!(id, 1);

            // ② 轮询到终态，并验证反序列化（camelCase 契约）
            let mut last: Option<job::PythonStatus> = None;
            for _ in 0..80 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                match client::status(&base, token, id).await {
                    Ok(v) => {
                        let st: job::PythonStatus =
                            serde_json::from_value(v).expect("返回体不符合 PythonStatus 契约");
                        if job::is_terminal(&st.status) {
                            last = Some(st);
                            break;
                        }
                    }
                    Err(e) => panic!("状态查询失败：{e}"),
                }
            }
            let st = last.expect("job 未收敛");
            assert_eq!(st.status, job::ST_DONE);
            assert_eq!(st.steps.len(), 3, "骨架应有三步");
            assert_eq!(st.steps[0].tool, "plan");
            assert_eq!(st.steps[0].idx, 1);

            // ③ 两种失败语义必须区分开（收养判定与用户文案都靠它）
            assert!(matches!(
                client::status(&base, token, 999_999).await,
                Err(client::StatusErr::NotFound)
            ));
            assert!(matches!(
                client::status(&base, "wrong-token", 999_999).await,
                Err(client::StatusErr::Unauthorized(401))
            ));
        }
        .await;

        kill_tree(pid);
        let _ = child.wait();
        result
    }
}
