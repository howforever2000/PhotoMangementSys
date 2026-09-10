//! 应用内模型下载（FEAT-052）—— 后台下载 + 进度 + 官方/镜像择优
//!
//! - 每个模型配「官方 + 镜像」两路地址，两路**并行下载、先完成者赢**（真正"哪一个快用哪一个"）
//! - 下载/导出在后台线程（不阻塞 UI）；进度经 Tauri 事件实时上报，状态全局可查
//! - `yolov8*-cls`：官方源 `.pt`（Ultralytics release）→ 本地 python(torch+ultralytics) 导出 `.onnx`
//! - 场景 `resnet18_places365`（AI 分类必需）：`.pth.tar` + 类目表 → 导出 `.onnx`
//! - 临时文件 + 原子改名；支持取消；打包版导出依赖本机 python 含 torch/ultralytics
//!
//! 目录与 server.py 一致：python/models（VCR_MODEL_DIR 缺省）。

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// GitHub 加速镜像前缀（官方地址以 github.com 开头时，镜像 = 前缀 + 原地址）
const GITHUB_MIRROR_PREFIX: &str = "https://ghfast.top";

#[derive(Clone, PartialEq)]
enum DlKind {
    /// yolov8*-cls：.pt → ultralytics 导出 onnx
    ClsPt,
    /// Places365 场景：.pth.tar + 类目表 → onnx（AI 分类必需）
    Scene,
    /// 语义搜索双塔：Chinese-CLIP fp16 ONNX（Xenova 转换，hf-mirror 直链，
    /// 含 tokenizer.json/vocab.txt，下载后落到 chinese-clip/ 子目录，无导出步骤；
    /// 固定 CPU 推理——DML 对 fp16 图存在算子级数值 bug，不做 GPU 加速）
    ClipFp16,
}

/// 模型注册表白名单
struct DlSpec {
    name: &'static str,
    file: &'static str,
    kind: DlKind,
    required: bool,
}

impl DlSpec {
    /// 官方下载地址
    fn official(&self) -> String {
        match self.kind {
            DlKind::ClsPt => format!(
                "https://github.com/ultralytics/assets/releases/download/v8.3.0/{}.pt",
                self.name
            ),
            DlKind::Scene => {
                "http://places2.csail.mit.edu/models_places365/resnet18_places365.pth.tar".into()
            }
            // hf-mirror 已是国内加速镜像（huggingface.co 直连超时），不再二次套 ghfast
            DlKind::ClipFp16 => {
                "https://hf-mirror.com/Xenova/chinese-clip-vit-base-patch16/resolve/main/onnx/model_fp16.onnx".into()
            }
        }
    }
    /// 镜像地址（ghfast 前缀；Scene 官方 mit.edu 也可经 ghfast 加速）
    fn mirror(&self) -> String {
        if self.kind == DlKind::ClipFp16 {
            return self.official(); // 单源：download_first_wins 对同 URL 自动降为单路
        }
        format!("{GITHUB_MIRROR_PREFIX}/{}", self.official())
    }
    /// 下载临时文件的扩展名（导出环节依赖正确扩展名）
    fn tmp_ext(&self) -> &'static str {
        match self.kind {
            DlKind::ClsPt => "pt",
            DlKind::Scene => "pth.tar",
            DlKind::ClipFp16 => "onnx",
        }
    }
}

fn cls_spec(name: &'static str, required: bool) -> DlSpec {
    DlSpec {
        name,
        file: Box::leak(format!("{name}.onnx").into_boxed_str()),
        kind: DlKind::ClsPt,
        required,
    }
}

fn specs() -> Vec<DlSpec> {
    vec![
        cls_spec("yolov8l-cls", true),
        cls_spec("yolov8m-cls", true),
        cls_spec("yolov8x-cls", false),
        cls_spec("yolov8s-cls", false),
        DlSpec {
            name: "resnet18_places365",
            file: "resnet18_places365.onnx",
            kind: DlKind::Scene,
            required: true,
        },
        DlSpec {
            // 语义搜索双塔（FEAT-SEM）：fp16，377MB，固定 CPU 推理
            name: "chinese-clip",
            file: "chinese-clip/onnx/model_fp16.onnx",
            kind: DlKind::ClipFp16,
            required: false,
        },
    ]
}

