//! 视觉内容识别模块（VCR 客户端，测试功能，独立组件）
//!
//! 职责：启动 Python 微服务（FastAPI + ONNX Runtime，见 `python/server.py`），
//! 将相册目录内图片分批提交 `POST /classify_batch`，把 ImageNet 细类
//! 映射为相册大类（动物/食物/植物/建筑/运动/风景/文档/其他），
//! 并通过 Tauri 事件通道实时上报进度。
//!
//! 解耦原则：
//! - 不依赖 `db` / `thumbnail` / `tone` 模块（图片扩展名列表本地定义）
//! - 微服务为独立 Python 进程，本模块只是 HTTP 客户端 + 生命周期管理
//! - `lib.rs` 仅保留薄命令壳 `classify_album` / `open_image`
//! - 服务不可用 / 模型缺失 → 返回明确错误，不影响其他功能

use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tauri::{Emitter, Manager};

/// 识别服务基址（本进程内 ensure 成功启动/收养后设置；跨重启经 vcr-instance.json 收养）
static VCR_BASE: Mutex<Option<String>> = Mutex::new(None);

fn current_base() -> Option<String> {
    VCR_BASE.lock().ok().and_then(|g| g.clone())
}
fn set_base(base: String) {
    if let Ok(mut g) = VCR_BASE.lock() {
        *g = Some(base);
    }
}
fn clear_base() {
    if let Ok(mut g) = VCR_BASE.lock() {
        *g = None;
    }
}
/// 兼容既有调用：ensure 成功后必然已设置；未设置返回空串（调用方会先 ensure）
fn vcr_base() -> String {
    current_base().unwrap_or_default()
}

/// 冷门段选端口（避开 Windows 临时端口 49152~65535——该段被系统出站连接
/// 随机使用，是此前 bind 10048 的直接原因）；绑定测试通过才返回
fn pick_cold_port() -> u16 {
    for p in 18765..=18865 {
        if let Ok(l) = std::net::TcpListener::bind(("127.0.0.1", p)) {
            drop(l);
            return p;
        }
    }
    0
}

/// 实例落盘文件（跨应用重启收养存活服务 / 清理僵尸）：{"pid":..,"port":..}
fn instance_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?
        .join("vcr-instance.json"))
}
fn save_instance(app: &tauri::AppHandle, pid: u32, port: u16) {
    if let Ok(f) = instance_file(app) {
        let _ = std::fs::write(f, format!("{{\"pid\":{},\"port\":{}}}", pid, port));
    }
}
fn load_instance(app: &tauri::AppHandle) -> Option<(u32, u16)> {
    let f = instance_file(app).ok()?;
    let txt = std::fs::read_to_string(f).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    Some((v.get("pid")?.as_u64()? as u32, v.get("port")?.as_u64()? as u16))
}
fn clear_instance(app: &tauri::AppHandle) {
    if let Ok(f) = instance_file(app) {
        let _ = std::fs::remove_file(f);
    }
}

/// 进程存活检测（Windows tasklist / Unix /proc）
fn pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// 结束本项目识别服务进程（PID 明确属于我们，安全）
fn kill_pid(pid: u32) {
    eprintln!("[VCR][ensure] 结束本项目识别服务进程 PID={pid}");
    #[cfg(target_os = "windows")]
    {
        // /T 连子进程一起结束（PyInstaller 运行期若派生子进程不留孤儿）
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

/// 清理本项目遗留的识别服务孤儿（多次启动遗留的 vcr_server.exe /
/// 命令行含 server.py 的 python 实例；不触碰其他 python 程序）
fn kill_our_orphans() {
    eprintln!("[VCR][ensure] 清理本项目遗留识别服务进程…");
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", "vcr-server.exe"])
            .output();
        let script = "Get-CimInstance Win32_Process -Filter \"Name='python.exe'\" | Where-Object { $_.CommandLine -like '*server.py*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }";
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", script])
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("sh")
            .args(["-c", "pkill -f 'vcr-server' 2>/dev/null; pkill -f 'server.py' 2>/dev/null; true"])
            .output();
    }
}
/// 服务就绪等待上限（FEAT-051 后需加载 l/m 全通道模型，DML 首次初始化较慢）
const READY_TIMEOUT: Duration = Duration::from_secs(90);
/// FEAT-051：要求的服务 API 版本（GPU 开关 + 模型切换能力）；
/// 探测到运行中服务版本过旧时自动 POST /shutdown 重启到新版本
/// FEAT-051：API 版本。宿主检测到运行中服务版本过旧时自动重启到新版。
/// v3（FEAT-053）：/benchmark 端点 + /gpu /models /health 新增会话实测字段。
/// v4（FEAT-SEM）：语义搜索 —— /embed_text /embed_batch /health.clip_ready。
/// v5（语义分类）：分类模型/场景/专家通道下线；语义模型档位切换；新增 /embed_text_batch。
/// v6：CPU 线程数可调（/threads）+ 测速可临时指定线程数（/benchmark）+ 线程扫档（/benchmark_sweep）。
const VCR_API_VERSION: u64 = 6;
/// FEAT-051：ensure 单飞锁 —— 并发命令共享一次「探测/重启/启动」流程，
/// 邓免多进程同时拚 8765 端口（winerror 10048）
static ENSURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 过期组件登记（OldVersion 收敛保护）：冷启动拉起的服务探测后仍是旧版本，
/// 说明组件文件本身过期（重拉的永远是同一份 exe，重启进程无济于事）。
/// 记录「组件路径 + mtime」，后续 ensure 直接报错，杜绝「杀→拉→杀」资源风暴
/// （曾致十几个 vcr-server.exe 连环生灭 + 设置面板卡死）；组件被重新打包
/// （mtime 变化）后自动解除登记。
static STALE_COMPONENT: Mutex<Option<(PathBuf, SystemTime)>> = Mutex::new(None);

fn stale_component_record() -> Option<(PathBuf, SystemTime)> {
    STALE_COMPONENT.lock().ok().and_then(|g| g.clone())
}
fn set_stale_component(path: PathBuf) {
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if let (Some(m), Ok(mut g)) = (mtime, STALE_COMPONENT.lock()) {
        *g = Some((path, m));
    }
}
fn clear_stale_component() {
    if let Ok(mut g) = STALE_COMPONENT.lock() {
        *g = None;
    }
}
/// 当前 spawn 目标身份（与 spawn_server 的选择一致：打包 exe 优先，否则 python/server.py）
fn spawn_target_path(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .resource_dir()
        .ok()
        .map(|r| r.join("vcr").join("vcr-server.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| project_python_dir().join("server.py"))
}
fn stale_component_err(path: &Path) -> String {
    format!(
        "识别服务组件版本过旧（{}），重启进程无法自愈。请重新打包：\
         powershell -ExecutionPolicy Bypass -File python/build_vcr_exe.ps1，\
         并将 python/dist/vcr-server.exe 同步到 src-tauri/target 对应目录后重启应用；\
         开发环境可直接删除 target/debug/vcr/ 与 target/release/vcr/ 下的 \
         vcr-server.exe，改用 python/server.py 运行当前代码",
        path.display()
    )
}

/// 健康探测结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthProbe {
    /// 新版本服务已就绪
    Ready,
    /// 服务存活但是旧版本（缺 GPU/模型端点）→ 需重启
    OldVersion,
    /// 服务可连接但尚未就绪（正在加载模型）→ 等待，不打断
    Loading,
    /// 端口不可达（无进程 / 连接拒绝）→ 可安全拉起新进程
    NotReachable,
}

impl HealthProbe {
    fn label(self) -> &'static str {
        match self {
            HealthProbe::Ready => "Ready",
            HealthProbe::OldVersion => "OldVersion",
            HealthProbe::Loading => "Loading",
            HealthProbe::NotReachable => "NotReachable",
        }
    }
}

/// 性能设置链路统一日志前缀（便于 `grep '\[PERF\]'` 只看性能面板相关）
pub fn perf_log(msg: &str) {
    crate::logger::log_info(&format!("[PERF] {msg}"));
}

/// 把 /health 响应压成一行诊断摘要（日志用）：
/// `ok=true api=6 det=true face=false ocr=true clip=false err={...}`。
/// 面板「一直转圈」时这一行直接给出「为何 ok=false」——即宿主等待的真实原因。
fn health_digest(v: &serde_json::Value) -> String {
    let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let mut s = format!(
        "ok={} api={} det={} face={} ocr={} clip={}",
        b("ok"),
        v.get("api_version").and_then(|x| x.as_u64()).unwrap_or(0),
        b("det_ready"),
        b("face_ready"),
        b("ocr_ready"),
        b("clip_ready"),
    );
    if let Some(errs) = v.get("load_errors").and_then(|x| x.as_object()) {
        if !errs.is_empty() {
            let list: Vec<String> = errs
                .iter()
                .map(|(k, e)| format!("{k}={}", e.as_str().unwrap_or("?")))
                .collect();
            s.push_str(&format!(" load_errors=[{}]", list.join("; ")));
        }
    }
    s
}

/// 探测 /health，同时返回诊断摘要（摘要为空串 = 端口不可达/无法解析，无信息可记）
async fn probe_base(client: &reqwest::Client, base: &str) -> (HealthProbe, String) {
    let resp = match client
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return (HealthProbe::NotReachable, String::new()), // 连接拒绝/超时 → 端口可拉起
    };
    let v = match resp.json::<serde_json::Value>().await {
        Ok(v) => v,
        Err(_) => return (HealthProbe::Loading, String::new()), // 可连接但解析失败 → 视为加载中
    };
    let digest = health_digest(&v);
    let ok = v.get("ok").and_then(|x| x.as_bool()) == Some(true);
    if ok {
        let ver = v.get("api_version").and_then(|x| x.as_u64()).unwrap_or(1);
        if ver >= VCR_API_VERSION {
            (HealthProbe::Ready, digest)
        } else {
            (HealthProbe::OldVersion, digest)
        }
    } else {
        // ok=false = 服务可达但模型未就绪（正在加载）→ 等待，不打断
        (HealthProbe::Loading, digest)
    }
}

