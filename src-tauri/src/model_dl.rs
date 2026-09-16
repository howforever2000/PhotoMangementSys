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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Clone, PartialEq)]
enum DlKind {
    /// Chinese-CLIP fp16 ONNX（Xenova 转换，hf-mirror 直链）：
    /// 单文件双塔（首次使用时服务端自动拆）+ tokenizer.json/vocab.txt，无导出步骤；
    /// 固定 CPU 推理——DML 对 fp16 图存在算子级数值 bug（BUG-2026-0910-006）
    ClipOnnx,
}

/// 模型注册表白名单
///
/// 目录约定（与 python/vcr/config.py::CLIP_MODEL_META 严格一致）：
///
/// 档位清单 = b16（fp16，默认）+ b16-fp32（可走 DirectML）。
/// L/14-336 已于 2026-09-21 实测否决并下架（低于 B/16 13pp 且慢 3.5~10×，
/// 见 design/clip-accuracy-comparison.md），故不在下载表中。
/// ```text
/// python/models/<root>/onnx/<onnx>     ← 双塔整图（首次使用时服务端自动拆成 clip_vision/clip_text）
/// python/models/<root>/tokenizer.json  ← 附带文件落「模型根目录」，不是 onnx/ 子目录
/// python/models/<root>/vocab.txt
/// ```
/// ⚠️ tokenizer.json / vocab.txt 必须在 `<root>` 而不是 `<root>/onnx`：
/// CLIP_TOKENIZER_JSON/CLIP_VOCAB 都按模型根目录解析（BUG-2026-0920-002）。
struct DlSpec {
    name: &'static str,
    /// HF 仓库（hf-mirror 直链前缀）
    repo: &'static str,
    /// 模型根目录（相对 python/models）
    root: &'static str,
    /// onnx 文件名（位于 <root>/onnx/ 下）：model_fp16.onnx / model.onnx
    onnx: &'static str,
    /// 附带文件（落 <root>/ 下）
    extra: &'static [&'static str],
    kind: DlKind,
    required: bool,
    /// FEAT-061：下载文件体积下限（字节）——用于识别 HTML 错误页 / 半包文件
    expect_min: u64,
}

impl DlSpec {
    fn base_url(&self) -> String {
        // hf-mirror 已是国内加速镜像（huggingface.co 直连超时），不再二次套 ghfast
        // FEAT-061：不再硬编码单一 hf-mirror；取候选源模板的第一个前缀
        //（其余候选由 download_first_wins 的 official/mirror 双路竞速覆盖）
        match self.kind {
            DlKind::ClipOnnx => source_prefix(self.repo),
        }
    }
    /// 整图**本地**相对路径（相对 python/models；存在即视为已下载）
    fn rel_file(&self) -> String {
        format!("{}/onnx/{}", self.root, self.onnx)
    }
    /// 整图在**仓库内**的相对路径（用于拼下载 URL）—— 只有 `onnx/<file>`，
    /// **绝不能带本地落位目录名**（root 是本机的目录约定，仓库里没有这一层）。
    /// 这里曾误用 rel_file() 拼 URL，导致应用内点「下载」全部 404（BUG-2026-0921-001）：
    ///   错：.../resolve/main/chinese-clip-l14/onnx/model_fp16.onnx → 404
    ///   对：.../resolve/main/onnx/model_fp16.onnx                 → 200
    fn repo_file(&self) -> String {
        format!("onnx/{}", self.onnx)
    }
    /// 官方下载地址（仓库路径，不带本地目录）
    fn official(&self) -> String {
        let tpls = source_templates();
        fill_source(&tpls[0], self.repo, &self.repo_file())
    }
    /// 镜像地址（单源直链：download_first_wins 对同 URL 自动降为单路）
    fn mirror(&self) -> String {
        let tpls = source_templates();
        let idx = if tpls.len() > 1 { 1 } else { 0 };
        fill_source(&tpls[idx], self.repo, &self.repo_file())
    }
    /// 下载临时文件的扩展名（导出环节依赖正确扩展名）
    fn tmp_ext(&self) -> &'static str {
        "onnx"
    }
}