/// 单模型下载状态（前端展示 + list 返回值）
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDlStatus {
    pub name: String,
    pub file: String,
    pub required: bool,
    pub running: bool,
    pub done: bool,
    pub stage: String, // downloading | exporting | done | error | idle
    pub bytes: u64,
    pub total: u64,
    pub error: Option<String>,
}

#[derive(Default)]
struct Store {
    status: HashMap<String, ModelDlStatus>,
    cancel: HashMap<String, Arc<AtomicBool>>,
}
static STORE: std::sync::LazyLock<Mutex<Store>> = std::sync::LazyLock::new(|| Mutex::new(Store::default()));

fn models_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(|p| p.join("python").join("models"))
        .unwrap_or_else(|| manifest.join("python").join("models"))
}

fn set_status(app: &AppHandle, s: ModelDlStatus) {
    if let Ok(mut st) = STORE.lock() {
        st.status.insert(s.name.clone(), s.clone());
    }
    let _ = app.emit("model-dl-progress", &s);
}

fn is_cancelled(name: &str) -> bool {
    STORE
        .lock()
        .ok()
        .and_then(|st| st.cancel.get(name).cloned())
        .map(|f| f.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// 开始下载（命令入口，后台异步）
pub async fn start(app: &AppHandle, name: &str) -> Result<(), String> {
    let Some(spec) = specs().into_iter().find(|s| s.name == name) else {
        return Err(format!("未知模型: {name}"));
    };
    let dir = models_dir();
    if dir.join(spec.file).is_file() {
        return Err(format!("{} 已存在", spec.file));
    }
    {
        let mut st = STORE.lock().map_err(|e| e.to_string())?;
        if st.status.get(name).map(|s| s.running).unwrap_or(false) {
            return Err("该模型正在下载中".into());
        }
        st.cancel.insert(name.to_string(), Arc::new(AtomicBool::new(false)));
    }
    set_status(
        app,
        ModelDlStatus {
            name: name.to_string(),
            file: spec.file.to_string(),
            required: spec.required,
            running: true,
            done: false,
            stage: "downloading".into(),
            bytes: 0,
            total: 0,
            error: None,
        },
    );

    let app = app.clone();
    let name = name.to_string();
    tauri::async_runtime::spawn(async move {
        let r = run(&app, &spec).await;
        // 更新终态
        let status = {
            let mut st = STORE.lock().unwrap_or_else(|e| e.into_inner());
            st.cancel.remove(&name);
            if let Some(s) = st.status.get_mut(&name) {
                s.running = false;
                match &r {
                    Ok(()) => {
                        s.stage = "done".into();
                        s.done = true;
                        s.bytes = s.total;
                    }
                    Err(e) => {
                        s.stage = "error".into();
                        s.error = Some(e.clone());
                    }
                }
                Some(s.clone())
            } else {
                None
            }
        };
        if let Some(s) = status {
            let _ = app.emit("model-dl-progress", &s);
        }
    });
    Ok(())
}

/// 取消下载
pub fn cancel(name: &str) {
    if let Ok(st) = STORE.lock() {
        if let Some(f) = st.cancel.get(name) {
            f.store(true, Ordering::SeqCst);
        }
    }
}

/// 全部模型下载状态（含未启动 idle）
pub fn list() -> Vec<ModelDlStatus> {
    let dir = models_dir();
    let st = STORE.lock().unwrap_or_else(|e| e.into_inner());
    specs()
        .into_iter()
        .map(|s| {
            let exists = dir.join(s.file).is_file();
            st.status.get(s.name).cloned().unwrap_or_else(|| ModelDlStatus {
                name: s.name.to_string(),
                file: s.file.to_string(),
                required: s.required,
                running: false,
                done: exists,
                stage: if exists { "done".into() } else { "idle".into() },
                bytes: 0,
                total: 0,
                error: None,
            })
        })
        .collect()
}

/// 一次完整流程：并行下载 → 导出 → 落盘
async fn run(app: &AppHandle, spec: &DlSpec) -> Result<(), String> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建模型目录失败: {e}"))?;

    // FEAT-SEM：CLIP fp16（固定 CPU）—— 下载 → 落子目录 → 附带 tokenizer/vocab，无导出步骤
    if spec.kind == DlKind::ClipFp16 {
        let final_dir = dir.join("chinese-clip").join("onnx");
        std::fs::create_dir_all(&final_dir).map_err(|e| format!("创建 CLIP 目录失败: {e}"))?;
        let winner = download_first_wins(app, spec, &dir).await?;
        let final_path = final_dir.join("model_fp16.onnx");
        std::fs::rename(&winner, &final_path)
            .map_err(|e| format!("落位失败: {e}"))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
        let base = "https://hf-mirror.com/Xenova/chinese-clip-vit-base-patch16/resolve/main/";
        let clip_dir = dir.join("chinese-clip");
        for f in ["tokenizer.json", "vocab.txt"] {
            if clip_dir.join(f).is_file() {
                continue;
            }
            let app2 = app.clone();
            let name2 = spec.name.to_string();
            download_one(
                app2,
                name2,
                format!("{base}{f}"),
                clip_dir.join(f),
                client.clone(),
            )
            .await?;
        }
        return Ok(());
    }

    // 1. 并行下载（官方 + 镜像），先完成者赢，返回赢家临时文件
    emit_status(app, spec, "downloading", 0, 0, false);
    let winner = download_first_wins(app, spec, &dir).await?;

    // 2. 导出 onnx（阻塞，用 spawn_blocking 避免占用异步运行时）
    emit_status(app, spec, "exporting", 0, 0, false);
    let final_path = dir.join(spec.file);
    let winner2 = winner.clone();
    let final2 = final_path.clone();
    let dir2 = dir.clone();
    let kind = spec.kind.clone();
    let export = |winner: PathBuf, final_path: PathBuf, dir: PathBuf, kind: DlKind| {
        match kind {
            DlKind::ClsPt => export_cls_pt(&winner, &final_path),
            DlKind::Scene => export_scene_pth(&winner, &final_path, &dir),
            // ClipFp16 在 run() 内提前处理（无导出步骤），此处不可达
            DlKind::ClipFp16 => Ok(()),
        }
    };
    tauri::async_runtime::spawn_blocking(move || export(winner2, final2, dir2, kind))
        .await
        .map_err(|e| format!("导出任务线程失败: {e}"))??;

    // 3. 清理临时文件
    let _ = std::fs::remove_file(&winner);
    Ok(())
}

fn emit_status(app: &AppHandle, spec: &DlSpec, stage: &str, bytes: u64, total: u64, done: bool) {
    set_status(
        app,
        ModelDlStatus {
            name: spec.name.to_string(),
            file: spec.file.to_string(),
            required: spec.required,
            running: !done,
            done,
            stage: stage.into(),
            bytes,
            total,
            error: None,
        },
    );
}

/// 两路并行下载，先完成者赢；返回赢家临时文件路径（带正确扩展名）
async fn download_first_wins(app: &AppHandle, spec: &DlSpec, dir: &Path) -> Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let d0 = dir.join(format!(".dl_{}_0.{}", spec.name, spec.tmp_ext()));
    let d1 = dir.join(format!(".dl_{}_1.{}", spec.name, spec.tmp_ext()));
    let official = spec.official();
    let mirror = spec.mirror();
    let app2 = app.clone();
    let name = spec.name.to_string();

    let f0 = download_one(app2.clone(), name.clone(), official.clone(), d0.clone(), client.clone());
    let f1 = download_one(app2, name.clone(), mirror.clone(), d1.clone(), client.clone());
    // 先完成者赢：tokio::select 返回第一个完成的 future，对位的另一路被取消；
    // 单源直链（官方 == 镜像）降为单路，避免同 URL 双倍流量
    let win_idx = if official == mirror {
        f0.await.map(|_| 0u8)?;
        0u8
    } else {
        tokio::select! {
            _ = f0 => 0u8,
            _ = f1 => 1u8,
        }
    };
    let (winner_dest, loser) = if win_idx == 0 { (d0, d1) } else { (d1, d0) };
    let _ = std::fs::remove_file(&loser);
    Ok(winner_dest)
}