/// 支持的图片扩展名（与 photo_scan/tone 一致；为解耦本地复制一份）
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

/// Top3 单项
#[derive(Debug, Clone, Serialize)]
pub struct VisionTopItem {
    /// 相册大类
    pub category: String,
    /// 最具体的 ImageNet 细类名
    pub label: String,
    /// 大类置信度（0~1）
    pub confidence: f64,
}

/// 单张图片的识别结果
#[derive(Debug, Clone, Serialize)]
pub struct VisionResult {
    /// 文件名（不含路径）
    pub file_name: String,
    /// 完整路径（前端 tooltip / 打开图片用）
    pub path: String,
    /// 相册大类
    pub category: String,
    /// 子类（动物→狗/猫/鸟等）
    pub sub_category: String,
    /// 最具体的 ImageNet 细类名（如 "golden retriever"）
    pub label: String,
    /// 大类置信度（0~1）
    pub confidence: f64,
    /// Top3 候选（大类 + 细类 + 置信度）
    pub top3: Vec<VisionTopItem>,
    /// 同人标号（如 ["P001","P003"]）
    pub person_ids: Vec<String>,
    /// 检测到的人数
    pub person_count: usize,
    /// 推理耗时（毫秒）
    pub elapsed_ms: f64,
    /// 单张失败原因（如无法读取图片）
    pub error: Option<String>,
}

/// 批量识别进度事件载荷（前端进度条）
#[derive(Debug, Clone, Serialize)]
pub struct ClassifyProgress {
    /// 已处理图片数
    pub current: usize,
    /// 图片总数
    pub total: usize,
    /// 成功数
    pub done: usize,
    /// 失败数
    pub failed: usize,
}

/// 识别相册目录内所有图片的内容（递归子目录，跳过隐藏文件/目录）
///
/// 流程：收集图片路径 → 确保微服务就绪 → 按 `batch_size` 分批 /classify_batch →
/// 每批 emit `classify-progress` 事件 → 汇总返回。
pub async fn classify_album(
    dir: &str,
    batch_size: usize,
    app: &tauri::AppHandle,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Vec<VisionResult>, String> {
    let photos = collect_images(dir)?;
    classify_paths(&photos, batch_size, app, cancel).await
}

/// 对显式路径列表批量识别（FEAT-SEM：供增量扫描跳过已入库照片后传入）
pub async fn classify_paths(
    paths: &[String],
    batch_size: usize,
    app: &tauri::AppHandle,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Vec<VisionResult>, String> {
    let photos = paths.to_vec();
    if photos.is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    ensure_service_ready(&client, app, true).await?;

    let batch = batch_size.max(1);
    let mut results: Vec<VisionResult> = Vec::with_capacity(photos.len());
    let mut done = 0usize;
    let mut failed = 0usize;

    for chunk in photos.chunks(batch) {
        // 收到停止请求 → 提前结束，保留已识别部分
        if cancel
            .as_ref()
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(false)
        {
            break;
        }
        let resp: serde_json::Value = client
            .post(format!("{}/classify_batch", vcr_base()))
            .json(&serde_json::json!({ "paths": chunk }))
            .send()
            .await
            .map_err(|e| format!("调用识别服务失败: {e}"))?
            .json()
            .await
            .map_err(|e| format!("解析识别结果失败: {e}"))?;

        if let Some(items) = resp.get("results").and_then(|v| v.as_array()) {
            for item in items {
                results.push(parse_item(item));
                if item.get("error").is_some() {
                    failed += 1;
                } else {
                    done += 1;
                }
            }
        }

        // 上报进度
        let _ = app.emit(
            "classify-progress",
            ClassifyProgress {
                current: results.len().min(photos.len()),
                total: photos.len(),
                done,
                failed,
            },
        );
    }

    Ok(results)
}

/// 收集目录内全部图片路径（与 photo_scan/tone 一致的遍历规则）
pub fn collect_images(dir: &str) -> Result<Vec<String>, String> {
    let root = Path::new(dir);
    if !root.is_dir() {
        return Err(format!("路径不存在或不是文件夹: {dir}"));
    }
    let mut photos: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        let Ok(e) = entry else { continue };
        if !e.file_type().is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        if IMAGE_EXTS.iter().any(|ext| lower.ends_with(&format!(".{ext}"))) {
            photos.push(e.into_path().to_string_lossy().into_owned());
        }
    }
    photos.sort();
    Ok(photos)
}

/// FEAT-D：单张图片识别（不依赖相册目录扫描）
///
/// 为「ensure_photo_scanned」专用：
/// - 走 /classify_batch 但只传 1 个 path（仍受服务是否就绪控制）
/// - 复用 parse_item 避免重复代码
/// - 失败 / 服务不可用时返回 error 项（不是抛错），让上层仍能落库
pub async fn classify_single(path: &str, app: &tauri::AppHandle) -> Result<VisionResult, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    ensure_service_ready(&client, app, true).await?;
    let resp: serde_json::Value = client
        .post(format!("{}/classify_batch", vcr_base()))
        .json(&serde_json::json!({ "paths": [path] }))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("解析识别结果失败: {e}"))?;
    if let Some(items) = resp.get("results").and_then(|v| v.as_array()) {
        if let Some(item) = items.first() {
            return Ok(parse_item(item));
        }
    }
    // 服务返回结构异常：返回 error 项而不是抛错
    Ok(VisionResult {
        file_name: Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_string(),
        category: String::new(),
        sub_category: String::new(),
        label: String::new(),
        confidence: 0.0,
        top3: Vec::new(),
        person_ids: Vec::new(),
        person_count: 0,
        elapsed_ms: 0.0,
        error: Some("服务未返回结果".into()),
    })
}