fn specs() -> Vec<DlSpec> {
    vec![
        DlSpec {
            // 语义默认档 B/16 fp16（512 维，377MB，固定 CPU 推理）
            name: "chinese-clip",
            repo: "Xenova/chinese-clip-vit-base-patch16",
            root: "chinese-clip",
            onnx: "model_fp16.onnx",
            extra: &["tokenizer.json", "vocab.txt"],
            kind: DlKind::ClipOnnx,
            // FEAT-061：体积下限（fp16 档实测 ~377MB；取 100MB 兜底，容忍仓库更新）
            expect_min: 100_000_000,
            required: false,
        },
        DlSpec {
            // B/16 fp32（512 维，719MB）：体积换精度 + 可走 DirectML（Phase 0 实测 37.9ms/张
            // vs CPU 91ms）；fp16 在 DML 上有算子级数值 bug，故 GPU 只能在 fp32 档追求。
            // 独立档位 = 独立 model id（向量空间与 fp16 微差，绝不混用）。
            name: "chinese-clip-fp32",
            repo: "Xenova/chinese-clip-vit-base-patch16",
            root: "chinese-clip-fp32",
            onnx: "model.onnx",
            extra: &["tokenizer.json", "vocab.txt"],
            kind: DlKind::ClipOnnx,
            // FEAT-061：体积下限（fp32 档实测 ~754MB；取 100MB 兜底）
            expect_min: 100_000_000,
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
    // VCR_MODEL_DIR 优先：打包版由 lib.rs::setup 指向 resource_dir/vcr/models，
    // 子进程（vcr-server.exe）也按同一路径注入；未设置时回落源码目录
    // python/models（开发态）。
    if let Ok(v) = std::env::var("VCR_MODEL_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(|p| p.join("python").join("models"))
        .unwrap_or_else(|| manifest.join("python").join("models"))
}

/// 下载过程日志（走 stderr，与 vision 微服务日志同渠道，宿主副窗口可见）
fn logger_info(msg: &str) {
    eprintln!("{msg}");
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
    let rel = spec.rel_file();
    if dir.join(&rel).is_file() {
        return Err(format!("{rel} 已存在"));
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
            file: spec.rel_file(),
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
            let exists = dir.join(s.rel_file()).is_file();
            st.status.get(s.name).cloned().unwrap_or_else(|| ModelDlStatus {
                name: s.name.to_string(),
                file: s.rel_file(),
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

/// 一次完整流程：下载主文件 → 落位 → 拉附带文件（tokenizer/vocab）
async fn run(app: &AppHandle, spec: &DlSpec) -> Result<(), String> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建模型目录失败: {e}"))?;

    // 模型根目录：tokenizer.json / vocab.txt 落这里（不是 onnx/ 子目录）
    let final_dir = dir.join(spec.root);
    let onnx_dir = final_dir.join("onnx");
    let final_path = onnx_dir.join(spec.onnx);
    std::fs::create_dir_all(&onnx_dir).map_err(|e| format!("创建语义模型目录失败: {e}"))?;

    // 1. 主文件（单源直链；download_first_wins 对同 URL 自动降为单路）
    emit_status(app, spec, "downloading", 0, 0, false);
    let winner = download_first_wins(app, spec, &dir).await?;
    // FEAT-061：落地前做基本校验（服务端可能返回 HTML 错误页 / 半包文件）
    if let Err(e) = validate_model_file(&winner, spec.expect_min) {
        let _ = std::fs::remove_file(&winner);
        return Err(e);
    }
    std::fs::rename(&winner, &final_path).map_err(|e| format!("落位失败: {e}"))?;

    // 2. 附带文件（tokenizer.json / vocab.txt）
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(Duration::from_secs(600))
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    for f in spec.extra {
        let dest = final_dir.join(f);
        if dest.is_file() {
            continue;
        }
        // 小文件同样受 hf-mirror 瞬时 403 影响；整图已下完却因 tokenizer 失败而整体报错
        // 代价太大（但 tokenizer 缺失会让 CLIP 直接不可用），故同样做有限重试。
        let mut last = String::new();
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            }
            // FEAT-061：附带文件同样按源轮换（tokenizer.json / vocab.txt 缺失会让 CLIP 直接不可用）
            let tpls = source_templates();
            let tpl = tpls[(attempt as usize) % tpls.len()].clone();
            match download_one(
                app.clone(),
                spec.name.to_string(),
                fill_source(&tpl, spec.repo, f),
                dest.clone(),
                client.clone(),
            )
            .await
            {
                Ok(()) => {
                    last.clear();
                    break;
                }
                Err(e) => last = format!("{e} @ {tpl}"),
            }
        }
        if !last.is_empty() {
            return Err(format!("{f} 下载失败: {last}"));
        }
    }

    // FEAT-SEM：模型就位后立即解除语义退避（下载完即可用，不必等 TTL）
    crate::vision::clear_semantic_down();
    // 首次使用前需拆双塔：服务端自动拆（幂等），此处不阻塞下载完成
    Ok(())
}

fn emit_status(app: &AppHandle, spec: &DlSpec, stage: &str, bytes: u64, total: u64, done: bool) {
    set_status(
        app,
        ModelDlStatus {
            name: spec.name.to_string(),
            file: spec.rel_file(),
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
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(Duration::from_secs(3600))
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let d0 = dir.join(format!(".dl_{}_0.{}", spec.name, spec.tmp_ext()));
    let d1 = dir.join(format!(".dl_{}_1.{}", spec.name, spec.tmp_ext()));
    let official = spec.official();
    let mirror = spec.mirror();
    let name = spec.name.to_string();

    // hf-mirror 对同一来源的密集请求会瞬时 403（实测：连续 HEAD/Range/GET 后 403，稍候恢复），
    // 而这里是 400MB~800MB 的大文件 —— 失败即白下，故加有限重试（3 次，退避 2s/4s）。
    let mut last_err = String::from("下载失败");
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            logger_info(&format!(
                "[model_dl] 重试第 {attempt} 次下载 {}（上次：{last_err}）",
                spec.name
            ));
        }
        let f0 = download_one(
            app.clone(),
            name.clone(),
            official.clone(),
            d0.clone(),
            client.clone(),
        );
        let f1 = download_one(
            app.clone(),
            name.clone(),
            mirror.clone(),
            d1.clone(),
            client.clone(),
        );
        // 先完成者赢：tokio::select 返回第一个完成的 future，对位的另一路被取消；
        // 单源直链（官方 == 镜像）降为单路，避免同 URL 双倍流量
        let r = if official == mirror {
            f0.await.map(|_| 0u8)
        } else {
            tokio::select! {
                r0 = f0 => r0.map(|_| 0u8),
                r1 = f1 => r1.map(|_| 1u8),
            }
        };
        match r {
            Ok(win_idx) => {
                let (winner_dest, loser) = if win_idx == 0 { (d0, d1) } else { (d1, d0) };
                let _ = std::fs::remove_file(&loser);
                return Ok(winner_dest);
            }
            Err(e) => {
                last_err = e;
                let _ = std::fs::remove_file(&d0);
                let _ = std::fs::remove_file(&d1);
                if is_cancelled(&name) {
                    return Err("已取消".into());
                }
            }
        }
    }
    Err(last_err)
}

/// 单路流式下载（带进度 + 可取消）
async fn download_one(
    app: AppHandle,
    name: String,
    url: String,
    dest: PathBuf,
    client: reqwest::Client,
) -> Result<(), String> {
    // FEAT-061：连接/首字节超时。旧实现只给整单 3600s 超时，源不可达时
    //（TCP 可连、TLS 被重置或不响应）会静默挂 1 小时，UI 表现为「点了没反应」。
    let resp = tokio::time::timeout(FIRST_BYTE_TIMEOUT, client.get(&url).send())
        .await
        .map_err(|_| format!("连接/首字节超时（{}s）: {url}", FIRST_BYTE_TIMEOUT.as_secs()))?
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

// ---------------------------------------------------------------------------
// 下载源治理（FEAT-061）：多镜像候选 + 连通性自检 + 自定义源
// ---------------------------------------------------------------------------

/// 连接超时（握手阶段：覆盖「TCP 可连但 TLS 无响应」的静默挂起）
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// 首字节超时（发出请求 → 拿到响应头）
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(20);
/// 读超时（两次数据之间的最长间隔；大文件慢速下载不会被误杀）
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// 单源自检超时
const PROBE_TIMEOUT: Duration = Duration::from_secs(12);

/// 内置镜像源模板（FEAT-061）
///
/// `{repo}` = HF 仓库 id；`{path}` = 仓库内相对路径（如 `onnx/model_fp16.onnx`）。
/// 只留国内可连的镜像；**不再保留 huggingface.co 直连**（大陆基本不可达，
/// 旧实现里 official()/mirror() 返回同一地址的「双路竞速」实际是空转）。
/// 两个源的路径前缀规则不同（aifasthub 多一层 `/models`），故用整模板而非域名替换。
const BUILTIN_SOURCES: [&str; 2] = [
    "https://hf-mirror.com/{repo}/resolve/main/{path}",
    "https://aifasthub.com/models/{repo}/resolve/main/{path}",
];

/// 按模板拼下载地址
fn fill_source(tpl: &str, repo: &str, path: &str) -> String {
    tpl.replace("{repo}", repo).replace("{path}", path)
}

/// 取模板前缀（用于 base_url：去掉 {path} 尾巴，保留 resolve/main/ 前缀）
fn source_prefix(repo: &str) -> String {
    let tpls = source_templates();
    fill_source(&tpls[0], repo, "")
}

fn custom_sources_file() -> PathBuf {
    models_dir().join("download_sources.json")
}

/// 自定义源（用户在「模型下载 → 源自检」里维护；每行一个模板）
fn load_custom_sources() -> Vec<String> {
    let path = custom_sources_file();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Vec<String>>(&text) else {
        return Vec::new();
    };
    v.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| s.starts_with("http") && s.contains("{repo}") && s.contains("{path}"))
        .collect()
}

/// 保存自定义源（空数组 = 恢复仅内置）
pub fn save_custom_sources(list: Vec<String>) -> Result<(), String> {
    let cleaned: Vec<String> = list
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for s in &cleaned {
        if !(s.starts_with("http") && s.contains("{repo}") && s.contains("{path}")) {
            return Err(format!("源模板不合法（需以 http 开头且同时含 {{repo}} 与 {{path}}）: {s}"));
        }
    }
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建模型目录失败: {e}"))?;
    let text = serde_json::to_string_pretty(&cleaned).map_err(|e| e.to_string())?;
    std::fs::write(custom_sources_file(), text)
        .map_err(|e| format!("写入 download_sources.json 失败: {e}"))?;
    Ok(())
}

/// 候选源顺序：自定义优先 → 内置（去重）
fn source_templates() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in load_custom_sources()
        .into_iter()
        .chain(BUILTIN_SOURCES.iter().map(|s| s.to_string()))
    {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    if out.is_empty() {
        out.push(BUILTIN_SOURCES[0].to_string());
    }
    out
}

/// 下载结果校验：体积下限 + 首字节不是 HTML（避免把错误页/半包当成模型）
fn validate_model_file(path: &Path, expect_min: u64) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取下载文件失败: {e}"))?;
    if meta.len() < expect_min {
        return Err(format!(
            "下载文件仅 {} 字节，低于预期下限 {} 字节（疑似错误页或半包），已丢弃",
            meta.len(),
            expect_min
        ));
    }
    let mut f = std::fs::File::open(path).map_err(|e| format!("打开下载文件失败: {e}"))?;
    let mut head = [0u8; 16];
    let n = f.read(&mut head).map_err(|e| format!("读取文件头失败: {e}"))?;
    if n == 0 {
        return Err("下载文件为空，已丢弃".to_string());
    }
    let text = String::from_utf8_lossy(&head[..n]).trim_start().to_ascii_lowercase();
    if text.starts_with('<') {
        return Err("下载内容不是模型文件（疑似 HTML 错误页），已丢弃".to_string());
    }
    Ok(())
}

/// 下载源配置（内置 + 自定义）
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcesInfo {
    pub builtin: Vec<String>,
    pub custom: Vec<String>,
}

pub fn sources_info() -> SourcesInfo {
    SourcesInfo {
        builtin: BUILTIN_SOURCES.iter().map(|s| s.to_string()).collect(),
        custom: load_custom_sources(),
    }
}

/// 单个下载源的连通性自检结果
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProbe {
    pub url: String,
    pub host: String,
    pub builtin: bool,
    pub ok: bool,
    pub status: u16,
    pub ms: u64,
    pub error: Option<String>,
}

fn url_host(url: &str) -> String {
    url.split("//")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or("")
        .to_string()
}

/// 对某模型的主文件逐个源做 Range 探测（只取 1 字节，不下载整文件）
pub async fn probe_sources(name: &str) -> Result<Vec<SourceProbe>, String> {
    let Some(spec) = specs().into_iter().find(|s| s.name == name) else {
        return Err(format!("未知模型: {name}"));
    };
    let path = spec.repo_file();
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let custom = load_custom_sources();
    let mut out = Vec::new();
    for tpl in source_templates() {
        let url = fill_source(&tpl, spec.repo, &path);
        let host = url_host(&url);
        let builtin = !custom.contains(&tpl);
        let started = Instant::now();
        match client.get(&url).header("Range", "bytes=0-1").send().await {
            Ok(resp) => {
                let code = resp.status().as_u16();
                out.push(SourceProbe {
                    url,
                    host,
                    builtin,
                    ok: (200..300).contains(&code),
                    status: code,
                    ms: started.elapsed().as_millis() as u64,
                    error: None,
                });
            }
            Err(e) => out.push(SourceProbe {
                url,
                host,
                builtin,
                ok: false,
                status: 0,
                ms: started.elapsed().as_millis() as u64,
                error: Some(e.to_string()),
            }),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 提取 python 配置里某个 key 的**字符串字面量**值（`"dir": "xxx"`）。
    /// 非字符串值（如 `"dir": d,` 这种变量引用）不会被匹配。
    fn quoted_values(text: &str, key: &str) -> Vec<String> {
        let needle = format!("\"{key}\": \"");
        text.match_indices(&needle)
            .map(|(i, _)| {
                let rest = &text[i + needle.len()..];
                rest[..rest.find('"').unwrap_or(0)].to_string()
            })
            .collect()
    }

    /// 下载落位必须与 python/vcr/config.py::CLIP_MODEL_META 严格一致：
    /// 三种档位（b16 / b16-fp32 / l14）×（模型根目录 + onnx 文件名）一一对应，
    /// 且 tokenizer.json / vocab.txt 落在**模型根目录**（不是 onnx/ 子目录，
    /// 见 BUG-2026-0920-002）。
    #[test]
    fn clip_specs_match_python_tier_table() {
        let config_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("python")
            .join("vcr")
            .join("config.py");
        let text = std::fs::read_to_string(&config_path).expect("应能读取 python/vcr/config.py");

        let py_dirs = quoted_values(&text, "dir");
        let py_onnx = quoted_values(&text, "onnx");
        // 档位数由 Python 档位表决定（不硬编码：L/14 已实测否决下架，未来档位还会增减）
        assert!(!py_dirs.is_empty(), "档位表不能为空");
        assert_eq!(py_dirs.len(), py_onnx.len(), "每档必须声明 onnx 文件名：{py_onnx:?}");

        let py_repos = quoted_values(&text, "repo");
        let specs = specs();
        assert_eq!(specs.len(), py_dirs.len(), "Rust 下载表与 Python 档位表数量必须一致");
        // 仓库名也必须一致（URL 的另一半），且三个 onnx 文件名对应的 URL 均为可下载形态
        for spec in &specs {
            assert!(
                py_repos.contains(&spec.repo.to_string()),
                "Rust 下载表的仓库 {} 不在 Python 档位表中",
                spec.repo
            );
            assert!(spec.official().ends_with(&format!("/onnx/{}", spec.onnx)));
        }
        for (dir, onnx) in py_dirs.iter().zip(py_onnx.iter()) {
            let hit = specs
                .iter()
                .find(|s| s.root == dir)
                .unwrap_or_else(|| panic!("Rust 下载表缺少档位目录 {dir}"));
            assert_eq!(&hit.onnx, onnx, "档位 {dir} 的 onnx 文件名不一致");
            assert_eq!(hit.rel_file(), format!("{dir}/onnx/{onnx}"));
            // 下载 URL 必须是**仓库内**路径：onnx/<file>（不带本地落位目录名）
            // —— 回归 BUG-2026-0921-001：应用内点下载 404
            assert_eq!(hit.repo_file(), format!("onnx/{onnx}"));
            assert_eq!(
                hit.official(),
                format!("{}{}", hit.base_url(), format!("onnx/{onnx}")),
                "下载 URL 必须按仓库路径拼接"
            );
            assert!(
                !hit.official().contains(&format!("/{dir}/")),
                "下载 URL 不得包含本地落位目录名 {dir}：{}",
                hit.official()
            );
            // 附带文件必须落模型根目录（修复前落在 onnx/ 子目录 → tokenizer 找不到）
            assert!(
                hit.extra.contains(&"tokenizer.json") && hit.extra.contains(&"vocab.txt"),
                "档位 {dir} 必须附带 tokenizer.json + vocab.txt"
            );
        }
    }

    /// 下载条目自洽性：命名唯一、目录唯一、镜像=官方（防止误删/重复条目）
    #[test]
    fn clip_specs_are_self_consistent() {
        let all = specs();
        let names: Vec<&str> = all.iter().map(|s| s.name).collect();
        let roots: Vec<&str> = all.iter().map(|s| s.root).collect();
        for (label, list) in [("name", &names), ("root", &roots)] {
            let mut uniq = list.clone();
            uniq.sort_unstable();
            uniq.dedup();
            assert_eq!(uniq.len(), list.len(), "{label} 必须唯一：{list:?}");
        }
        // 已知档位必须都在表里（按目录判定，避免把"名字"当契约）
        for dir in ["chinese-clip", "chinese-clip-fp32"] {
            assert!(roots.contains(&dir), "缺少档位目录 {dir}；现有 {roots:?}");
        }
        for spec in &all {
            assert!(
                spec.mirror() == spec.official(),
                "hf-mirror 为单源直链，镜像应等于官方地址"
            );
        }
    }
}
