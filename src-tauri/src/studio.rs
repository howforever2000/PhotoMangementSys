//! 创意工坊微服务生命周期（FEAT-063）
//! ===================================================================
//! 职责：启动/收养/探活「创意工坊」Python 微服务（FastAPI，见
//! `python-studio/server.py`），并把服务基址提供给前端直连。
//!
//! 与 `vision.rs`（VCR 识别服务）的关系：
//! - 同一套 ensure 思路（探活 → 收养 → 启动），但**大幅精简**：
//!   本服务无模型、无数据库、无预热，`/health` 可达即完全就绪，
//!   不需要 90s READY 超时 / GPU 状态机 / 组件防陈旧风暴等重型机制；
//! - 独立端口（默认 8790 起，动态分配）、独立实例文件
//!   （studio-instance.json），与 VCR 互不影响。
//!
//! 前端通信方式：`studio_ensure` 拿到基址后，前端直接 fetch
//! `http://127.0.0.1:{port}/api/*`（本服务只监听回环地址 + 全放行 CORS）。
//! 图片/蒙版走 multipart，无需经 Rust 中转；「保存结果」因前端无 fs
//! 写权限，由本模块 `studio_save_result` 命令落盘。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::Manager;

/// 服务基址（ensure 成功后设置；跨重启经 studio-instance.json 收养）
static STUDIO_BASE: Mutex<Option<String>> = Mutex::new(None);

fn set_base(base: String) {
    *STUDIO_BASE.lock().unwrap() = Some(base);
}

/// 探活超时：本服务毫秒级响应，宽限 3s 足够
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// 启动等待上限：uvicorn 绑定端口通常 <1s，留足冷启动余量
const START_TIMEOUT: Duration = Duration::from_secs(20);

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(8790)
}

fn instance_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|d| d.join("studio-instance.json"))
        .map_err(|e| format!("获取应用数据目录失败: {e}"))
}

fn save_instance(app: &tauri::AppHandle, pid: u32, port: u16) {
    if let Ok(path) = instance_file(app) {
        let body = format!("{{\"pid\":{pid},\"port\":{port}}}");
        let _ = std::fs::write(path, body);
    }
}

fn load_instance(app: &tauri::AppHandle) -> Option<(u32, u16)> {
    let path = instance_file(app).ok()?;
    let body = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    Some((v.get("pid")?.as_u64()? as u32, v.get("port")?.as_u64()? as u16))
}