/// 确保视觉识别微服务已就绪；未运行则启动并轮询 /health
///
/// 启动优先级：
///   1. 打包版：`resource_dir/vcr/vcr-server.exe`（PyInstaller 单文件，随 MSI 内置）
///   2. 开发版：`python/server.py`（需 `pip install -r python/requirements.txt` 并在 PATH 中）
///
/// 打包版通过环境变量把「模型目录」指向随安装包内置的资源目录（只读）、
/// 把「数据目录」指向 app_data_dir（可写，避免写入 Program Files）。
async fn ensure_service_ready(
    client: &reqwest::Client,
    app: &tauri::AppHandle,
    wait_models: bool,
) -> Result<(), String> {
    // 单飞：并发命令共享一次「探测/收养/重启/启动」，避免多进程抽数据库与端口
    perf_log(&format!("ensure 请求 | wait_models={wait_models}"));
    let t_enter = Instant::now();
    let _guard = ENSURE_LOCK.lock().await;
    perf_log(&format!(
        "ensure 获得单飞锁 | 排队等待 {}ms",
        t_enter.elapsed().as_millis()
    ));
    let t0 = Instant::now();

    // 0. 过期组件收敛保护：上次冷启动已确认组件文件过期 → 直接报错，
    //    不再「杀→拉→杀」空转（组件重新打包后 mtime 变化自动解除）
    if let Some((path, mtime)) = stale_component_record() {
        let cur = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if cur == Some(mtime) {
            perf_log(&format!(
                "ensure 拒绝：组件过期登记未解除 path={}",
                path.display()
            ));
            return Err(stale_component_err(&path));
        }
        clear_stale_component();
    }

    let inst = load_instance(app);

    // A. 本进程已启动/收养过的实例
    if let Some(base) = current_base() {
        let (probe, digest) = probe_base(client, &base).await;
        perf_log(&format!(
            "ensure A段 进程内base={base} → {} | {digest}",
            probe.label()
        ));
        match probe {
            HealthProbe::Ready => {
                perf_log(&format!("ensure 完成（A段直接命中）| 耗时 {}ms", t0.elapsed().as_millis()));
                return Ok(());
            }
            HealthProbe::Loading => {
                wait_service(client, &base, wait_models).await?;
                perf_log(&format!(
                    "ensure 完成（A段等待就绪）| base={base} wait_models={wait_models} 耗时 {}ms",
                    t0.elapsed().as_millis()
                ));
                return Ok(());
            }
            // OldVersion / NotReachable → 统一落到 B 段按实例记录处理
            //（旧版本重启 / 收养等待或清场），杀进程逻辑只保留一处，避免分叉
            _ => {
                perf_log("ensure A段 缓存 base 不可用 → 清除缓存，转 B 段实例记录判定");
                clear_base();
            }
        }
    }

    // B. 实例记录（本进程或上次运行遗留）
    if let Some((pid, port)) = inst {
        let base = format!("http://127.0.0.1:{port}");
        let (probe, digest) = probe_base(client, &base).await;
        perf_log(&format!(
            "ensure B段 实例 pid={pid} port={port} → {} | {digest}",
            probe.label()
        ));
        match probe {
            HealthProbe::Ready => {
                set_base(base);
                perf_log(&format!("ensure 完成（B段收养就绪实例）| 耗时 {}ms", t0.elapsed().as_millis()));
                return Ok(());
            }
            HealthProbe::Loading => {
                wait_service(client, &base, wait_models).await?;
                set_base(base);
                perf_log(&format!(
                    "ensure 完成（B段等待就绪）| wait_models={wait_models} 耗时 {}ms",
                    t0.elapsed().as_millis()
                ));
                return Ok(());
            }
            HealthProbe::OldVersion => {
                // 旧版本服务：请其自退并兜底强杀，再冷启动到新版本
                perf_log(&format!("ensure B段 实例为旧版本 → 请求自退并强杀 pid={pid}"));
                let _ = client
                    .post(format!("{base}/shutdown"))
                    .timeout(Duration::from_secs(2))
                    .send()
                    .await;
                tokio::time::sleep(Duration::from_millis(800)).await;
                if pid_alive(pid) {
                    kill_pid(pid);
                }
                clear_instance(app);
            }
            HealthProbe::NotReachable => {
                // 端口不可达 ≠ 可杀：Python 服务先起进程、加载完模型才监听端口，
                // 「进程存活但端口未监听」是正常的启动中状态 → 保留并等待就绪
                //（与 C 段超时策略一致）。旧版在此直接 kill 存活进程，C 段保留的
                // 加载进程被下一轮 ensure 误杀重启 → 「杀→重拉→再杀」死循环
                //（PID 连环变化 + ConnectionResetError 10054 刷屏）。
                let alive = pid_alive(pid);
                perf_log(&format!(
                    "ensure B段 端口不可达 pid={pid} 进程存活={alive} → {}",
                    if alive { "收养等待（不杀）" } else { "清场后冷启动" }
                ));
                if alive {
                    match wait_service(client, &base, wait_models).await {
                        Ok(()) => {
                            set_base(base);
                            perf_log(&format!(
                                "ensure 完成（B段收养启动中进程）| 耗时 {}ms",
                                t0.elapsed().as_millis()
                            ));
                            return Ok(());
                        }
                        // 进程仍在启动/加载：保留实例，下次 ensure 继续收养等待
                        Err(err) => {
                            perf_log(&format!("ensure 失败（B段收养等待超时，实例保留）| {err}"));
                            return Err(err);
                        }
                    }
                }
                // 进程确已死亡（崩溃 / bind 失败残留）→ 清场后走 C 冷启动
                clear_instance(app);
            }
        }
    } else {
        // 无 PID 记录（历史遗留）：按进程名清本项目孤儿
        perf_log("ensure B段 无实例记录 → 清理本项目遗留识别服务孤儿");
        kill_our_orphans();
    }

    // C. 冷启动：冷门段端口；加载慢时「不杀进程」，落实例供下次收养继续等
    let mut last_err: Option<String> = None;
    for attempt in 1..=3 {
        let port = pick_cold_port();
        if port == 0 {
            perf_log(&format!("ensure C段 第{attempt}轮：无可用冷门端口"));
            continue;
        }
        let base = format!("http://127.0.0.1:{port}");
        let pid = spawn_server(app, port)?;
        perf_log(&format!("ensure C段 第{attempt}轮 冷启动 port={port} pid={pid}"));
        // 等 800ms：bind 失败的 uvicorn 会立刻退出
        tokio::time::sleep(Duration::from_millis(800)).await;
        if !pid_alive(pid) {
            // 带上子进程输出尾部：Store 占位符会打印 "Python was not found…"，
            // 没有这一行就只能靠手动复现（本次排查的真实教训）
            let tail = child_output_tail();
            let err = if tail.is_empty() {
                "进程启动后立即退出（端口被占或运行时异常，且子进程无输出）".to_string()
            } else {
                format!("进程启动后立即退出（端口被占或运行时异常）。子进程输出：\n{tail}")
            };
            last_err = Some(err.clone());
            perf_log(&format!("ensure C段 第{attempt}轮 pid={pid} 启动后立即退出 | {err}"));
            continue;
        }
        match wait_service(client, &base, wait_models).await {
            Ok(()) => {
                set_base(base.clone());
                save_instance(app, pid, port);
                perf_log(&format!(
                    "ensure 完成（C段冷启动成功）| base={base} pid={pid} wait_models={wait_models} 耗时 {}ms",
                    t0.elapsed().as_millis()
                ));
                return Ok(());
            }
            Err(timeout_err) => {
                if !pid_alive(pid) {
                    last_err = Some(timeout_err);
                    continue;
                }
                let (probe, digest) = probe_base(client, &base).await;
                // 区分「加载中超时」（进程保留供收养继续等）与「拉起的组件仍是
                // 旧版本」：后者重启一万次也不会变新，登记组件并终止，否则形成
                // 「杀→拉→杀」死循环（十几个 vcr-server.exe 连环生灭的资源灾难）
                if probe == HealthProbe::OldVersion {
                    kill_pid(pid);
                    clear_instance(app);
                    set_stale_component(spawn_target_path(app));
                    perf_log(&format!(
                        "ensure 失败：拉起的组件仍是旧版本（登记为过期）| {digest} | {}",
                        spawn_target_path(app).display()
                    ));
                    return Err(stale_component_err(&spawn_target_path(app)));
                }
                // 进程仍存活 = 正在加载模型 → 保留进程，落实例供收养，提示稍候
                set_base(base.clone());
                save_instance(app, pid, port);
                perf_log(&format!(
                    "ensure 未就绪但进程存活（加载中，实例保留供收养）| base={base} pid={pid} 耗时 {}ms | {timeout_err} | {digest}",
                    t0.elapsed().as_millis()
                ));
                return Err(timeout_err);
            }
        }
    }
    let err = last_err.unwrap_or_else(|| "识别服务启动失败".into());
    perf_log(&format!(
        "ensure 失败（冷启动重试耗尽）| 耗时 {}ms | {err}",
        t0.elapsed().as_millis()
    ));
    Err(err)
}

/// 语义通道宽松就绪超时：仅需服务进程可响应（CLIP 由 /embed_text 触发懒加载）
const ALIVE_TIMEOUT: Duration = Duration::from_secs(25);

/// FEAT-SEM：语义服务最近一次不可用时刻（退避窗口）
///
/// 重启后首次语义搜索需拉起服务 + 触发 CLIP 懒加载（数秒），这是预期开销；
/// 但「模型未下载 / 服务起不来」这类确定性失败若每次都重试，会让每次搜索都白等
/// 数秒并反复拉起进程，故失败后进入退避窗口（TTL 内直接降级纯关键词）。
static SEMANTIC_DOWN_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// 退避窗口时长。
///
/// 原为 180s，是把「瞬时抖动」与「确定性失败」混为一谈的代价：重启后 CLIP 首次
/// 加载期间的**一次** 503，会让之后整整 3 分钟内**所有**搜索静默降级成纯关键词，
/// 用户看到的现象就是「向量明明在库里，却一条都搜不出来」（BUG-2026-0920-008）。
/// 而真正需要长窗口保护的确定性失败（模型缺失 / 端口不通 / 组件过期）已由
/// `clip_model_present()` 与实例登记廉价拦截，不必靠长退避兜。故压到 20s：
/// 既避开瞬时抖动期的连续硬等，又保证服务恢复后最多 20s 内自动回归。
const SEMANTIC_DOWN_TTL: Duration = Duration::from_secs(20);

/// 可重试错误的标记前缀：`embed_*` 判定为「服务未就绪 / 模型加载中」时返回。
///
/// 调用方据此区分「本次跳过」与「进入退避」——只有确定性失败才进退避，
/// 自愈型失败（503 加载中）绝不能把整条语义链路锁死数十秒。
const RETRYABLE_PREFIX: &str = "[retryable]";

/// 错误是否属于「可重试」（服务加载中 / 未就绪这类会自愈的失败）
pub fn is_retryable_err(e: &str) -> bool {
    e.starts_with(RETRYABLE_PREFIX)
}

/// 是否应因该错误进入退避窗口（可重试错误只跳过本次，不退避）
pub fn should_enter_backoff(e: &str) -> bool {
    !is_retryable_err(e)
}

/// 记一次语义调用失败：可重试型（服务加载中/未就绪）只跳过本次，确定性失败才进退避
///
/// 收敛到单一入口，避免三个调用点（图像语义 / 描述语义 / 以图搜图）各自为政——
/// 「任何失败都 mark_semantic_down」正是 BUG-2026-0920-008 的成因。
pub fn note_semantic_failure(e: &str) {
    if should_enter_backoff(e) {
        mark_semantic_down();
    }
}

/// 退避窗口剩余时长（未在退避返回 None）
pub fn semantic_backoff_remaining() -> Option<Duration> {
    SEMANTIC_DOWN_AT
        .lock()
        .ok()
        .and_then(|g| *g)
        .and_then(|t| SEMANTIC_DOWN_TTL.checked_sub(t.elapsed()))
}

/// 语义服务是否处于退避窗口
pub fn semantic_backoff_active() -> bool {
    semantic_backoff_remaining().is_some()
}

/// 标记语义不可用（进入退避窗口）
pub fn mark_semantic_down() {
    if let Ok(mut g) = SEMANTIC_DOWN_AT.lock() {
        *g = Some(Instant::now());
    }
}

/// 清除退避（一次成功调用后立即恢复可用）
pub fn clear_semantic_down() {
    if let Ok(mut g) = SEMANTIC_DOWN_AT.lock() {
        *g = None;
    }
}

/// 语义链路可用性快照（前端据此给出**分档**提示，而不是笼统的一句「模型未下载」）
///
/// 背景（BUG-2026-0920-006/007）：前端此前只拿到 `warmup_semantic_service -> bool`，
/// 任何失败都渲染成同一句「CLIP 模型未下载」；而真实原因可能是退避中、服务没起来、
/// 或索引与当前模型档位不匹配。用户照提示去「下载模型」，当然解决不了问题。
#[derive(Debug, Clone, Serialize)]
pub struct SemanticStatus {
    /// 是否可用
    pub ready: bool,
    /// 机器可读原因码：ready / model_missing / backoff / service_unreachable
    pub code: String,
    /// 面向用户的说明（说清「发生了什么 + 怎么办」）
    pub message: String,
    /// 退避剩余毫秒（0 = 未在退避）
    pub backoff_remaining_ms: u64,
}

// ---------------------------------------------------------------------------
// embedding 端点错误分类（BUG-2026-0920-008）
//
// 原实现直接 `.json()` 解析响应体：FastAPI 的 503 `{"detail": "..."}` 是一个**合法
// JSON**，于是被成功解析，随后落到 `resp.get("results") == None`，真实原因被
// 「embedding 响应缺少 results」整句覆盖——app.log 里再也看不出到底是「没就绪」
// 还是「模型缺失」。此处改为**先判 HTTP 状态**，把 detail 原样带出并落日志。
// ---------------------------------------------------------------------------

