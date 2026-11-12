//! AI 创作微服务 · 进程管理（FEAT-152）
//!
//! 与 VCR（`vision.rs`）同一范式，但**独立成模块、独立端口、独立版本号**，互不影响：
//!   - 动态端口（绑定 127.0.0.1:0 取系统分配），避免固定端口与本机其它程序冲突；
//!   - `/health` 带 `api_version`，Rust 侧据此判定是否需要重启到新版；
//!   - 单飞锁：并发命令只启动一次，避免多进程抢端口；
//!   - 先起进程后绑端口：启动期端口不可达属正常，探不到就等，不误杀。
//!
//! 开发版：`python -m ai_studio.server`（复用 VCR 的 Python 解释器解析）；
//! 打包版：优先用内置 `resources/ai/ai-server.exe`（见 spawn）；
//! 该 exe 由 python/build_ai_exe.ps1 构建、经 release.tauri.conf.json 的
//! `ai/ai-server.exe` 资源条目随安装包下发。

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::Manager;

/// 协议版本：新增/变更 /jobs 接口时 +1，旧服务会被重启
const AI_API_VERSION: u64 = 1;

/// 进程内缓存的 base（已就绪的服务地址）
static AI_BASE: Mutex<Option<String>> = Mutex::new(None);
/// 本进程拉起的服务 PID（用于下次重启前清场）
static AI_PID: Mutex<Option<u32>> = Mutex::new(None);
/// 单飞锁
static AI_ENSURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn current_base() -> Option<String> {
    AI_BASE.lock().ok().and_then(|g| g.clone())
}

fn set_base(base: &str) {
    if let Ok(mut g) = AI_BASE.lock() {
        *g = Some(base.to_string());
    }
}

fn clear_base() {
    if let Ok(mut g) = AI_BASE.lock() {
        *g = None;
    }
}

fn project_python_dir() -> PathBuf {
    // 收口到 devpath：release 下返回空路径，绝不把构建机绝对路径带进二进制
    // （BUG-2026-1013-001，详见 devpath.rs 模块文档；与 vision.rs 同口径）。
    crate::devpath::dev_project_dir().join("python")
}

fn pick_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("分配 AI 服务端口失败: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("读取 AI 服务端口失败: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

async fn probe(client: &reqwest::Client, base: &str) -> bool {
    match client
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(v) => v.get("api_version").and_then(|x| x.as_u64()).unwrap_or(0) >= AI_API_VERSION,
            Err(_) => false,
        },
        Err(_) => false,
    }
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

