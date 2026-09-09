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

use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
const VCR_API_VERSION: u64 = 3;
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

async fn probe_base(client: &reqwest::Client, base: &str) -> HealthProbe {
    let resp = match client
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return HealthProbe::NotReachable, // 连接拒绝/超时 → 端口可拉起
    };
    let v = match resp.json::<serde_json::Value>().await {
        Ok(v) => v,
        Err(_) => return HealthProbe::Loading, // 可连接但解析失败 → 视为加载中，不打断
    };
    let ok = v.get("ok").and_then(|x| x.as_bool()) == Some(true);
    if ok {
        let ver = v.get("api_version").and_then(|x| x.as_u64()).unwrap_or(1);
        if ver >= VCR_API_VERSION {
            HealthProbe::Ready
        } else {
            HealthProbe::OldVersion
        }
    } else {
        // ok=false = 服务可达但模型未就绪（正在加载）→ 等待，不打断
        HealthProbe::Loading
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
    if photos.is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    ensure_service_ready(&client, app).await?;

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
fn collect_images(dir: &str) -> Result<Vec<String>, String> {
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
    ensure_service_ready(&client, app).await?;
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
) -> Result<(), String> {
    // 单飞：并发命令共享一次「探测/收养/重启/启动」，避免多进程抽数据库与端口
    let _guard = ENSURE_LOCK.lock().await;

    // 0. 过期组件收敛保护：上次冷启动已确认组件文件过期 → 直接报错，
    //    不再「杀→拉→杀」空转（组件重新打包后 mtime 变化自动解除）
    if let Some((path, mtime)) = stale_component_record() {
        let cur = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if cur == Some(mtime) {
            return Err(stale_component_err(&path));
        }
        clear_stale_component();
    }

    // A. 本进程已启动/收养过的实例
    if let Some(base) = current_base() {
        match probe_base(client, &base).await {
            HealthProbe::Ready => return Ok(()),
            HealthProbe::Loading => {
                poll_ready(client, &base, READY_TIMEOUT).await?;
                return Ok(());
            }
            // OldVersion / NotReachable → 统一落到 B 段按实例记录处理
            //（旧版本重启 / 收养等待或清场），杀进程逻辑只保留一处，避免分叉
            _ => clear_base(),
        }
    }

    // B. 实例记录（本进程或上次运行遗留）
    if let Some((pid, port)) = load_instance(app) {
        let base = format!("http://127.0.0.1:{port}");
        match probe_base(client, &base).await {
            HealthProbe::Ready => {
                set_base(base);
                return Ok(());
            }
            HealthProbe::Loading => {
                poll_ready(client, &base, READY_TIMEOUT).await?;
                set_base(base);
                return Ok(());
            }
            HealthProbe::OldVersion => {
                // 旧版本服务：请其自退并兜底强杀，再冷启动到新版本
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
                if pid_alive(pid) {
                    match poll_ready(client, &base, READY_TIMEOUT).await {
                        Ok(()) => {
                            set_base(base);
                            return Ok(());
                        }
                        // 进程仍在启动/加载：保留实例，下次 ensure 继续收养等待
                        Err(err) => return Err(err),
                    }
                }
                // 进程确已死亡（崩溃 / bind 失败残留）→ 清场后走 C 冷启动
                clear_instance(app);
            }
        }
    } else {
        // 无 PID 记录（历史遗留）：按进程名清本项目孤儿
        kill_our_orphans();
    }

    // C. 冷启动：冷门段端口；加载慢时「不杀进程」，落实例供下次收养继续等
    let mut last_err: Option<String> = None;
    for _ in 0..3 {
        let port = pick_cold_port();
        if port == 0 {
            continue;
        }
        let base = format!("http://127.0.0.1:{port}");
        let pid = spawn_server(app, port)?;
        // 等 800ms：bind 失败的 uvicorn 会立刻退出
        tokio::time::sleep(Duration::from_millis(800)).await;
        if !pid_alive(pid) {
            last_err = Some("进程启动后立即退出（端口被占或运行时异常）".into());
            continue;
        }
        match poll_ready(client, &base, READY_TIMEOUT).await {
            Ok(()) => {
                set_base(base.clone());
                save_instance(app, pid, port);
                return Ok(());
            }
            Err(timeout_err) => {
                if !pid_alive(pid) {
                    last_err = Some(timeout_err);
                    continue;
                }
                // 区分「加载中超时」（进程保留供收养继续等）与「拉起的组件仍是
                // 旧版本」：后者重启一万次也不会变新，登记组件并终止，否则形成
                // 「杀→拉→杀」死循环（十几个 vcr-server.exe 连环生灭的资源灾难）
                if matches!(probe_base(client, &base).await, HealthProbe::OldVersion) {
                    kill_pid(pid);
                    clear_instance(app);
                    set_stale_component(spawn_target_path(app));
                    return Err(stale_component_err(&spawn_target_path(app)));
                }
                // 进程仍存活 = 正在加载模型 → 保留进程，落实例供收养，提示稍候
                set_base(base.clone());
                save_instance(app, pid, port);
                return Err(timeout_err);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "识别服务启动失败".into()))
}

/// 轮询就绪；Loading 继续等，OldVersion 报版本错误，超时报错
async fn poll_ready(
    client: &reqwest::Client,
    base: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match probe_base(client, base).await {
            HealthProbe::Ready => return Ok(()),
            HealthProbe::OldVersion => {
                return Err("识别服务为旧版本（缺 GPU/模型切换端点），请结束该进程后重试".into());
            }
            _ => {}
        }
        if Instant::now() > deadline {
            // 进程仍在加载模型：不杀（下次 ensure 会收养继续等），提示稍候
            return Err("识别服务正在加载模型（大模型首次加载较慢），请稍候重试".into());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// 启动识别微服务（打包版优先内置 exe；开发版 python server.py）。
/// 返回进程 PID（供实例落盘/清场）。
fn spawn_server(app: &tauri::AppHandle, port: u16) -> Result<u32, String> {
    // 解析资源/数据目录（打包版定位依赖这两个路径）
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("获取资源目录失败: {e}"))?;
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?
        .join("vcr-data");
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
        // 模型在资源目录（随 MSI 安装，只读）；人物数据在 app_data_dir（可写）
        cmd.env("VCR_MODEL_DIR", bundled_dir.join("models"));
        cmd.env("VCR_DATA_DIR", &data_dir);
    } else {
        // 开发版：python server.py
        let server_script = project_python_dir().join("server.py");
        if !server_script.is_file() {
            return Err(format!("识别服务脚本不存在: {}", server_script.display()));
        }
        let py_dir = project_python_dir();
        workdir = py_dir.clone();
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd = std::process::Command::new("python");
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        #[cfg(not(target_os = "windows"))]
        {
            cmd = std::process::Command::new("python");
        }
        cmd.arg(&server_script);
    }

    // 动态端口：本次分配的端口经环境变量传给服务
    cmd.env("VCR_PORT", port.to_string());
    let child = cmd
        .current_dir(&workdir)
        .spawn()
        .map_err(|e| format!(
            "启动识别服务失败（打包版请确认安装目录 vcr/vcr-server.exe 存在；开发版请 pip install -r python/requirements.txt）: {e}"
        ))?;
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

/// 查询 GPU 加速可行性：确保服务就绪后请求 /gpu
pub async fn vcr_gpu_status(app: &tauri::AppHandle) -> Result<VcrGpuStatus, String> {
    let client = http_client().await?;
    ensure_service_ready(&client, app).await?;
    let resp: serde_json::Value = client
        .get(format!("{}/gpu", vcr_base()))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("解析结果失败: {e}"))?;
    Ok(gpu_status_from_value(&resp, true))
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
    let client = http_client().await?;
    ensure_service_ready(&client, app).await?;
    let resp = client
        .post(format!("{}/gpu", vcr_base()))
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
        return Err(format!(
            "{detail}（若为 Method Not Allowed，说明识别服务是旧版本，请重启应用自动升级）"
        ));
    }
    Ok(gpu_status_from_value(&v, true))
}

/// FEAT-051：分类模型候选清单（含是否已下载 / 当前生效）
pub async fn vcr_list_models(app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    let client = http_client().await?;
    ensure_service_ready(&client, app).await?;
    let resp = client
        .get(format!("{}/models", vcr_base()))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析结果失败: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "识别服务不含模型清单端点（服务版本过旧，请重启应用）: HTTP {}",
            status.as_u16()
        ));
    }
    Ok(v)
}

/// FEAT-051：切换分类模型（文件未下载 / 未知名称 → 提取服务端 detail 报错）
pub async fn vcr_set_model(app: &tauri::AppHandle, model: &str) -> Result<serde_json::Value, String> {
    let client = http_client().await?;
    ensure_service_ready(&client, app).await?;
    let resp = client
        .post(format!("{}/model", vcr_base()))
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
        return Err(detail.to_string());
    }
    Ok(v)
}

/// FEAT-053：cls 通道固定张量测速（CPU/GPU 真实加速比一键对比）。
/// 可能触发模型加载与数十次推理（秒级耗时），用每请求独立长超时（共享客户端 15s 不够）。
pub async fn vcr_benchmark(
    app: &tauri::AppHandle,
    runs: u32,
    warmup: u32,
) -> Result<serde_json::Value, String> {
    let client = http_client().await?;
    ensure_service_ready(&client, app).await?;
    let resp = client
        .post(format!("{}/benchmark", vcr_base()))
        .timeout(Duration::from_secs(60))
        .json(&serde_json::json!({ "runs": runs, "warmup": warmup }))
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
        return Err(detail.to_string());
    }
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
}