/// 从 FastAPI 错误体里取 `detail`（取不到退回原始文本；压平换行并截断防刷屏）
fn extract_detail(body: &str) -> String {
    let txt = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("detail")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| body.trim().to_string());
    let flat = txt.replace(['\r', '\n'], " ");
    if flat.chars().count() > 300 {
        flat.chars().take(300).collect::<String>() + "…"
    } else {
        flat
    }
}

/// 该失败是否属于「服务未就绪 / 模型加载中」这类可自愈的失败
///
/// 判据：503（服务暂不可用）本身即视为可重试；其余状态码需错误体里有明确的加载
/// 语义（中英文）才按可重试处理——避免把「模型缺失」这类确定性错误也当成抖动，
/// 否则会陷入「每次搜索都白等 7 秒」的反向坑。
fn is_not_ready(status: reqwest::StatusCode, detail: &str) -> bool {
    let d = detail.to_lowercase();
    let loading = d.contains("未就绪")
        || d.contains("加载中")
        || d.contains("正在加载")
        || d.contains("初始化")
        || d.contains("not ready")
        || d.contains("loading")
        || d.contains("initializ");
    loading || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
}

/// 把「非 2xx 响应」翻译成明确错误（可重试型带 `RETRYABLE_PREFIX` 前缀）
fn http_failure_err(status: reqwest::StatusCode, detail: &str) -> String {
    if is_not_ready(status, detail) {
        format!("{RETRYABLE_PREFIX} 语义服务未就绪（HTTP {status}）：{detail}")
    } else {
        format!("语义服务调用失败（HTTP {status}）：{detail}")
    }
}

/// 目录是否可写（试写探针：MSI 装到 Program Files 时普通权限进程不可写）
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".pms-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 把只读目录树「链接」到可写目录（硬链接优先=零拷贝；跨卷/失败退回真实复制）
///
/// 已存在且大小相同的文件跳过，因此可重复调用；应用后续新增的文件
/// （clip_vision.onnx / clip_text.onnx / current_clip.json 等）天然落在可写侧。
fn link_or_copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            link_or_copy_tree(&from, &to)?;
            continue;
        }
        let src_len = entry.metadata().map(|m| m.len()).ok();
        let same = match (std::fs::metadata(&to), src_len) {
            (Ok(m), Some(n)) => m.len() == n,
            _ => false,
        };
        if same {
            continue;
        }
        let _ = std::fs::remove_file(&to);
        if std::fs::hard_link(&from, &to).is_err() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 解析「模型目录」（全应用唯一口径）
///
/// 1. 环境变量 `VCR_MODEL_DIR`（已设置则直接采用，便于调试 / 自定义部署）
/// 2. 打包版内置资源 `resource_dir/vcr/models`：
///    - 可写（NSIS 默认按用户安装）→ 直接用它；
///    - 只读（MSI 默认装到 Program Files）→ 在 `app_data_dir/vcr-models` 建
///      硬链接 / 复制副本并改用副本——语义子图拆分、档位持久化、应用内模型
///      下载都要写模型目录，只读目录会让这些功能直接失败。
/// 3. 开发态 → 源码目录 `python/models`
pub fn resolve_model_dir(app: &tauri::AppHandle) -> PathBuf {
    if let Ok(v) = std::env::var("VCR_MODEL_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(res) = app.path().resource_dir() {
        let bundled = res.join("vcr").join("models");
        if bundled.is_dir() {
            if dir_writable(&bundled) {
                return bundled;
            }
            if let Ok(data) = app.path().app_data_dir() {
                let cache = data.join("vcr-models");
                match link_or_copy_tree(&bundled, &cache) {
                    Ok(()) => {
                        eprintln!("[vcr] 内置模型目录只读，已就绪可写副本: {}", cache.display());
                        return cache;
                    }
                    Err(e) => eprintln!(
                        "[vcr] 模型副本创建失败（{e}），退回只读内置目录 {}",
                        bundled.display()
                    ),
                }
            }
            return bundled;
        }
    }
    project_python_dir().join("models")
}

