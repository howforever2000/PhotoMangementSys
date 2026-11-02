//! 通用 Python 侧车生命周期管理（`SidecarSpec` 参数化）。
//!
//! 来源：FEAT-063 的 `studio.rs`（`git show 4af7760^:src-tauri/src/studio.rs`）。
//! 那次踩平的坑原样保留，一条都不许"简化"掉 —— 每一条都对应过线上故障：
//!
//! 1. **实例文件与内存缓存在探活成功后立刻写**，绝不排在"等子进程输出"之后；
//! 2. **输出读线程绝不 `join`** —— 服务活着就没有 EOF，join 必然把整个 ensure 卡死
//!    （BUG-2026-0918-009：表现为前端"永远加载中"，且每次重试泄漏一个 Python 进程）；
//! 3. **kill 必须带 `/T`**（Windows venv 的 `python.exe` 只是启动器，真实解释器是子进程）；
//! 4. **ensure 单飞**（并发调用各自 spawn 是"后台一堆 python"的另一半来源）；
//! 5. **端口每次向 0 要**，启动失败要大声失败并把 stderr 尾巴塞进错误文案，
//!    且文案里绝不出现构建机绝对路径（BUG-2026-1013-001）。
//!
//! 与 `vision.rs`（VCR 识别服务）的关系：同一套思路，但本模块不碰模型、
//! 不碰 GPU 状态机、不做版本化重启 —— 它服务的对象只是 HTTP + Pillow。
//!
//! ## 比 studio 多出来的一件事：per-process token
//!
//! Agent 侧车的请求体里带 provider 明文 key（只在内存里活一次），因此端点必须
//! 校验 `X-Agent-Token`。token 由本模块生成并通过环境变量注入子进程。
//!
//! 由此引出一个 studio 没有的问题：**收养上次遗留的实例时，我们手里没有它的 token**。
//! 解法是把 token 一起写进实例文件，收养时复用；同时用「请求一个不存在的 job」
//! 来验证 token（`404` = token 通过，`401` = token 不对），从而把"该收养"和
//! "该杀掉重起"区分开。实例文件与 `app.key` 同处 `app_data_dir`，属同一信任域。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand_core::RngCore;
use tauri::Manager;

/// 一个侧车服务的静态描述。`env` 用于注入端口/密钥等运行期参数。
pub struct SidecarSpec {
    /// 人话名字（错误文案里用）
    pub name: &'static str,
    /// 打包后位于 `resource_dir/<resource_subdir>/` 下
    pub resource_subdir: &'static str,
    /// 打包后的 exe 文件名
    pub exe_file: &'static str,
    /// 开发版入口（相对项目根，如 `python/agent_server.py`）
    pub dev_entry: &'static str,
    /// 实例文件名（落在 app_data_dir）
    pub instance_file: &'static str,
    /// 探活路径（免 token）
    pub health_path: &'static str,
    /// 日志文件名（落在 app_data_dir；子进程输出由读线程写入）
    pub log_name: &'static str,
    /// `/health` 返回的 `api` 版本；不匹配即视为"该重启"
    pub api_version: u64,
    /// 拉起时注入的环境变量
    pub env: fn(port: u16, secret: &str) -> Vec<(String, String)>,
}

/// 侧车的运行期状态（由 Tauri 以 `State` 托管）。
pub struct SidecarState {
    spec: &'static SidecarSpec,
    base: Mutex<Option<String>>,
    /// per-process token；收养遗留实例时会被替换成实例文件里那个
    secret: Mutex<String>,
    /// ensure 单飞锁
    ensure_lock: tokio::sync::Mutex<()>,
}

/// 探活超时：本类服务毫秒级响应
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// 启动等待上限：uvicorn 绑定端口通常 <1s，留足冷启动 + PyInstaller 解包余量
const START_TIMEOUT: Duration = Duration::from_secs(25);
/// 排障用输出缓冲上限（字符）
const OUTPUT_TAIL_CHARS: usize = 4000;
/// 错误文案里附带的 stderr 尾巴长度
const ERROR_TAIL_CHARS: usize = 600;

