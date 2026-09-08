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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, Manager};

/// Python 微服务固定端口（与 server.py 默认一致，服务地址由 VCR_URL 引用）
const VCR_URL: &str = "http://127.0.0.1:8765";
/// 服务启动就绪等待上限
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// FEAT-051：要求的服务 API 版本（GPU 开关 + 模型切换能力）；
/// 探测到运行中服务版本过旧时自动 POST /shutdown 重启到新版本
const VCR_API_VERSION: u64 = 2;
/// FEAT-051：ensure 单飞锁 —— 并发命令共享一次「探测/重启/启动」流程，
/// 邓免多进程同时拚 8765 端口（winerror 10048）
static ENSURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 健康探测结果
enum HealthProbe {
    /// 新版本服务已就绪
    Ready,
    /// 服务存活但是旧版本（缺 GPU/模型端点）→ 需重启
    OldVersion,
    /// 未运行 / 探测失败
    Down,
}

async fn probe_health(client: &reqwest::Client) -> HealthProbe {
    let resp = match client
        .get(format!("{VCR_URL}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return HealthProbe::Down,
    };
    let v = match resp.json::<serde_json::Value>().await {
        Ok(v) => v,
        Err(_) => return HealthProbe::Down,
    };
    if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        let ver = v.get("api_version").and_then(|x| x.as_u64()).unwrap_or(1);
        if ver >= VCR_API_VERSION {
            HealthProbe::Ready
        } else {
            HealthProbe::OldVersion
        }
    } else {
        HealthProbe::Down
    }
}

/// 兜底释放 VCR 端口：旧版本服务无 /shutdown 自退端点时强制结束占用进程
fn kill_port_holder() {
    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("netstat")
            .args(["-ano", "-p", "tcp"])
            .output();
        if let Ok(out) = out {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut pids: std::collections::HashSet<String> = Default::default();
            for line in text.lines() {
                if line.contains(":8765") && line.to_uppercase().contains("LISTENING") {
                    if let Some(pid) = line.split_whitespace().last() {
                        if pid.chars().all(|c| c.is_ascii_digit()) {
                            pids.insert(pid.to_string());
                        }
                    }
                }
            }
            for pid in pids {
                eprintln!("[VCR][ensure] 强制结束占用 8765 端口的进程 PID={pid}");
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/PID", &pid])
                    .output();
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("sh")
            .args(["-c", "lsof -ti :8765 | xargs -r kill -9"])
            .output();
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
            .post(format!("{VCR_URL}/classify_batch"))
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
        .post(format!("{VCR_URL}/classify_batch"))
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
    // 单飞：拿锁后重新探测（可能已被前一个等待者修复），避免并发拉起多进程报 10048
    let _guard = ENSURE_LOCK.lock().await;
    match probe_health(client).await {
        HealthProbe::Ready => return Ok(()),
        HealthProbe::OldVersion => {
            // 旧版本：请求自退（新版本才有该端点），随后兣底强制释放端口
            let _ = client
                .post(format!("{VCR_URL}/shutdown"))
                .timeout(Duration::from_secs(2))
                .send()
                .await;
            tokio::time::sleep(Duration::from_millis(800)).await;
            kill_port_holder();
        }
        HealthProbe::Down => {
            // 端口仍可能被无响应进程占着（连接失败但 LISTENING）→ 兕底清理
            kill_port_holder();
        }
    }

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

    let child = cmd.current_dir(&workdir).spawn();
    if let Err(e) = child {
        return Err(format!(
            "启动识别服务失败（打包版请确认安装目录 vcr/vcr-server.exe 存在；开发版请 pip install -r python/requirements.txt）: {e}"
        ));
    }

    // 讉询 /health 直到模型就绪且 API 版本达标
    let deadline = std::time::Instant::now() + READY_TIMEOUT;
    let mut saw_old_version = false;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(300)).await;
        match probe_health(client).await {
            HealthProbe::Ready => return Ok(()),
            HealthProbe::OldVersion => saw_old_version = true,
            HealthProbe::Down => {}
        }
    }
    if saw_old_version {
        return Err(
            "识别服务为旧版本（缺 GPU 开关/模型切换能力），自动重启失败：请手动结束旧的 python server.py 进程后重试".into(),
        );
    }
    Err("识别服务启动超时".into())
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
}

/// 查询 GPU 加速可行性：确保服务就绪后请求 /gpu
pub async fn vcr_gpu_status(app: &tauri::AppHandle) -> Result<VcrGpuStatus, String> {
    let client = http_client().await?;
    ensure_service_ready(&client, app).await?;
    let resp: serde_json::Value = client
        .get(format!("{VCR_URL}/gpu"))
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
    }
}

/// FEAT-051：GPU 加速开关（开 = GPU 优先 / 关 = 强制 CPU），返回切换后状态
pub async fn vcr_set_gpu(app: &tauri::AppHandle, enabled: bool) -> Result<VcrGpuStatus, String> {
    let client = http_client().await?;
    ensure_service_ready(&client, app).await?;
    let resp = client
        .post(format!("{VCR_URL}/gpu"))
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
        .get(format!("{VCR_URL}/models"))
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
        .post(format!("{VCR_URL}/model"))
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