/// CLIP 模型文件是否已下载（未下载则不预热服务，避免无谓进程与等待）
///
/// 目录口径与 `model_dl::models_dir` 一致：VCR_MODEL_DIR 优先（由 lib.rs::setup
/// 按 vision::resolve_model_dir 写入进程环境变量），缺省回落源码目录 python/models。
pub fn clip_model_present() -> bool {
    let dir = std::env::var("VCR_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| project_python_dir().join("models"));
    dir.join("chinese-clip").join("onnx").join("model_fp16.onnx").is_file()
        && dir.join("chinese-clip").join("tokenizer.json").is_file()
}

/// FEAT-SEM：语义服务预热 —— 拉起服务并触发 CLIP 懒加载（搜索页进入时后台调用）
///
/// 与 `clip_ready()` 的区别：后者只探测「已运行且 CLIP 已加载」（不启动、不触发），
/// 重启后必然为 false 导致搜索静默降级；本函数走宽松就绪 + 真实编码一次，
/// 让用户真正输入搜索词时 CLIP 已就绪（首次搜索也快）。
pub async fn warmup_clip(app: &tauri::AppHandle) -> Result<(), String> {
    if !clip_model_present() {
        return Err("CLIP 模型未下载".into());
    }
    let _ = embed_text_query("预热", app).await?;
    Ok(())
}

/// 轮询「服务进程可响应」（不等模型就绪）：语义链路/配置端点专用
///
/// 日志：每 5s 打一行进度（含 /health 摘要），超时给出最后一次摘要——
/// 面板/语义链路卡住时能直接看出「卡在哪一步、ok 为何为 false」。
async fn poll_alive(client: &reqwest::Client, base: &str, timeout: Duration) -> Result<(), String> {
    let t0 = Instant::now();
    let deadline = t0 + timeout;
    let mut last_digest = String::new();
    let mut last_log = t0;
    perf_log(&format!(
        "poll_alive 开始 | base={base} 超时 {}s（只等进程可达，不等模型）",
        timeout.as_secs()
    ));
    loop {
        let (probe, digest) = probe_base(client, base).await;
        if !digest.is_empty() {
            last_digest = digest;
        }
        match probe {
            // Ready = 主链路模型也已就绪；Loading = 服务可达但模型仍在加载，
            // 两者对语义链路都算可用（/embed_text 会自行触发 CLIP 加载）
            HealthProbe::Ready | HealthProbe::Loading => {
                perf_log(&format!(
                    "poll_alive 就绪 | {} | 耗时 {}ms | {last_digest}",
                    probe.label(),
                    t0.elapsed().as_millis()
                ));
                return Ok(());
            }
            HealthProbe::OldVersion => {
                perf_log(&format!("poll_alive 失败：服务为旧版本 | {last_digest}"));
                return Err("识别服务为旧版本（缺语义端点），请结束该进程后重试".into());
            }
            HealthProbe::NotReachable => {}
        }
        if Instant::now() > deadline {
            perf_log(&format!(
                "poll_alive 超时 | 耗时 {}ms | {last_digest}",
                t0.elapsed().as_millis()
            ));
            return Err("识别服务启动超时（端口未响应）".into());
        }
        if last_log.elapsed() >= Duration::from_secs(5) {
            last_log = Instant::now();
            perf_log(&format!(
                "poll_alive 等待中… {}ms | {last_digest}",
                t0.elapsed().as_millis()
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 按用途等待服务：识别链路需模型就绪（classify 依赖 cls），语义链路只需进程可达
async fn wait_service(
    client: &reqwest::Client,
    base: &str,
    wait_models: bool,
) -> Result<(), String> {
    if wait_models {
        poll_ready(client, base, READY_TIMEOUT).await
    } else {
        poll_alive(client, base, ALIVE_TIMEOUT).await
    }
}

/// 轮询就绪；Loading 继续等，OldVersion 报版本错误，超时报错
///
/// 日志：每 5s 打一行进度（含 /health 摘要）。性能设置面板「检测中…一直转圈」
/// 的现场就在这里——超时行会写明 det/face/ocr/clip 就绪位与 load_errors。
async fn poll_ready(
    client: &reqwest::Client,
    base: &str,
    timeout: Duration,
) -> Result<(), String> {
    let t0 = Instant::now();
    let deadline = t0 + timeout;
    let mut last_digest = String::new();
    let mut last_log = t0;
    perf_log(&format!(
        "poll_ready 开始 | base={base} 超时 {}s（等主链路模型就绪）",
        timeout.as_secs()
    ));
    loop {
        let (probe, digest) = probe_base(client, base).await;
        if !digest.is_empty() {
            last_digest = digest;
        }
        match probe {
            HealthProbe::Ready => {
                perf_log(&format!(
                    "poll_ready 就绪 | 耗时 {}ms | {last_digest}",
                    t0.elapsed().as_millis()
                ));
                return Ok(());
            }
            HealthProbe::OldVersion => {
                perf_log(&format!("poll_ready 失败：服务为旧版本 | {last_digest}"));
                return Err("识别服务为旧版本（缺 GPU/模型切换端点），请结束该进程后重试".into());
            }
            _ => {}
        }
        if Instant::now() > deadline {
            // 进程仍在加载模型：不杀（下次 ensure 会收养继续等），提示稍候
            perf_log(&format!(
                "poll_ready 超时 | 耗时 {}ms | {last_digest}",
                t0.elapsed().as_millis()
            ));
            return Err("识别服务正在加载模型（大模型首次加载较慢），请稍候重试".into());
        }
        if last_log.elapsed() >= Duration::from_secs(5) {
            last_log = Instant::now();
            perf_log(&format!(
                "poll_ready 等待中… {}ms | {last_digest}",
                t0.elapsed().as_millis()
            ));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/* ─────────────── 开发模式：解释器解析 + 子进程输出捕获 ─────────────── */

/// 开发模式识别服务子进程的最近输出（stdout+stderr 合并，保留末 N 行）。
///
/// 为什么需要：spawn 时子进程输出默认丢弃，服务「启动后立即退出」时只剩一句
/// 「端口被占或运行时异常」，看不到真正原因。实测踩过：PATH 上第一个 `python`
/// 是 Microsoft Store「应用执行别名」占位符，spawn 成功但立刻退出并打印
/// `Python was not found…`，日志里却只有「立即退出」，排查全靠手动复现。
/// 现在子进程输出同时写日志并留一份尾部快照，出错时直接带回错误信息。
static CHILD_OUTPUT: LazyLock<Mutex<VecDeque<String>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
const CHILD_OUTPUT_KEEP: usize = 40;

fn clear_child_output() {
    if let Ok(mut g) = CHILD_OUTPUT.lock() {
        g.clear();
    }
}

fn push_child_output(line: &str) {
    if let Ok(mut g) = CHILD_OUTPUT.lock() {
        g.push_back(line.to_string());
        while g.len() > CHILD_OUTPUT_KEEP {
            g.pop_front();
        }
    }
}

/// 最近一次冷启动的输出尾部（无输出返回空串）
fn child_output_tail() -> String {
    CHILD_OUTPUT
        .lock()
        .map(|g| g.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

/// 把子进程 stdout/stderr 逐行写进日志与环形缓冲
fn attach_child_output(stdout: Option<std::process::ChildStdout>, stderr: Option<std::process::ChildStderr>) {
    let streams = [
        stdout.map(|s| Box::new(s) as Box<dyn Read + Send>),
        stderr.map(|s| Box::new(s) as Box<dyn Read + Send>),
    ];
    for stream in streams.into_iter().flatten() {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let mut stream = stream;
            let reader = BufReader::new(&mut stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let line = line.trim_end();
                if line.is_empty() {
                    continue;
                }
                push_child_output(line);
                crate::logger::log_info(&format!("[VCR:py] {line}"));
            }
        });
    }
}

/// 探针：一次性确认候选**既是真解释器、又带识别服务依赖**。
/// - Microsoft Store 占位符 → 非零退出（"Python was not found…"）
/// - 系统 python / anaconda / C:\Python312 → `ModuleNotFoundError: fastapi`
/// - 项目 venv → 打印 sys.executable 且 0 退出
const INTERP_PROBE: &str = "import sys, fastapi, uvicorn; print(sys.executable)";
const INTERP_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// 解释器解析结果缓存：成功永久有效；失败保留 30s。
/// 失败缓存的意义：服务起不来时性能面板会反复探测，不缓存就会每次都把
/// 候选解释器挨个探一遍（实测每次数秒），面板看起来像卡死。30s 后允许重试，
/// 用户修好环境不必重启应用。
static INTERP_CACHE: LazyLock<Mutex<Option<(Instant, Result<PathBuf, String>)>>> =
    LazyLock::new(|| Mutex::new(None));
const INTERP_FAIL_TTL: Duration = Duration::from_secs(30);

/// 开发模式候选解释器（按优先级）：项目自带 venv → PATH 上的 python / python3
fn python_candidates() -> Vec<PathBuf> {
    let py = project_python_dir();
    let mut v = Vec::new();
    #[cfg(target_os = "windows")]
    {
        v.push(py.join(".venv-vcr").join("Scripts").join("python.exe"));
        v.push(py.join(".venv").join("Scripts").join("python.exe"));
    }
    #[cfg(not(target_os = "windows"))]
    {
        v.push(py.join(".venv-vcr").join("bin").join("python3"));
        v.push(py.join(".venv").join("bin").join("python3"));
    }
    v.push(PathBuf::from("python"));
    v.push(PathBuf::from("python3"));
    v
}

/// 跑一次探针；成功返回子进程 stdout（内含 sys.executable）
fn probe_interpreter(cand: &Path) -> Result<String, String> {
    if cand.is_absolute() && !cand.is_file() {
        return Err("文件不存在".into());
    }
    let mut cmd = std::process::Command::new(cand);
    cmd.arg("-c")
        .arg(INTERP_PROBE)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = cmd.spawn().map_err(|e| format!("无法执行: {e}"))?;
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
    let deadline = Instant::now() + INTERP_PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(e) => return Err(format!("等待探针进程失败: {e}")),
        }
    };
    for r in readers {
        let _ = r.join();
    }
    let text = buf.lock().map(|g| g.trim().to_string()).unwrap_or_default();
    match status {
        Some(st) if st.success() => Ok(text),
        Some(st) => Err(if text.is_empty() {
            format!("探针非零退出（code={:?}，无输出）", st.code())
        } else {
            first_nonempty_line(&text)
        }),
        None => Err(format!("探针超时（{}s）", INTERP_PROBE_TIMEOUT.as_secs())),
    }
}

fn first_nonempty_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// 解析开发模式解释器。**不能直接用 PATH 上的 `python`**：
/// Windows「应用执行别名」会把它指向 Microsoft Store 占位符（spawn 成功、
/// 运行即退出），这台机器上 PATH 顺序恰好是 占位符 → anaconda → Python312，
/// 三者要么秒退要么没有 fastapi，只有项目 venv 可用。
/// 失败时返回「试过哪些候选、各自为什么不行」的完整说明。
/// （studio.rs 的工坊微服务同机同坑，复用本函数：BUG-2026-0918-007）
pub(crate) fn resolve_python_interpreter() -> Result<PathBuf, String> {
    if let Ok(g) = INTERP_CACHE.lock() {
        if let Some((at, res)) = g.as_ref() {
            if res.is_ok() || at.elapsed() < INTERP_FAIL_TTL {
                return res.clone();
            }
        }
    }
    let mut reasons: Vec<String> = Vec::new();
    let mut chosen: Option<PathBuf> = None;
    for cand in python_candidates() {
        match probe_interpreter(&cand) {
            Ok(exe) => {
                perf_log(&format!(
                    "解释器解析成功 | 候选={} | sys.executable={exe}",
                    cand.display()
                ));
                chosen = Some(cand);
                break;
            }
            Err(e) => reasons.push(format!("{}：{e}", cand.display())),
        }
    }
    let result = match chosen {
        Some(p) => Ok(p),
        None => Err(format!(
            "找不到可用的 Python 解释器（识别服务依赖 fastapi/uvicorn）。已尝试：\n  - {}\n\
             提示：开发模式请使用项目自带虚拟环境 python\\.venv-vcr（由 setup-env.ps1 / \
             build_vcr_exe.ps1 创建）。若系统 `python` 指向 Microsoft Store 占位符，\
             请在「设置 → 应用 → 高级应用设置 → 应用执行别名」里关闭 python.exe / python3.exe。",
            reasons.join("\n  - ")
        )),
    };
    if let Ok(mut g) = INTERP_CACHE.lock() {
        *g = Some((Instant::now(), result.clone()));
    }
    result
}

/// 启动识别微服务（打包版优先内置 exe；开发版 python server.py）。
/// 返回进程 PID（供实例落盘/清场）。
fn spawn_server(app: &tauri::AppHandle, port: u16) -> Result<u32, String> {
    // 解析资源/数据目录（打包版定位依赖这两个路径）
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("获取资源目录失败: {e}"))?;
    // 数据目录（人物库 persons.db）：优先取 lib.rs::setup 已解析的 VCR_DATA_DIR，
    // 使安装版与开发版统一到 app_data_dir/vcr-data（persons.rs 读写同一口径）；
    // 仅当环境变量缺失（异常启动路径）时才现算。
    let data_dir = match std::env::var("VCR_DATA_DIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => app
            .path()
            .app_data_dir()
            .map_err(|e| format!("获取应用数据目录失败: {e}"))?
            .join("vcr-data"),
    };
    let bundled_dir = resource_dir.join("vcr");
    let bundled_exe = bundled_dir.join("vcr-server.exe");

    let mut cmd: std::process::Command;
    let mut workdir = bundled_dir.clone(); // 打包版默认工作目录

    if bundled_exe.is_file() {
        // 打包版：直接启动内置单文件 exe
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd = std::process::Command::new(&bundled_exe);
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW（隐藏控制台窗口）
        }
        #[cfg(not(target_os = "windows"))]
        {
            cmd = std::process::Command::new(&bundled_exe);
        }
        // 模型目录由 lib.rs::setup 统一解析（内置目录只读时指向 app_data 下的可写副本）；
        // 人物库目录同样统一（VCR_DATA_DIR，见 setup）——二者均写盘在 app_data
        let model_dir = std::env::var("VCR_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| bundled_dir.join("models"));
        cmd.env("VCR_MODEL_DIR", &model_dir);
        cmd.env("VCR_DATA_DIR", &data_dir);
    } else {
        // 开发版：python server.py
        let server_script = project_python_dir().join("server.py");
        if !server_script.is_file() {
            return Err(format!("识别服务脚本不存在: {}", server_script.display()));
        }
        let py_dir = project_python_dir();
        workdir = py_dir.clone();
        // 不能用裸 `python`（PATH 上常被 Store 占位符/无依赖解释器占据）
        let interp = resolve_python_interpreter()?;
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd = std::process::Command::new(&interp);
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        #[cfg(not(target_os = "windows"))]
        {
            cmd = std::process::Command::new(&interp);
        }
        cmd.arg(&server_script);
    }

    // 动态端口：本次分配的端口经环境变量传给服务
    cmd.env("VCR_PORT", port.to_string());
    // 捕获子进程输出：否则「启动后立即退出」查无实据（见 CHILD_OUTPUT 注释）
    clear_child_output();
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .current_dir(&workdir)
        .spawn()
        .map_err(|e| format!(
            "启动识别服务失败（打包版请确认安装目录 vcr/vcr-server.exe 存在；开发版请 pip install -r python/requirements.txt）: {e}"
        ))?;
    attach_child_output(child.stdout.take(), child.stderr.take());
    Ok(child.id())
}

/// （人物列表/重命名/合并/头像已迁移到 `persons` 模块直读 persons.db；此处仅保留删除代理）

/// GPU 加速可行性状态（R3，来自微服务 /gpu）
#[derive(Debug, Clone, Serialize)]
pub struct VcrGpuStatus {
    /// 服务是否在运行
    pub running: bool,
    /// 当前是否实际走 GPU 推理
    pub use_gpu: bool,
    /// 当前选中的提供方（如 DmlExecutionProvider / CPUExecutionProvider）
    pub provider: String,
    /// 检测到的 GPU 提供方列表
    pub gpu: Vec<String>,
    /// 全部可用提供方
    pub available: Vec<String>,
    /// 批次安全上限
    pub batch_max: usize,
    /// FEAT-051：是否被用户强制 CPU（前端开关初始状态）
    #[serde(default)]
    pub forced_cpu: bool,
    /// FEAT-053：各通道会话实测 provider（sess.get_providers()，未加载通道不出现）。
    /// use_gpu/provider 是「请求值」，这里是「实测值」—— 两者对照才算真验证。
    #[serde(default)]
    pub sessions: std::collections::HashMap<String, Vec<String>>,
}

/// 性能设置端点专用 ensure：只等「服务进程可达」，**不等检测模型就绪**。
///
/// 为什么必须分开（BUG-2026-0921-003）：
///   `/gpu` `/models` `/threads` 是配置级端点，服务进程一监听端口就能回答；
///   而 `/health.ok` 依赖检测通道加载成功。此前这里传 wait_models=true，一旦
///   det 加载失败/极慢，面板三个请求会串在 ENSURE_LOCK 上各等 90s，UI 表现就是
///   「一直转圈、不能选模型和线程」。
async fn ensure_perf_ready(client: &reqwest::Client, app: &tauri::AppHandle) -> Result<(), String> {
    ensure_service_ready(client, app, false).await
}

/// GPU 状态一行摘要（日志用）
pub fn gpu_brief(s: &VcrGpuStatus) -> String {
    let sessions: Vec<String> = s
        .sessions
        .iter()
        .map(|(k, v)| format!("{k}:[{}]", v.join("+")))
        .collect();
    format!(
        "running={} use_gpu={} provider={} gpu=[{}] available=[{}] forced_cpu={} sessions={{{}}}",
        s.running,
        s.use_gpu,
        s.provider,
        s.gpu.join(","),
        s.available.join(","),
        s.forced_cpu,
        sessions.join(" ")
    )
}

/// 语义模型清单一行摘要（日志用）
pub fn models_brief(v: &serde_json::Value) -> String {
    let current = v.get("current").and_then(|x| x.as_str()).unwrap_or("?");
    let ready = v.get("clip_ready").and_then(|x| x.as_bool()).unwrap_or(false);
    let names: Vec<String> = v
        .get("models")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let n = m.get("name")?.as_str()?;
                    let dl = m.get("downloaded").and_then(|x| x.as_bool()).unwrap_or(false);
                    let act = m.get("active").and_then(|x| x.as_bool()).unwrap_or(false);
                    Some(format!(
                        "{n}{}{}",
                        if dl { "·已下载" } else { "·未下载" },
                        if act { "·当前" } else { "" }
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    format!(
        "current={current} clip_ready={ready} models=[{}]",
        names.join(", ")
    )
}

/// 线程数一行摘要（日志用）
pub fn threads_brief(v: &serde_json::Value) -> String {
    let n = |k: &str| v.get(k).and_then(|x| x.as_i64()).map(|x| x.to_string()).unwrap_or_else(|| "?".into());
    let opts = v
        .get("options")
        .and_then(|x| x.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let applied = v
        .get("applied")
        .and_then(|x| x.as_i64())
        .map(|x| format!(" applied={x}"))
        .unwrap_or_default();
    format!(
        "threads={} default={} physical={} logical={} options={opts}档{applied}",
        n("threads"),
        n("default"),
        n("physical_guess"),
        n("logical")
    )
}

/// 测速结果一行摘要（日志用）
pub fn bench_brief(v: &serde_json::Value) -> String {
    let f = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_f64())
            .map(|x| format!("{x}"))
            .or_else(|| v.get(k).and_then(|x| x.as_i64()).map(|x| x.to_string()))
            .unwrap_or_else(|| "?".into())
    };
    let provs = v
        .get("providers")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join("+")
        })
        .unwrap_or_default();
    format!(
        "channel={} threads={} avg={}ms min={}ms runs={} providers=[{provs}]",
        v.get("channel").and_then(|x| x.as_str()).unwrap_or("?"),
        f("threads"),
        f("avg_ms"),
        f("min_ms"),
        f("runs")
    )
}

/// 扫档结果一行摘要（日志用）
pub fn sweep_brief(v: &serde_json::Value) -> String {
    let results = v.get("results").and_then(|x| x.as_array());
    let Some(arr) = results else {
        return "results=?".into();
    };
    let errors = arr.iter().filter(|r| r.get("error").is_some()).count();
    let best = arr.iter().find(|r| r.get("best").and_then(|x| x.as_bool()) == Some(true));
    let best_s = best
        .map(|b| {
            format!(
                "best={}线程/{}ms",
                b.get("threads").and_then(|x| x.as_i64()).unwrap_or(0),
                b.get("avg_ms").and_then(|x| x.as_f64()).unwrap_or(0.0)
            )
        })
        .unwrap_or_else(|| "best=无".into());
    format!("档位 {} 个·失败 {errors} | {best_s}", arr.len())
}

/// 查询 GPU 加速可行性：确保服务进程可达后请求 /gpu（不等检测模型就绪）
pub async fn vcr_gpu_status(app: &tauri::AppHandle) -> Result<VcrGpuStatus, String> {
    let client = http_client().await?;
    ensure_perf_ready(&client, app).await?;
    let base = vcr_base();
    let resp: serde_json::Value = client
        .get(format!("{base}/gpu"))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("解析结果失败: {e}"))?;
    let s = gpu_status_from_value(&resp, true);
    perf_log(&format!("get_gpu_status | base={base} | {}", gpu_brief(&s)));
    Ok(s)
}

/// /gpu 响应 → VcrGpuStatus 映射（探测与切换共用）
fn gpu_status_from_value(resp: &serde_json::Value, running: bool) -> VcrGpuStatus {
    let gpu = resp
        .get("gpu")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let available = resp
        .get("available")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    VcrGpuStatus {
        running,
        use_gpu: resp.get("use_gpu").and_then(|v| v.as_bool()).unwrap_or(false),
        provider: resp.get("provider").and_then(|v| v.as_str()).unwrap_or("cpu").to_string(),
        gpu,
        available,
        batch_max: resp.get("batch_max").and_then(|v| v.as_u64()).unwrap_or(8) as usize,
        forced_cpu: resp.get("forced_cpu").and_then(|v| v.as_bool()).unwrap_or(false),
        sessions: resp
            .get("sessions")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .map(|(k, v)| {
                        let list = v
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        (k.clone(), list)
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// FEAT-051：GPU 加速开关（开 = GPU 优先 / 关 = 强制 CPU），返回切换后状态
pub async fn vcr_set_gpu(app: &tauri::AppHandle, enabled: bool) -> Result<VcrGpuStatus, String> {
    let t0 = Instant::now();
    perf_log(&format!("set_gpu 请求 | enabled={enabled}"));
    let client = http_client().await?;
    ensure_perf_ready(&client, app).await?;
    let base = vcr_base();
    let resp = client
        .post(format!("{base}/gpu"))
        .json(&serde_json::json!({ "enabled": enabled }))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析结果失败: {e}"))?;
    if !status.is_success() {
        // 旧版本服务只有 GET /gpu → POST 会 405：提示重启
        let detail = v.get("detail").and_then(|x| x.as_str()).unwrap_or("切换失败");
        perf_log(&format!("set_gpu 失败 | HTTP {} | {detail}", status.as_u16()));
        return Err(format!(
            "{detail}（若为 Method Not Allowed，说明识别服务是旧版本，请重启应用自动升级）"
        ));
    }
    let s = gpu_status_from_value(&v, true);
    perf_log(&format!(
        "set_gpu 完成 | 耗时 {}ms | {}",
        t0.elapsed().as_millis(),
        gpu_brief(&s)
    ));
    Ok(s)
}

/// FEAT-051：语义模型档位候选清单（含是否已下载 / 当前生效 / 会话实测事实）
pub async fn vcr_list_models(app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    let client = http_client().await?;
    ensure_perf_ready(&client, app).await?;
    let base = vcr_base();
    let resp = client
        .get(format!("{base}/models"))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析结果失败: {e}"))?;
    if !status.is_success() {
        perf_log(&format!("list_models 失败 | HTTP {}", status.as_u16()));
        return Err(format!(
            "识别服务不含模型清单端点（服务版本过旧，请重启应用）: HTTP {}",
            status.as_u16()
        ));
    }
    perf_log(&format!("list_models | base={base} | {}", models_brief(&v)));
    Ok(v)
}

/// FEAT-051：切换语义模型档位（文件未下载 / 未知档位 → 提取服务端 detail 报错）
pub async fn vcr_set_model(app: &tauri::AppHandle, model: &str) -> Result<serde_json::Value, String> {
    let t0 = Instant::now();
    perf_log(&format!("set_model 请求 | model={model}"));
    let client = http_client().await?;
    ensure_perf_ready(&client, app).await?;
    let base = vcr_base();
    let resp = client
        .post(format!("{base}/model"))
        .json(&serde_json::json!({ "name": model }))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析结果失败: {e}"))?;
    if !status.is_success() {
        let detail = v
            .get("detail")
            .and_then(|x| x.as_str())
            .unwrap_or("切换失败");
        perf_log(&format!("set_model 失败 | HTTP {} | {detail}", status.as_u16()));
        return Err(detail.to_string());
    }
    perf_log(&format!(
        "set_model 完成（后台加载中）| 耗时 {}ms | {}",
        t0.elapsed().as_millis(),
        models_brief(&v)
    ));
    Ok(v)
}

/// v6：CPU 线程数现状（/threads）—— 当前/默认/物理核推测/可选档，供「性能设置」展示
pub async fn vcr_threads_status(app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    let client = http_client().await?;
    ensure_perf_ready(&client, app).await?;
    let base = vcr_base();
    let resp = client
        .get(format!("{base}/threads"))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("解析结果失败: {e}"))?;
    if !status.is_success() {
        perf_log(&format!("get_threads 失败 | HTTP {}", status.as_u16()));
        return Err(format!("识别服务不支持线程设置（版本过旧，请重启应用）: HTTP {}", status.as_u16()));
    }
    perf_log(&format!("get_threads | base={base} | {}", threads_brief(&v)));
    Ok(v)
}

/// v6：设置 CPU 线程数（越界由服务端夹紧）；切换后会后台重建会话
pub async fn vcr_set_threads(app: &tauri::AppHandle, threads: i64) -> Result<serde_json::Value, String> {
    let t0 = Instant::now();
    perf_log(&format!("set_threads 请求 | threads={threads}"));
    let client = http_client().await?;
    ensure_perf_ready(&client, app).await?;
    let base = vcr_base();
    let resp = client
        .post(format!("{base}/threads"))
        .json(&serde_json::json!({ "threads": threads }))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("解析结果失败: {e}"))?;
    if !status.is_success() {
        let detail = v.get("detail").and_then(|x| x.as_str()).unwrap_or("设置失败");
        perf_log(&format!("set_threads 失败 | HTTP {} | {detail}", status.as_u16()));
        return Err(detail.to_string());
    }
    perf_log(&format!(
        "set_threads 完成（后台重建会话）| 耗时 {}ms | {}",
        t0.elapsed().as_millis(),
        threads_brief(&v)
    ));
    Ok(v)
}

/// v6：线程数扫档（一次请求测多个档位；可能十几秒，用长超时）
pub async fn vcr_benchmark_sweep(
    app: &tauri::AppHandle,
    channel: &str,
    options: Vec<i64>,
    runs: u32,
    warmup: u32,
) -> Result<serde_json::Value, String> {
    let t0 = Instant::now();
    perf_log(&format!(
        "benchmark_sweep 请求 | channel={channel} options={options:?} runs={runs} warmup={warmup}"
    ));
    let client = http_client().await?;
    ensure_perf_ready(&client, app).await?;
    let base = vcr_base();
    let resp = client
        .post(format!("{base}/benchmark_sweep"))
        .timeout(Duration::from_secs(300))
        .json(&serde_json::json!({
            "channel": channel, "options": options, "runs": runs, "warmup": warmup
        }))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("解析结果失败: {e}"))?;
    if !status.is_success() {
        let detail = v.get("detail").and_then(|x| x.as_str()).unwrap_or("扫档失败");
        perf_log(&format!("benchmark_sweep 失败 | HTTP {} | {detail}", status.as_u16()));
        return Err(detail.to_string());
    }
    perf_log(&format!(
        "benchmark_sweep 完成 | 耗时 {}ms | {}",
        t0.elapsed().as_millis(),
        sweep_brief(&v)
    ));
    Ok(v)
}

/// FEAT-053：固定张量测速（CPU/GPU 真实加速比一键对比）。
/// 可能触发模型加载与数十次推理（秒级耗时），用每请求独立长超时（共享客户端 15s 不够）。
pub async fn vcr_benchmark(
    app: &tauri::AppHandle,
    runs: u32,
    warmup: u32,
    channel: &str,
) -> Result<serde_json::Value, String> {
    let t0 = Instant::now();
    perf_log(&format!(
        "benchmark 请求 | channel={channel} runs={runs} warmup={warmup}"
    ));
    let client = http_client().await?;
    ensure_perf_ready(&client, app).await?;
    let base = vcr_base();
    let resp = client
        .post(format!("{base}/benchmark"))
        .timeout(Duration::from_secs(60))
        .json(&serde_json::json!({ "runs": runs, "warmup": warmup, "channel": channel }))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析结果失败: {e}"))?;
    if !status.is_success() {
        let detail = v
            .get("detail")
            .and_then(|x| x.as_str())
            .unwrap_or("测速失败");
        perf_log(&format!("benchmark 失败 | HTTP {} | {detail}", status.as_u16()));
        return Err(detail.to_string());
    }
    perf_log(&format!(
        "benchmark 完成 | 耗时 {}ms | {}",
        t0.elapsed().as_millis(),
        bench_brief(&v)
    ));
    Ok(v)
}

async fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

/// Python 微服务目录（项目根/python）
fn project_python_dir() -> std::path::PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(|p| p.join("python"))
        .unwrap_or_else(|| manifest.join("python"))
}

/// 将微服务返回的单条结果解析为 VisionResult（兼容成功/失败两种形态）
fn parse_item(item: &serde_json::Value) -> VisionResult {
    let path = item
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let file_name = match item.get("file_name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };

    if let Some(err) = item.get("error").and_then(|v| v.as_str()) {
        return VisionResult {
            file_name,
            path,
            category: String::new(),
            sub_category: String::new(),
            label: String::new(),
            confidence: 0.0,
            top3: Vec::new(),
            person_ids: Vec::new(),
            person_count: 0,
            elapsed_ms: 0.0,
            error: Some(err.to_string()),
        };
    }

    let top3 = item
        .get("top3")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|t| VisionTopItem {
                    category: t
                        .get("category")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    label: t
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    confidence: t.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0),
                })
                .collect()
        })
        .unwrap_or_default();

    VisionResult {
        file_name,
        path,
        category: item
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("other")
            .to_string(),
        sub_category: item
            .get("sub_category")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        label: item
            .get("top3")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|t| t.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        confidence: item.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0),
        top3,
        person_ids: item
            .get("person_ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        person_count: item
            .get("person_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        elapsed_ms: item.get("elapsed_ms").and_then(|v| v.as_f64()).unwrap_or(0.0),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_images_skips_hidden() {
        let dir = format!("{}/../test_fixture_photos", env!("CARGO_MANIFEST_DIR"));
        if !Path::new(&dir).is_dir() {
            eprintln!("跳过：无测试图片目录 {dir}");
            return;
        }
        let photos = collect_images(&dir).expect("collect 应成功");
        // 6 张 fixture（.hidden 目录被跳过）
        assert_eq!(photos.len(), 6, "got: {photos:?}");
        assert!(photos.iter().all(|p| !p.contains(".hidden")));
    }

    #[test]
    fn test_project_python_dir() {
        let dir = project_python_dir();
        assert!(dir.ends_with("python"), "got: {}", dir.display());
        assert!(dir.join("server.py").exists(), "server.py 应存在");
    }

    /// BUG-2026-0918-002：候选解释器必须首选项目自带 venv。
    /// 直接用 PATH 上的 `python` 会踩 Microsoft Store 应用执行别名占位符。
    #[test]
    fn test_python_candidates_prefers_project_venv() {
        let c = python_candidates();
        let first = c[0].to_string_lossy().to_lowercase();
        assert!(
            first.contains(".venv-vcr"),
            "首个候选应为项目 venv，实际 {}",
            c[0].display()
        );
        assert!(c.iter().any(|p| p.to_string_lossy() == "python"));
    }

    /// 不存在的路径不能蒙混过关（否则会拿个不存在的 exe 去 spawn）
    #[test]
    fn test_probe_interpreter_rejects_missing_path() {
        let missing = project_python_dir()
            .join(".venv-vcr")
            .join("Scripts")
            .join("__definitely_not_here__.exe");
        assert!(probe_interpreter(&missing).is_err());
    }

    /// venv 存在时，解析结果必须就是它（本机验证；无 venv 的机器跳过）
    #[test]
    fn test_resolve_prefers_venv_when_present() {
        let venv = python_candidates()[0].clone();
        if !venv.is_file() {
            eprintln!("跳过：本机未创建 {}", venv.display());
            return;
        }
        let got = resolve_python_interpreter().expect("venv 存在时应能解析出解释器");
        assert_eq!(got, venv, "应优先选择项目自带 venv");
    }
}

/// FEAT-SEM 语义退避的回归测试（BUG-2026-0920-008）
#[cfg(test)]
mod semantic_backoff_tests {
    use super::*;

    /// 回归：CLIP **强制未就绪**（503「加载中」）必须判为可重试，不得进入退避窗口
    ///
    /// 旧实现「任何失败都 mark_semantic_down」，于是重启后 CLIP 首次加载期的一次 503
    /// 就把整条语义链路锁死 180s——表现正是「向量明明在库里，却一条都搜不出来」。
    #[test]
    fn not_ready_must_not_enter_backoff() {
        let zh = http_failure_err(reqwest::StatusCode::SERVICE_UNAVAILABLE, "CLIP 模型加载中");
        assert!(is_retryable_err(&zh), "503 加载中必须判为可重试: {zh}");
        assert!(!should_enter_backoff(&zh), "可重试错误不得进退避: {zh}");

        // 非 503 但带明确加载语义（英文）同样要认
        let en = http_failure_err(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "model loading");
        assert!(is_retryable_err(&en), "英文 loading 也必须认: {en}");

        // 端到端：强制未就绪后，语义链路必须仍可用
        clear_semantic_down();
        note_semantic_failure(&zh);
        assert!(
            !semantic_backoff_active(),
            "可重试失败之后不得处于退避中（CLIP 强制未就绪不得进长时间退避）"
        );
        assert!(semantic_backoff_remaining().is_none());
    }

    /// 确定性失败仍须进退避，否则每次搜索都要白等连接超时
    #[test]
    fn fatal_error_enters_backoff() {
        let missing = http_failure_err(reqwest::StatusCode::NOT_FOUND, "模型缺失");
        assert!(!is_retryable_err(&missing), "{missing}");
        assert!(should_enter_backoff(&missing));

        // 端口不通（连接层失败）不带可重试前缀 → 进退避
        let conn = "调用 embedding 服务失败: error sending request for url";
        assert!(should_enter_backoff(conn));

        clear_semantic_down();
        note_semantic_failure(&missing);
        assert!(semantic_backoff_active(), "确定性失败必须进退避");
        assert!(semantic_backoff_remaining().is_some());
        clear_semantic_down();
    }

    /// 退避窗口必须足够短：服务恢复后自动回归，而不是让用户干等数分钟
    #[test]
    fn backoff_ttl_is_short() {
        assert!(
            SEMANTIC_DOWN_TTL <= Duration::from_secs(30),
            "退避窗口过长 = 「一次抖动丢掉几分钟的语义检索」: {SEMANTIC_DOWN_TTL:?}"
        );
    }

    /// detail 提取：FastAPI 错误体取 detail / 非 JSON 退回原文 / 压平换行 / 超长截断
    #[test]
    fn extract_detail_handles_bodies() {
        assert_eq!(extract_detail(r#"{"detail":"模型缺失"}"#), "模型缺失");
        assert_eq!(extract_detail("plain text"), "plain text");
        // JSON 转义换行 → 解码后是真换行 → 压平成空格
        assert_eq!(extract_detail("{\"detail\":\"a\\nb\"}"), "a b");
        let long = "x".repeat(400);
        assert!(
            extract_detail(&long).chars().count() <= 301,
            "超长 detail 必须被截断"
        );
    }
}

// ---------------------------------------------------------------------------
// 语义搜索（FEAT-SEM）：Chinese-CLIP embedding 客户端
// 解耦原则与分类通道一致：本模块只是 HTTP 客户端，模型生命周期由微服务管理；
// CLIP 为可选通道——服务未就绪时调用方（content.rs）静默降级，不报错。
// ---------------------------------------------------------------------------

/// 单张 embedding 结果（错误内联，与 VisionResult 同风格）
#[derive(Debug, Clone, Serialize)]
pub struct EmbedResult {
    /// 图片（缩略图编码输入对应）源图路径
    pub path: String,
    /// 512 维已归一化向量；解码失败为 None
    pub embedding: Option<Vec<f32>>,
    /// 单张失败原因
    pub error: Option<String>,
}

/// 批量 embedding 进度事件载荷（前端进度条，与 ClassifyProgress 同构）
#[derive(Debug, Clone, Serialize)]
pub struct EmbedProgress {
    pub current: usize,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
}

/// 向 VCR 提交 JSON 并解析响应：**先判 HTTP 状态**，未就绪按 1s/2s/4s 退让重试
///
/// - 2xx → 返回解析后的 JSON
/// - 503「未就绪 / 加载中」→ 退让重试（最多 3 次）；最终错误带 `RETRYABLE_PREFIX`，
///   调用方据此**不进入退避**，只是本次搜索降级为纯关键词
/// - 其他非 2xx → 立即返回确定性错误（模型缺失 / 组件过期等），调用方进退避
/// - 连接层失败（端口不通 / 进程已退）→ 立即返回，不做无谓等待
///
/// 无论走哪条路径，真实原因都会写进 app.log——此前只落在 stderr，用户与排查者
/// 都看不到（BUG-2026-0920-008 的直接成因）。
async fn post_json_with_retry(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    what: &str,
) -> Result<serde_json::Value, String> {
    // 退让间隔：第 1/2/3 次失败后分别等 1s / 2s / 4s（合计最多 7s）
    const DELAYS_MS: [u64; 3] = [1000, 2000, 4000];
    let mut last_err = String::new();
    for (attempt, delay_ms) in DELAYS_MS.iter().enumerate() {
        match client.post(url).json(body).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return resp
                        .json::<serde_json::Value>()
                        .await
                        .map_err(|e| format!("解析 {what} 结果失败: {e}"));
                }
                let raw = resp.text().await.unwrap_or_default();
                let detail = extract_detail(&raw);
                crate::logger::log_warn(&format!("[embed] {what} HTTP {status} | {detail}"));
                let err = http_failure_err(status, &detail);
                // 确定性失败：不值得再等，直接把真实原因交给调用方
                if !is_retryable_err(&err) {
                    return Err(err);
                }
                last_err = err;
                crate::logger::log_info(&format!(
                    "[embed] {what} 第 {} 次未就绪，{}ms 后重试",
                    attempt + 1,
                    delay_ms
                ));
            }
            Err(e) => {
                let msg = format!("调用 embedding 服务失败: {e}");
                crate::logger::log_warn(&format!("[embed] {what} {msg}"));
                return Err(msg);
            }
        }
        tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
    }
    Err(last_err)
}

/// 批量编码：图片路径 → 512 维归一化向量（Chinese-CLIP fp16，双塔 vision 侧）
///
/// 输入应为**缩略图**路径（256px 编码与原图向量质量差异可忽略，解码快 50~200ms/张），
/// 由调用方（content.rs `do_semantic` 分支）负责从 `photo_thumb_cache` 取映射并补齐缺失。
pub async fn embed_images_batch(
    paths: &[String],
    batch_size: usize,
    app: &tauri::AppHandle,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Vec<EmbedResult>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    ensure_service_ready(&client, app, false).await?;

    let batch = batch_size.max(1);
    let mut results: Vec<EmbedResult> = Vec::with_capacity(paths.len());
    let mut done = 0usize;
    let mut failed = 0usize;

    for chunk in paths.chunks(batch) {
        // 收到停止请求 → 提前结束，保留已完成部分
        if cancel
            .as_ref()
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(false)
        {
            break;
        }
        let resp = post_json_with_retry(
            &client,
            &format!("{}/embed_batch", vcr_base()),
            &serde_json::json!({ "paths": chunk }),
            "embed_batch",
        )
        .await?;

        let Some(items) = resp.get("results").and_then(|v| v.as_array()) else {
            // 2xx 却没有 results：响应结构不对（服务版本不匹配等）。显式报错而不是
            // 静默返回空列表——否则「索引在库里却搜不到」的根因会被悄悄吞掉。
            return Err("embedding 响应缺少 results（服务版本过旧？）".to_string());
        };
        for item in items {
            let path = item
                .get("path")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let embedding = item
                .get("embedding")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|f| f.as_f64()).map(|f| f as f32).collect::<Vec<f32>>());
            let error = item
                .get("error")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            if embedding.is_some() {
                done += 1;
            } else {
                failed += 1;
            }
            results.push(EmbedResult { path, embedding, error });
        }

        let _ = app.emit(
            "embed-progress",
            EmbedProgress {
                current: results.len().min(paths.len()),
                total: paths.len(),
                done,
                failed,
            },
        );
    }

    Ok(results)
}