/// 单路流式下载（带进度 + 可取消）
async fn download_one(
    app: AppHandle,
    name: String,
    url: String,
    dest: PathBuf,
    client: reqwest::Client,
) -> Result<(), String> {
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut stream = resp.bytes_stream();
    let mut f = std::fs::File::create(&dest).map_err(|e| format!("创建临时文件失败: {e}"))?;
    let mut bytes = 0u64;
    let mut last_emit = Instant::now();
    while let Some(chunk) = stream.next().await {
        if is_cancelled(&name) {
            let _ = std::fs::remove_file(&dest);
            return Err("已取消".into());
        }
        let c = chunk.map_err(|e| format!("下载中断: {e}"))?;
        f.write_all(&c).map_err(|e| format!("写文件失败: {e}"))?;
        bytes += c.len() as u64;
        if last_emit.elapsed() >= Duration::from_millis(150) {
            last_emit = Instant::now();
            let _ = app.emit(
                "model-dl-progress",
                &ModelDlStatus {
                    name: name.clone(),
                    file: String::new(),
                    required: false,
                    running: true,
                    done: false,
                    stage: "downloading".into(),
                    bytes,
                    total,
                    error: None,
                },
            );
        }
    }
    let _ = f.sync_all();
    Ok(())
}

/// yolov8*-cls：.pt → .onnx（ultralytics）
fn export_cls_pt(pt: &Path, dst: &Path) -> Result<(), String> {
    let out = std::process::Command::new("python")
        .args([
            "-c",
            &format!(
                "from ultralytics import YOLO\n\
                 m = YOLO(r'{}')\n\
                 m.export(format='onnx', imgsz=224, simplify=True, opset=12)\n\
                 import os\n\
                 p = os.path.splitext(r'{}')[0] + '.onnx'\n\
                 print('ONNX', p)",
                pt.display(),
                pt.display()
            ),
        ])
        .output()
        .map_err(|e| format!("启动导出进程失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "导出 onnx 失败: {}",
            String::from_utf8_lossy(&out.stderr).lines().next_back().unwrap_or("未知错误")
        ));
    }
    let onnx = pt.with_extension("onnx");
    if !onnx.is_file() {
        return Err("导出产物不存在（检查 ultralytics/torch 环境）".into());
    }
    std::fs::copy(&onnx, dst).map_err(|e| format!("移动 onnx 失败: {e}"))?;
    let _ = std::fs::remove_file(&onnx);
    Ok(())
}