fn kill_pid(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F", "/T"])
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

/// PID 是否属于本项目 AI 创作服务（FEAT-153，防 PID 复用误杀用户进程）：
/// 打包版映像名 `ai-server.exe` 唯一；开发版是 python.exe，
/// 需再校验命令行含 `ai_studio`（与 vision.rs 同一口径）。
fn pid_is_our_service(pid: u32) -> bool {
    match crate::vision::pid_image_name(pid) {
        Some(img) if img.eq_ignore_ascii_case("ai-server.exe") => true,
        Some(img) if img.eq_ignore_ascii_case("python.exe") => {
            crate::vision::pid_cmdline_has(pid, "ai_studio")
        }
        _ => false,
    }
}

/// 应用退出时调用（lib.rs 的 RunEvent::Exit，FEAT-153）：
/// 优雅 /shutdown → 验明正身后强杀本进程拉起的 AI 创作服务，不再残留后台。
pub fn shutdown() {
    let pid = AI_PID.lock().ok().and_then(|g| *g);
    let Some(pid) = pid else { return };
    if pid_alive(pid) && pid_is_our_service(pid) {
        // 优雅路径：base 形如 http://127.0.0.1:52341，取端口发 /shutdown
        if let Some(port) = current_base()
            .and_then(|b| b.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()))
        {
            crate::vision::graceful_shutdown(port);
            for _ in 0..10 {
                if !pid_alive(pid) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        if pid_alive(pid) {
            kill_pid(pid);
        }
    }
    if let Ok(mut g) = AI_PID.lock() {
        *g = None;
    }
}

fn spawn(app: &tauri::AppHandle, port: u16) -> Result<u32, String> {
    // 打包版入口（预留）：资源目录下有 ai/ai-server.exe 则优先用它
    let resource_exe = app
        .path()
        .resource_dir()
        .ok()
        .map(|d| d.join("ai").join("ai-server.exe"))
        .filter(|p| p.is_file());

    let mut cmd: Command;
    let python_dir = project_python_dir();

    if let Some(exe) = resource_exe {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd = Command::new(&exe);
            cmd.creation_flags(0x0800_0000);
        }
        #[cfg(not(target_os = "windows"))]
        {
            cmd = Command::new(&exe);
        }
    } else {
        // 开发版：python -m ai_studio.server（cwd=python，保证包导入）
        let script = python_dir.join("ai_studio").join("server.py");
        if !script.is_file() {
            // 缺内置组件：release 下绝不拼接构建机路径，只提示重装（BUG-2026-1013-001）。
            return Err(crate::devpath::dev_component_missing_err(
                "AI 创作服务脚本",
                "ai/ai-server.exe",
            ));
        }
        let interp = crate::vision::resolve_python_interpreter()?;
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd = Command::new(&interp);
            cmd.creation_flags(0x0800_0000);
        }
        #[cfg(not(target_os = "windows"))]
        {
            cmd = Command::new(&interp);
        }
        cmd.arg("-m").arg("ai_studio.server").current_dir(&python_dir);
    }

    cmd.env("AI_STUDIO_PORT", port.to_string());
    cmd.stdin(Stdio::null());

    // 子进程输出转存 app_data_dir/ai_studio_service.log（启动失败靠它定位依赖缺失）
    let mut log_path = None;
    if let Ok(dir) = app.path().app_data_dir() {
        let p = dir.join("ai_studio_service.log");
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            if let Ok(f2) = file.try_clone() {
                cmd.stdout(Stdio::from(f2));
            }
            cmd.stderr(Stdio::from(file));
        }
        log_path = Some(p);
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!(
            "启动 AI 创作服务失败: {e}（打包版请确认安装目录 ai/ai-server.exe 存在；开发版请 pip install -r python/requirements.txt）"
        ))?;
    let pid = child.id();
    crate::logger::log_info(&format!(
        "[ai-studio] 启动服务 pid={pid} port={port} log={}",
        log_path.map(|p| p.display().to_string()).unwrap_or_else(|| "<无>".into())
    ));
    Ok(pid)
}

/// 确保 AI 创作服务就绪，返回 base url（如 `http://127.0.0.1:52341`）。
pub async fn ensure_ready(app: &tauri::AppHandle) -> Result<String, String> {
    let _guard = AI_ENSURE.lock().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    // A. 本进程已就绪
    if let Some(base) = current_base() {
        if probe(&client, &base).await {
            return Ok(base);
        }
        clear_base();
    }

    // B. 清理上一轮遗留进程
    let old = AI_PID.lock().ok().and_then(|g| *g);
    if let Some(pid) = old {
        if pid_alive(pid) {
            kill_pid(pid);
        }
        if let Ok(mut g) = AI_PID.lock() {
            *g = None;
        }
    }

    // C. 冷启动 + 轮询就绪
    let port = pick_port()?;
    let base = format!("http://127.0.0.1:{port}");
    let pid = spawn(app, port)?;
    if let Ok(mut g) = AI_PID.lock() {
        *g = Some(pid);
    }

    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if probe(&client, &base).await {
            set_base(&base);
            crate::logger::log_info(&format!("[ai-studio] 服务就绪 base={base}"));
            return Ok(base);
        }
        if !pid_alive(pid) {
            return Err(
                "AI 创作服务启动后立即退出：请检查 python 依赖（fastapi / uvicorn / requests / Pillow）是否已安装，或查看 app_data/ai_studio_service.log"
                    .to_string(),
            );
        }
        if Instant::now() > deadline {
            return Err("AI 创作服务启动超时（40s）：请检查 python 依赖与 ai_studio_service.log".to_string());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