/// 查询文本 → 512 维归一化向量（Chinese-CLIP 双塔 text 侧，单次 ~7ms@DML）
pub async fn embed_text_query(text: &str, app: &tauri::AppHandle) -> Result<Vec<f32>, String> {
    let mut out = embed_text_batch(&[text.to_string()], app).await?;
    match out.pop() {
        Some((_, v)) if !v.is_empty() => Ok(v),
        _ => Err("embedding 响应为空".into()),
    }
}

/// 批量文本 → 向量（v5 语义分类：用户分类关键词 + 中性基线提示词一次编码）
///
/// 返回顺序与请求一致；服务端单次封顶 64 条，此处自动分片（避免超长请求）。
pub async fn embed_text_batch(
    texts: &[String],
    app: &tauri::AppHandle,
) -> Result<Vec<(String, Vec<f32>)>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    ensure_service_ready(&client, app, false).await?;
    let mut out: Vec<(String, Vec<f32>)> = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(64) {
        let resp = post_json_with_retry(
            &client,
            &format!("{}/embed_text_batch", vcr_base()),
            &serde_json::json!({ "texts": chunk }),
            "embed_text_batch",
        )
        .await?;
        let items = resp
            .get("results")
            .and_then(|x| x.as_array())
            .ok_or_else(|| "embedding 响应缺少 results（服务版本过旧？）".to_string())?;
        for item in items {
            let text = item
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let vec: Vec<f32> = item
                .get("embedding")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|f| f.as_f64()).map(|f| f as f32).collect())
                .unwrap_or_default();
            if !vec.is_empty() {
                out.push((text, vec));
            }
        }
    }
    if out.is_empty() {
        return Err("embedding 响应为空".into());
    }
    Ok(out)
}