impl SidecarState {
    pub fn new(spec: &'static SidecarSpec) -> Self {
        Self {
            spec,
            base: Mutex::new(None),
            secret: Mutex::new(random_secret()),
            ensure_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// 已就绪基址（未 ensure 过则为 None）
    pub fn base(&self) -> Option<String> {
        self.base.lock().ok().and_then(|g| g.clone())
    }

    /// 当前进程的 token（客户端据此拼 `X-Agent-Token`）
    pub fn secret(&self) -> String {
        self.secret.lock().map(|g| g.clone()).unwrap_or_default()
    }

    fn set_base(&self, base: String) {
        if let Ok(mut g) = self.base.lock() {
            *g = Some(base);
        }
    }

    fn set_secret(&self, secret: String) {
        if let Ok(mut g) = self.secret.lock() {
            *g = secret;
        }
    }

    fn clear_base(&self) {
        if let Ok(mut g) = self.base.lock() {
            *g = None;
        }
    }

    /// 确保服务就绪并返回基址（`http://127.0.0.1:{port}`）。
    ///
    /// 顺序：本进程缓存 → 收养实例文件 → 清死实例 → 新启。
    pub async fn ensure(&self, app: &tauri::AppHandle) -> Result<String, String> {
        let _guard = self.ensure_lock.lock().await;
        let started = std::time::Instant::now();
        let out = self.ensure_inner(app).await;
        match &out {
            Ok(base) => crate::logger::log_info(&format!(
                "{} 侧车就绪 | {} | {}ms",
                self.spec.name,
                base,
                started.elapsed().as_millis()
            )),
            Err(e) => crate::logger::log_error(&format!("{}_ensure", self.spec.name), e),
        }
        out
    }

    async fn ensure_inner(&self, app: &tauri::AppHandle) -> Result<String, String> {
        let client = reqwest::Client::new();

        // 1) 本进程已管理且健康
        //    先把缓存值 clone 出来再判断：MutexGuard 不是 Send，
        //    守卫若活到 await 之后，future 就不再 Send。
        if let Some(base) = self.base() {
            if probe(&client, &base, self.spec.health_path).await == Some(self.spec.api_version) {
                return Ok(base);
            }
            crate::logger::log_warn(&format!("{} 侧车缓存失活，准备重启", self.spec.name));
            self.clear_base();
        }

        let inst_path = self.instance_path(app)?;

        // 2) 收养上次遗留实例（PID 存活 + /health 可达 + token 可验证）
        if let Some((pid, port, secret)) = load_instance(&inst_path) {
            let base = format!("http://127.0.0.1:{port}");
            let healthy =
                probe(&client, &base, self.spec.health_path).await == Some(self.spec.api_version);
            if healthy && pid_alive(pid) && verify_secret(&client, &base, &secret).await {
                crate::logger::log_info(&format!(
                    "{} 侧车收养成功 | pid={pid} port={port}",
                    self.spec.name
                ));
                self.set_secret(secret);
                self.set_base(base.clone());
                return Ok(base);
            }
            // 进程活着但服务不健康（版本不符/僵死/token 不符）→ 杀掉重起；
            // 进程已死则只清文件。
            if pid_alive(pid) {
                crate::logger::log_warn(&format!(
                    "{} 侧车实例不可用（healthy={healthy}），杀掉重起 pid={pid}",
                    self.spec.name
                ));
                kill_pid(pid);
            }
            clear_instance(&inst_path);
        }

        // 3) 新启动
        let port = pick_free_port();
        let secret = self.secret();
        let (pid, mut child) = self.spawn(app, port, &secret)?;
        let (out_buf, mut readers) = collect_output(&mut child, log_path(app, self.spec));

        let base = format!("http://127.0.0.1:{port}");
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            // 秒退立即报错（附输出尾巴），不干等满超时
            if let Ok(Some(status)) = child.try_wait() {
                clear_instance(&inst_path);
                let tail = output_tail_settled(&out_buf, std::mem::take(&mut readers)).await;
                return Err(self.fail_msg(
                    format!("{}服务进程提前退出（code={:?}）", self.spec.name, status.code()),
                    &tail,
                ));
            }
            if tokio::time::Instant::now() >= deadline {
                kill_pid(pid);
                clear_instance(&inst_path);
                let tail = output_tail_settled(&out_buf, std::mem::take(&mut readers)).await;
                return Err(self.fail_msg(
                    format!(
                        "{}服务启动超时（{}s 内 {} 不可达）",
                        self.spec.name,
                        START_TIMEOUT.as_secs(),
                        self.spec.health_path
                    ),
                    &tail,
                ));
            }
            if probe(&client, &base, self.spec.health_path).await == Some(self.spec.api_version) {
                // 就绪：先落实例文件与进程内缓存，再丢弃输出句柄。
                // 顺序很关键 —— 旧实现先 join 输出线程，永远走不到这一步。
                save_instance(&inst_path, pid, port, &secret);
                self.set_base(base.clone());
                drop(readers); // 只丢句柄：线程继续排空管道，退役于进程退出
                return Ok(base);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// 组装"大声失败"的错误文案：stderr 尾巴 + 两条可行动建议。
    fn fail_msg(&self, headline: String, tail: &str) -> String {
        let hint = if tail.is_empty() {
            "（无输出）排查：① 是否被杀软拦截；② 端口是否被占用；③ 便携版路径是否含特殊字符"
                .to_string()
        } else {
            format!("输出尾部：{tail}")
        };
        format!("{headline}。{hint}。可重试，或到应用数据目录查看 {} 日志。", self.spec.log_name)
    }

    /// 应用退出时调用：杀掉自己拉起的实例（崩溃路径不调用，实例文件留着给人收养）。
    pub fn shutdown(&self, app: &tauri::AppHandle) {
        if let Ok(path) = self.instance_path(app) {
            if let Some((pid, _, _)) = load_instance(&path) {
                if pid_alive(pid) {
                    kill_pid(pid);
                    crate::logger::log_info(&format!("{} 侧车已随应用退出 pid={pid}", self.spec.name));
                }
            }
            clear_instance(&path);
        }
    }

    fn instance_path(&self, app: &tauri::AppHandle) -> Result<PathBuf, String> {
        app.path()
            .app_data_dir()
            .map(|d| d.join(self.spec.instance_file))
            .map_err(|e| format!("获取应用数据目录失败: {e}"))
    }

    /// 拉起子进程：打包版优先内置 exe，开发版 `python/agent_server.py`。
    fn spawn(
        &self,
        app: &tauri::AppHandle,
        port: u16,
        secret: &str,
    ) -> Result<(u32, Child), String> {
        let resource_dir = app
            .path()
            .resource_dir()
            .map_err(|e| format!("获取资源目录失败: {e}"))?;
        let bundled_dir = resource_dir.join(self.spec.resource_subdir);
        let bundled_exe = bundled_dir.join(self.spec.exe_file);

        let mut cmd: std::process::Command;
        let mut workdir = bundled_dir.clone();

        if bundled_exe.is_file() {
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
            // 开发版。⚠️ 报错文案绝不能带绝对路径（构建机用户名会显示给用户，
            // BUG-2026-1013-001）—— 统一走 devpath 的中性文案。
            let script = crate::devpath::dev_project_dir().join(self.spec.dev_entry);
            if !script.is_file() {
                return Err(crate::devpath::dev_component_missing_err(
                    self.spec.name,
                    &format!("{}/{}", self.spec.resource_subdir, self.spec.exe_file),
                ));
            }
            workdir = script
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| crate::devpath::dev_project_dir());
            // 裸 `python` 会命中 Store 占位符（spawn 成功但立刻退出，BUG-2026-0918-007）
            let interp = crate::vision::resolve_python_interpreter()?;
            cmd = std::process::Command::new(&interp);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            cmd.arg(&script);
        }

        for (k, v) in (self.spec.env)(port, secret) {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = cmd.current_dir(&workdir).spawn().map_err(|e| {
            format!(
                "启动{}服务失败：{e}（开发版请先 pip install -r python/requirements-agent.txt）",
                self.spec.name
            )
        })?;
        crate::logger::log_info(&format!(
            "{} 侧车已拉起 pid={} port={port}",
            self.spec.name,
            child.id()
        ));
        Ok((child.id(), child))
    }
}

// ------------------------------------------------------------------ 工具函数

/// 向 0 号端口要一个空闲端口（docs/v5.md §4 第 5 条）。
pub fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(8791)
}

/// 128-bit 随机 token：复用已在依赖树里的 `rand_core`（与 `crypto.rs` 同一套 CSPRNG），
/// 不新增依赖。
pub fn random_secret() -> String {
    let mut buf = [0u8; 16];
    rand_core::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn log_path(app: &tauri::AppHandle, spec: &SidecarSpec) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join(spec.log_name))
}

fn instance_json(pid: u32, port: u16, secret: &str) -> String {
    // 手拼 JSON 而不是 derive：字段就三个，省一个结构体；
    // secret 是 hex，不含需要转义的字符。
    format!("{{\"pid\":{pid},\"port\":{port},\"secret\":\"{secret}\"}}")
}

fn save_instance(path: &Path, pid: u32, port: u16, secret: &str) {
    let _ = std::fs::write(path, instance_json(pid, port, secret));
}

fn load_instance(path: &Path) -> Option<(u32, u16, String)> {
    let body = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let pid = v.get("pid")?.as_u64()? as u32;
    let port = v.get("port")?.as_u64()? as u16;
    let secret = v.get("secret")?.as_str()?.to_string();
    if pid == 0 || port == 0 || secret.is_empty() {
        return None;
    }
    Some((pid, port, secret))
}

fn clear_instance(path: &Path) {
    let _ = std::fs::remove_file(path);
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
        // /T 连子进程一起结束（必须！venv 的 Scripts\python.exe 只是启动器）
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
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

/// 探活：拿到 `api` 版本号即视为可达（版本是否匹配由调用方判断）。
async fn probe(client: &reqwest::Client, base: &str, health_path: &str) -> Option<u64> {
    let v: serde_json::Value = client
        .get(format!("{base}{health_path}"))
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

/// 验证 token 是否属于该实例：请求一个**不存在的 job**。
/// 401 = token 不对；404 = token 通过（只是没有这个 job）。其余一律视为不通过。
async fn verify_secret(client: &reqwest::Client, base: &str, secret: &str) -> bool {
    let resp = client
        .get(format!("{base}/agent/jobs/0"))
        .header("X-Agent-Token", secret)
        .timeout(PROBE_TIMEOUT)
        .send()
        .await;
    matches!(resp, Ok(r) if r.status().as_u16() == 404)
}

fn push_bounded(buf: &Arc<Mutex<String>>, chunk: &str) {
    if let Ok(mut g) = buf.lock() {
        g.push_str(chunk);
        let len = g.chars().count();
        if len > OUTPUT_TAIL_CHARS {
            *g = g.chars().skip(len - OUTPUT_TAIL_CHARS).collect();
        }
    }
}

fn output_tail(buf: &Arc<Mutex<String>>) -> String {
    let s = buf.lock().map(|g| g.trim().to_string()).unwrap_or_default();
    if s.chars().count() <= ERROR_TAIL_CHARS {
        return s;
    }
    s.chars().skip(s.chars().count() - ERROR_TAIL_CHARS).collect()
}

/// 取（已退出/已强杀的）子进程输出尾部：给读线程一小段排空时间，**不做 join**。
///
/// 为什么不能 join：即使进程已经死了，管道也可能被孙进程继续持有 —— Windows 上
/// venv 的 `Scripts\python.exe` 是启动器，真实解释器是它拉起的子进程且继承同一组
/// 管道句柄；若只杀了启动器，EOF 永远不来，join 依旧挂死。
async fn output_tail_settled(
    buf: &Arc<Mutex<String>>,
    readers: Vec<std::thread::JoinHandle<()>>,
) -> String {
    drop(readers); // 只丢句柄：线程活着继续读，进程退出后自然结束
    let mut last = usize::MAX;
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        let now = buf.lock().map(|g| g.len()).unwrap_or(0);
        if now == last {
            break;
        }
        last = now;
    }
    output_tail(buf)
}

/// 起线程收集子进程 stdout/stderr：一份进内存尾部（报错用），一份追加落盘（排障用）。
///
/// ⚠️ 严禁在子进程存活时 `join` 这些句柄（BUG-2026-0918-009）。
fn collect_output(
    child: &mut Child,
    log_file: Option<PathBuf>,
) -> (Arc<Mutex<String>>, Vec<std::thread::JoinHandle<()>>) {
    let buf = Arc::new(Mutex::new(String::new()));
    let mut readers = Vec::new();

    // 每次拉起截断日志（保留"本次运行"的现场最有用；历史由 app.log 承担）
    if let Some(p) = &log_file {
        let _ = std::fs::write(p, b"");
    }

    let streams = [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
    ];
    for stream in streams.into_iter().flatten() {
        let buf = buf.clone();
        let log_file = log_file.clone();
        readers.push(std::thread::spawn(move || {
            let mut stream = stream;
            let mut file = log_file.and_then(|p| std::fs::OpenOptions::new().append(true).create(true).open(p).ok());
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&chunk[..n]).to_string();
                        push_bounded(&buf, &text);
                        if let Some(f) = file.as_mut() {
                            let _ = f.write_all(text.as_bytes());
                            let _ = f.flush();
                        }
                    }
                }
            }
        }));
    }
    (buf, readers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_free_port_returns_usable_port() {
        let p = pick_free_port();
        assert!(p > 0, "端口必须来自系统分配");
    }

    #[test]
    fn random_secret_is_128_bit_hex_and_unique() {
        let a = random_secret();
        let b = random_secret();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "两次生成不得相同");
    }

    #[test]
    fn instance_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pm-agent-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("agent-instance.json");

        save_instance(&p, 4321, 8791, "abc123");
        let got = load_instance(&p).expect("应能读回");
        assert_eq!(got, (4321, 8791, "abc123".to_string()));

        // 缺字段 / 非法值一律视为无实例（宁可重起，也不要收养一个半坏的实例）
        std::fs::write(&p, "{\"pid\":4321,\"port\":8791}").unwrap();
        assert!(load_instance(&p).is_none());
        std::fs::write(&p, "{\"pid\":0,\"port\":8791,\"secret\":\"x\"}").unwrap();
        assert!(load_instance(&p).is_none());

        clear_instance(&p);
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn push_bounded_keeps_tail() {
        let buf = Arc::new(Mutex::new(String::new()));
        for _ in 0..OUTPUT_TAIL_CHARS {
            push_bounded(&buf, "ab");
        }
        let len = buf.lock().unwrap().chars().count();
        assert!(len <= OUTPUT_TAIL_CHARS, "缓冲必须封顶，实得 {len}");
        assert!(len > OUTPUT_TAIL_CHARS - 8, "应保留最新内容");
    }

    #[tokio::test]
    async fn probe_bad_base_is_none() {
        let client = reqwest::Client::new();
        assert!(probe(&client, "http://127.0.0.1:1", "/health").await.is_none());
    }

    /// BUG-2026-0918-009 回归：子进程**还活着**时取输出必须立即返回。
    /// 旧实现 `for r in readers { r.join() }` 会在这里挂满子进程的整个生命周期。
    #[tokio::test]
    async fn output_tail_settled_returns_while_child_alive() {
        use std::process::{Command, Stdio};

        #[cfg(target_os = "windows")]
        let mut child = Command::new("cmd")
            .args(["/c", "ping", "-n", "15", "127.0.0.1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("起测试子进程失败");
        #[cfg(not(target_os = "windows"))]
        let mut child = Command::new("sleep")
            .arg("15")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("起测试子进程失败");

        let (buf, readers) = collect_output(&mut child, None);
        let t0 = std::time::Instant::now();
        let tail = output_tail_settled(&buf, readers).await;
        let elapsed = t0.elapsed();

        let _ = child.kill();
        let _ = child.wait();

        assert!(
            elapsed < Duration::from_secs(2),
            "子进程存活时取输出不得阻塞，实测 {elapsed:?}（超出即说明又去 join 了）"
        );
        // ping 会持续输出，所以这里只断言"拿到了东西且不卡"，不断言为空
        let _ = tail;
    }
}