fn clear_instance(app: &tauri::AppHandle) {
    if let Ok(path) = instance_file(app) {
        let _ = std::fs::remove_file(path);
    }
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .creation_flags(0x0800_0000)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

fn kill_pid(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .creation_flags(0x0800_0000)
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

async fn probe(client: &reqwest::Client, base: &str) -> Option<u64> {
    let v: serde_json::Value = client
        .get(format!("{base}/health"))
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    if v.get("ok")?.as_bool()? {
        v.get("api").and_then(|x| x.as_u64())
    } else {
        None
    }
}

/// 子进程输出缓冲上限：报错时只取尾部，防止占位符/异常输出刷屏
const OUTPUT_TAIL_CHARS: usize = 600;

fn output_tail(buf: &Arc<Mutex<String>>) -> String {
    let s = buf.lock().map(|g| g.trim().to_string()).unwrap_or_default();
    if s.chars().count() <= OUTPUT_TAIL_CHARS {
        return s;
    }
    s.chars().skip(s.chars().count() - OUTPUT_TAIL_CHARS).collect()
}

/// 起线程收集子进程 stdout/stderr（供启动失败时附在错误信息里）。
/// 返回 (缓冲, 读线程句柄)；探活成功后应 join 句柄并丢弃缓冲。
fn collect_output(child: &mut Child) -> (Arc<Mutex<String>>, Vec<std::thread::JoinHandle<()>>) {
    let buf = Arc::new(Mutex::new(String::new()));
    let mut readers = Vec::new();
    let streams = [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
    ];
    for stream in streams.into_iter().flatten() {
        let buf = buf.clone();
        readers.push(std::thread::spawn(move || {
            let mut stream = stream;
            let mut s = String::new();
            let _ = stream.read_to_string(&mut s);
            if let Ok(mut g) = buf.lock() {
                g.push_str(&s);
            }
        }));
    }
    (buf, readers)
}

/// 启动服务（打包版优先内置 exe；开发版 python-studio/server.py）。
/// 返回 (PID, 子进程)——子进程句柄由调用方持有，用于秒退检测。
/// 开发版解释器经 `vision::resolve_python_interpreter` 解析（BUG-2026-0918-007：
/// 裸 `python` 命中 Store 占位符，spawn 成功但立刻退出，20s 探活超时）。
fn spawn_studio(app: &tauri::AppHandle, port: u16) -> Result<(u32, Child), String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("获取资源目录失败: {e}"))?;
    let bundled_dir = resource_dir.join("studio");
    let bundled_exe = bundled_dir.join("studio-server.exe");

    let mut cmd: std::process::Command;
    let mut workdir = bundled_dir.clone();

    if bundled_exe.is_file() {
        // 打包版：PyInstaller 单文件（后续随 MSI 内置，接 PyInstaller 打包脚本即可）
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd = std::process::Command::new(&bundled_exe);
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        #[cfg(not(target_os = "windows"))]
        {
            cmd = std::process::Command::new(&bundled_exe);
        }
    } else {
        // 开发版：<解析出的解释器> python-studio/server.py
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let py_dir = manifest
            .parent()
            .map(|p| p.join("python-studio"))
            .unwrap_or_else(|| manifest.join("python-studio"));
        let script = py_dir.join("server.py");
        if !script.is_file() {
            return Err(format!("创意工坊服务脚本不存在: {}", script.display()));
        }
        workdir = py_dir;
        let interp = crate::vision::resolve_python_interpreter()?;
        cmd = std::process::Command::new(&interp);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.arg(&script);
    }

    cmd.env("STUDIO_PORT", port.to_string());
    // 执行日志落盘（后端排错的事实依据；打包版同样生效）
    if let Ok(dir) = app.path().app_data_dir() {
        if std::fs::create_dir_all(&dir).is_ok() {
            cmd.env("STUDIO_LOG", dir.join("studio-server.log"));
        }
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.current_dir(&workdir).spawn().map_err(|e| {
        format!("启动创意工坊服务失败（开发版请先 pip install fastapi uvicorn opencv-python numpy）: {e}")
    })?;
    Ok((child.id(), child))
}

/// ensure：返回服务基址（http://127.0.0.1:{port}）。
/// 顺序：本进程已ensure → 实例文件收养（健康检查通过）→ 清死实例 → 新启。
async fn ensure_base(app: &tauri::AppHandle) -> Result<String, String> {
    // 1) 本进程已管理且健康
    //    注意：先把缓存值 clone 出来再判断——std::sync::MutexGuard 不是 Send，
    //    守卫若借 if let 表达式的临时值活到 await 之后，future 就不再 Send。
    let cached = STUDIO_BASE.lock().unwrap().clone();
    if let Some(base) = cached {
        let client = reqwest::Client::new();
        if probe(&client, &base).await.is_some() {
            return Ok(base);
        }
        *STUDIO_BASE.lock().unwrap() = None;
    }

    // 2) 收养上次遗留实例（PID 存活 + /health 可达且 api 版本匹配）
    if let Some((pid, port)) = load_instance(app) {
        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();
        match probe(&client, &base).await {
            Some(api) if api == 1 && pid_alive(pid) => {
                set_base(base.clone());
                return Ok(base);
            }
            _ => {
                // 进程死了清实例；进程活着但服务不健康（版本不符/僵死）则杀掉重启
                if pid_alive(pid) {
                    kill_pid(pid);
                }
                clear_instance(app);
            }
        }
    }

    // 3) 新启动（探活期间监控子进程：秒退立即报错并附输出尾部，不干等满超时）
    let port = pick_free_port();
    let (pid, mut child) = spawn_studio(app, port)?;
    let (out_buf, mut readers) = collect_output(&mut child);
    let base = format!("http://127.0.0.1:{port}");

    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            clear_instance(app);
            for r in readers.drain(..) {
                let _ = r.join();
            }
            let tail = output_tail(&out_buf);
            return Err(if tail.is_empty() {
                format!("创意工坊服务进程提前退出（code={:?}，无输出）", status.code())
            } else {
                format!("创意工坊服务进程提前退出（code={:?}）：{tail}", status.code())
            });
        }
        if tokio::time::Instant::now() >= deadline {
            kill_pid(pid);
            clear_instance(app);
            for r in readers.drain(..) {
                let _ = r.join();
            }
            let tail = output_tail(&out_buf);
            return Err(format!(
                "创意工坊服务启动超时（{}s 内 /health 不可达）{}",
                START_TIMEOUT.as_secs(),
                if tail.is_empty() {
                    String::new()
                } else {
                    format!("；服务输出尾部：{tail}")
                }
            ));
        }
        if probe(&client, &base).await.is_some() {
            for r in readers.drain(..) {
                let _ = r.join();
            }
            save_instance(app, pid, port);
            set_base(base.clone());
            return Ok(base);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 前端命令：确保工坊服务就绪并返回基址
#[tauri::command]
pub async fn studio_ensure(app: tauri::AppHandle) -> Result<String, String> {
    ensure_base(&app).await
}

/// 前端命令：把工坊处理结果字节落盘（前端无 fs 写权限，由后端代写）
#[tauri::command]
pub async fn studio_save_result(path: String, data: Vec<u8>) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("保存路径为空".into());
    }
    std::fs::write(&path, data).map_err(|e| format!("写入文件失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_free_port_returns_usable_port() {
        let p = pick_free_port();
        assert!(p > 0);
    }

    #[tokio::test]
    async fn probe_bad_base_is_none() {
        let client = reqwest::Client::new();
        // 回环上几乎不可能有服务监听的端口
        assert!(probe(&client, "http://127.0.0.1:1").await.is_none());
    }
}