/// 当前生效的语义模型标识（/health.model_id；服务不可达返回 Err，调用方据此提示）
pub async fn clip_model_id(app: &tauri::AppHandle) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    ensure_service_ready(&client, app, false).await?;
    let resp: serde_json::Value = client
        .get(format!("{}/health", vcr_base()))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("解析 health 失败: {e}"))?;
    resp.get("model_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "识别服务未返回语义模型标识（服务版本过旧？）".to_string())
}

/// 探测 CLIP 子系统是否就绪（/health.clip_ready；服务不可达返回 false，不报错）。
/// 供诊断/脚本使用：主链路（扫描/搜索/分类）直接调 /embed_* 并依赖返回码降级。
#[allow(dead_code)]
pub async fn clip_ready(app: &tauri::AppHandle) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    // 已有实例直接探测；无实例不拉起（避免搜索时冷启动数十秒服务）
    if let Some(base) = current_base() {
        return probe_clip(client, base).await;
    }
    if let Some((_, port)) = load_instance(app) {
        return probe_clip(client, format!("http://127.0.0.1:{port}")).await;
    }
    false
}

#[allow(dead_code)]
async fn probe_clip(client: reqwest::Client, base: String) -> bool {
    let ok_json = match client
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(r) => r.json::<serde_json::Value>().await.ok(),
        Err(_) => None,
    };
    ok_json
        .and_then(|v| v.get("clip_ready").and_then(|x| x.as_bool()))
        .unwrap_or(false)
}