/// Places365：.pth.tar + 类目表 → .onnx
fn export_scene_pth(pth: &Path, dst: &Path, dir: &Path) -> Result<(), String> {
    let cats = dir.join("categories_places365.txt");
    if !cats.is_file() {
        let _ = download_simple(
            "https://raw.githubusercontent.com/csailvision/places365/master/categories_places365.txt",
            &cats,
        );
    }
    let out = std::process::Command::new("python")
        .args([
            "-c",
            &format!(
                "import torch, torchvision.models as M\n\
                 sd = torch.load(r'{}', map_location='cpu', weights_only=False)\n\
                 m = M.resnet18(num_classes=365)\n\
                 m.load_state_dict(sd, strict=True)\n\
                 m.eval()\n\
                 torch.onnx.export(m, torch.randn(1,3,224,224), r'{}', opset_version=12, input_names=['input'], output_names=['output'])\n\
                 print('ONNX done')",
                pth.display(),
                dst.display()
            ),
        ])
        .output()
        .map_err(|e| format!("启动导出进程失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "导出场景模型失败: {}",
            String::from_utf8_lossy(&out.stderr).lines().next_back().unwrap_or("未知错误")
        ));
    }
    Ok(())
}

fn download_simple(url: &str, dest: &Path) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let mut f = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    std::io::copy(&mut resp, &mut f).map_err(|e| e.to_string())?;
    Ok(())
}
