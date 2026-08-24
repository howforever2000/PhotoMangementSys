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
use tauri::Emitter;

/// Python 微服务固定端口（与 server.py 默认一致，服务地址由 VCR_URL 引用）
const VCR_URL: &str = "http://127.0.0.1:8765";
/// 服务启动就绪等待上限
const READY_TIMEOUT: Duration = Duration::from_secs(15);

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

    ensure_service_ready(&client).await?;

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

/// 确保 Python 微服务已就绪；未运行则启动并轮询 /health
async fn ensure_service_ready(client: &reqwest::Client) -> Result<(), String> {
    // 快速探测：已在运行且模型就绪 → 直接返回
    if let Ok(resp) = client
        .get(format!("{VCR_URL}/health"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
    {
        if let Ok(v) = resp.json::<serde_json::Value>().await {
            if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                return Ok(());
            }
        }
    }

    // 启动服务（Windows 下隐藏控制台窗口）
    let server_script = project_python_dir().join("server.py");
    if !server_script.is_file() {
        return Err(format!("识别服务脚本不存在: {}", server_script.display()));
    }
    #[cfg(target_os = "windows")]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("python");
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = std::process::Command::new("python");
    let child = cmd
        .arg(&server_script)
        .current_dir(&project_python_dir())
        .spawn();
    if let Err(e) = child {
        return Err(format!("启动识别服务失败（请确认已 pip install -r python/requirements.txt）: {e}"));
    }

    // 轮询 /health 直到模型就绪
    let deadline = std::time::Instant::now() + READY_TIMEOUT;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Ok(resp) = client.get(format!("{VCR_URL}/health")).send().await {
            if let Ok(v) = resp.json::<serde_json::Value>().await {
                if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                    return Ok(());
                }
                // 服务在但模型未就绪（如模型文件缺失）→ 直接报错，不再等待
                if let Some(classes) = v.get("classes").and_then(|x| x.as_u64()) {
                    if classes == 0 {
                        return Err("识别模型未加载（检查 python/models/ 目录下 ONNX 模型是否存在）".into());
                    }
                }
            }
        }
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
}

/// 查询 GPU 加速可行性：确保服务就绪后请求 /gpu
pub async fn vcr_gpu_status() -> Result<VcrGpuStatus, String> {
    let client = http_client().await?;
    ensure_service_ready(&client).await?;
    let resp: serde_json::Value = client
        .get(format!("{VCR_URL}/gpu"))
        .send()
        .await
        .map_err(|e| format!("调用识别服务失败: {e}"))?
        .json()
        .await
        .map_err(|e| format!("解析结果失败: {e}"))?;
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
    Ok(VcrGpuStatus {
        running: true,
        use_gpu: resp.get("use_gpu").and_then(|v| v.as_bool()).unwrap_or(false),
        provider: resp.get("provider").and_then(|v| v.as_str()).unwrap_or("cpu").to_string(),
        gpu,
        available,
        batch_max: resp.get("batch_max").and_then(|v| v.as_u64()).unwrap_or(8) as usize,
    })
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
