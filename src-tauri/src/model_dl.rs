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

/// 一个待下载文件（FEAT-114：从「单主文件」扩展为多文件）
///
/// 人脸/OCR 模型是**多个独立 onnx 落在模型根目录**（不是 CLIP 的 `<root>/onnx/` 结构），
/// 因此下载单元必须下沉到「文件」而不是「模型」。
struct DlFile {
    /// HF 仓库内相对路径（repo 非空且 abs_url 为空时使用）
    remote: &'static str,
    /// 本地落位路径（相对 python/models）
    local: &'static str,
    /// 非 HF 的绝对 URL（modelscope 等）；Some 时忽略 repo/remote
    abs_url: Option<&'static str>,
    /// FEAT-061：体积下限（字节）——识别 HTML 错误页 / 半包文件
    expect_min: u64,
}

/// 模型注册表白名单
///
/// 目录约定（与 python/vcr/config.py 严格一致）：
///
/// - 人物检测（主链路）：`yolov8n-det.onnx` 直接落模型根目录（`config.DET_MODEL`）
/// - CLIP 两档：`chinese-clip*/onnx/<onnx>` + 根目录 `tokenizer.json` / `vocab.txt`
///   （tokenizer 必须在模型根目录，见 BUG-2026-0920-002）
/// - 人脸两档：`det_*.onnx` + `w600k_*.onnx` 直接落 `python/models/`
/// - OCR：`paddleocr-det.onnx` 直接落 `python/models/`
///
/// CLIP 档位清单 = b16（fp16，默认）+ b16-fp32（可走 DirectML）。L/14-336 已于
/// 2026-09-21 实测否决并下架，故不在下载表中。
///
/// BUG-2026-1028-011：0.4.x 的发行包改用了 `release.tauri.conf.no-models.json`
/// （resources 只有 vcr-server.exe），模型全靠本表下载；而本表**漏了人物检测**，
/// 于是新装用户的 `/health.ok` 永远为 false（ok 的唯一判据就是 det），
/// 表现为「人物通道不可用 + 扫描等 90s 后报正在加载」。现在补齐并标 required。
struct DlSpec {
    name: &'static str,
    /// UI 展示名（如「语义 · B/16 fp16（推荐）」）
    label: &'static str,
    /// UI 副标题：体积 / 用途 / 取舍
    note: &'static str,
    /// HF 仓库；空字符串 = 全部走文件的绝对 URL
    repo: &'static str,
    files: &'static [DlFile],
    required: bool,
    /// 预计总字节（仅用于进度分母；实际以落盘为准）
    expect_bytes: u64,
}

impl DlSpec {
    fn main_file(&self) -> &'static DlFile {
        &self.files[0]
    }
    /// 主文件**本地**相对路径（相对 python/models；存在即视为已下载）
    fn rel_file(&self) -> String {
        self.main_file().local.to_string()
    }
    /// 主文件在**仓库内**的相对路径（用于拼下载 URL），绝不含本地落位目录名。
    /// 回归 BUG-2026-0921-001：曾误用 rel_file 拼 URL，应用内点下载全部 404。
    #[cfg(test)]
    fn repo_file(&self) -> String {
        self.main_file().remote.to_string()
    }
    /// 某文件的全部候选地址（HF 走内置多源；绝对 URL 单源）
    fn file_urls(&self, f: &DlFile) -> Vec<String> {
        if let Some(u) = f.abs_url {
            return vec![u.to_string()];
        }
        source_templates()
            .iter()
            .map(|t| fill_source(t, self.repo, f.remote))
            .collect()
    }
    /// 主文件官方地址（测试与自检用）
    #[cfg(test)]
    fn official(&self) -> String {
        self.file_urls(self.main_file())[0].clone()
    }
}

/// 模型注册表（FEAT-114：补齐此前只能通过 model.zip 获取的人脸 / OCR 模型）
///
/// 所有源均为国内可直连：HF 走 hf-mirror / aifasthub 双路竞速，
/// OCR 走阿里 ModelScope。体积取自各源 Content-Range 实测。
fn specs() -> Vec<DlSpec> {
    const CLIP_FP16: &[DlFile] = &[
        DlFile {
            remote: "onnx/model_fp16.onnx",
            local: "chinese-clip/onnx/model_fp16.onnx",
            abs_url: None,
            expect_min: 100_000_000,
        },
        DlFile {
            remote: "tokenizer.json",
            local: "chinese-clip/tokenizer.json",
            abs_url: None,
            expect_min: 1_000,
        },
        DlFile {
            remote: "vocab.txt",
            local: "chinese-clip/vocab.txt",
            abs_url: None,
            expect_min: 100,
        },
    ];
    const CLIP_FP32: &[DlFile] = &[
        DlFile {
            remote: "onnx/model.onnx",
            local: "chinese-clip-fp32/onnx/model.onnx",
            abs_url: None,
            expect_min: 100_000_000,
        },
        DlFile {
            remote: "tokenizer.json",
            local: "chinese-clip-fp32/tokenizer.json",
            abs_url: None,
            expect_min: 1_000,
        },
        DlFile {
            remote: "vocab.txt",
            local: "chinese-clip-fp32/vocab.txt",
            abs_url: None,
            expect_min: 100,
        },
    ];
    // buffalo_l 仓内是 detection/model.onnx + recognition/model.onnx，
    // 落位时必须改回 insightface 的原始文件名（python 侧按 det_10g/w600k_r50 找）。
    const FACE_PRECISE: &[DlFile] = &[
        DlFile {
            remote: "detection/model.onnx",
            local: "det_10g.onnx",
            abs_url: None,
            expect_min: 8_000_000,
        },
        DlFile {
            remote: "recognition/model.onnx",
            local: "w600k_r50.onnx",
            abs_url: None,
            expect_min: 50_000_000,
        },
    ];
    const FACE_LIGHT: &[DlFile] = &[
        DlFile {
            remote: "det_500m.onnx",
            local: "det_500m.onnx",
            abs_url: None,
            expect_min: 1_000_000,
        },
        DlFile {
            remote: "w600k_mbf.onnx",
            local: "w600k_mbf.onnx",
            abs_url: None,
            expect_min: 5_000_000,
        },
    ];
    const OCR: &[DlFile] = &[DlFile {
        remote: "",
        local: "paddleocr-det.onnx",
        abs_url: Some(
            "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/det/ch_PP-OCRv4_det_mobile.onnx",
        ),
        expect_min: 3_000_000,
    }];
    const PERSON_DET: &[DlFile] = &[DlFile {
        remote: "yolov8n-onnx-web/yolov8n.onnx",
        local: "yolov8n-det.onnx",
        abs_url: None,
        expect_min: 11_000_000,
    }];

    vec![
        DlSpec {
            name: "person-det",
            label: "人物检测 · YOLOv8n（必需）",
            note: "12.8MB · 人物/车辆检测主链路，缺它人物与扫描全部不可用",
            repo: "salim4n/yolov8n-detect-onnx",
            files: PERSON_DET,
            required: true,
            expect_bytes: 12_823_637,
        },
        DlSpec {
            name: "chinese-clip",
            label: "语义 · B/16 fp16（推荐）",
            note: "377MB · 零样本检索默认档，固定 CPU 推理；核显本首选",
            repo: "Xenova/chinese-clip-vit-base-patch16",
            files: CLIP_FP16,
            required: false,
            expect_bytes: 377_377_730 + 439_124 + 109_540,
        },
        DlSpec {
            name: "chinese-clip-fp32",
            label: "语义 · B/16 fp32（GPU）",
            note: "754MB · 体积换精度，可走 DirectML 加速",
            repo: "Xenova/chinese-clip-vit-base-patch16",
            files: CLIP_FP32,
            required: false,
            expect_bytes: 753_665_706 + 439_124 + 109_540,
        },
        DlSpec {
            name: "face-precise",
            label: "人脸 · 高精度",
            note: "191MB · det_10g + w600k_r50，误检/错挂最少（推荐）",
            repo: "immich-app/buffalo_l",
            files: FACE_PRECISE,
            required: false,
            expect_bytes: 16_923_827 + 174_383_860,
        },
        DlSpec {
            name: "face-light",
            label: "人脸 · 轻量",
            note: "16MB · det_500m + w600k_mbf，快 6~7 倍但误检更多",
            repo: "WePrompt/buffalo_sc",
            files: FACE_LIGHT,
            required: false,
            expect_bytes: 2_524_817 + 13_616_099,
        },
        DlSpec {
            name: "ocr",
            label: "文字 · OCR 检测",
            note: "4.7MB · PaddleOCR v4 det（ModelScope 国内源）",
            repo: "",
            files: OCR,
            required: false,
            expect_bytes: 4_745_517,
        },
    ]
}

/// 单模型下载状态（前端展示 + list 返回值）
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDlStatus {
    pub name: String,
    /// UI 展示名（FEAT-114）
    pub label: String,
    /// UI 副标题：体积 / 用途 / 取舍
    pub note: String,
    /// 主文件本地相对路径（兼容旧字段）
    pub file: String,
    /// 该模型需要的全部文件（相对 python/models；下载弹窗据此列出缺口）
    pub files: Vec<String>,
    pub required: bool,
    pub running: bool,
    pub done: bool,
    pub stage: String, // downloading | exporting | done | error | idle
    pub bytes: u64,
    pub total: u64,
    pub error: Option<String>,
}

impl ModelDlStatus {
    fn skeleton(spec: &DlSpec) -> Self {
        Self {
            name: spec.name.to_string(),
            label: spec.label.to_string(),
            note: spec.note.to_string(),
            file: spec.rel_file(),
            files: spec.files.iter().map(|f| f.local.to_string()).collect(),
            required: spec.required,
            running: false,
            done: false,
            stage: "idle".into(),
            bytes: 0,
            total: spec.expect_bytes,
            error: None,
        }
    }
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
    // 兜底：仅开发态（无 AppHandle 时 setup 未跑）才允许指向源码目录。
    // 收口到 devpath —— release 下返回空路径，不含构建机绝对路径（BUG-2026-1013-001）。
    let project = crate::devpath::dev_project_dir();
    project.join("python").join("models")
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
    if spec
        .files
        .iter()
        .all(|f| dir.join(f.local).is_file())
    {
        return Err(format!("{} 已全部存在", spec.label));
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
            running: true,
            stage: "downloading".into(),
            ..ModelDlStatus::skeleton(&spec)
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
            let exists = s.files.iter().all(|f| dir.join(f.local).is_file());
            st.status.get(s.name).cloned().unwrap_or_else(|| ModelDlStatus {
                done: exists,
                stage: if exists { "done".into() } else { "idle".into() },
                total: s.expect_bytes,
                ..ModelDlStatus::skeleton(&s)
            })
        })
        .collect()
}

/// 一次完整流程：逐个文件下载 → 校验 → 落位（已存在的文件跳过，支持断点续传）
async fn run(app: &AppHandle, spec: &DlSpec) -> Result<(), String> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建模型目录失败: {e}"))?;

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(Duration::from_secs(3600))
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    let mut completed = 0u64;
    for (idx, file) in spec.files.iter().enumerate() {
        let dest = dir.join(file.local);
        if dest.is_file() {
            completed += std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建模型目录失败: {e}"))?;
        }

        emit_status(app, spec, "downloading", completed, spec.expect_bytes, false);
        let winner =
            download_first_wins(app, spec, idx, file, &dir, &client, completed).await?;
        // FEAT-061：落地前做基本校验（服务端可能返回 HTML 错误页 / 半包文件）
        if let Err(e) = validate_model_file(&winner, file.expect_min) {
            let _ = std::fs::remove_file(&winner);
            // 坏文件对应的分片不能留（否则下次续传会接着拼坏字节）
            cleanup_partials(&dir, &part_key(spec.name, idx));
            return Err(e);
        }
        std::fs::rename(&winner, &dest).map_err(|e| format!("落位失败: {e}"))?;
        // FEAT-062：落位成功后清掉残留分片与元数据
        cleanup_partials(&dir, &part_key(spec.name, idx));
        completed += std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    }

    emit_status(app, spec, "downloading", completed, spec.expect_bytes, false);

    // FEAT-SEM：模型就位后立即解除语义退避（下载完即可用，不必等 TTL）
    crate::vision::clear_semantic_down();
    // CLIP 首次使用前需拆双塔：服务端自动拆（幂等），此处不阻塞下载完成
    Ok(())
}

/// 分片文件名的模型键（每个文件独立续传/取消/清理）
fn part_key(name: &str, idx: usize) -> String {
    format!("{name}_{idx}")
}

fn emit_status(app: &AppHandle, spec: &DlSpec, stage: &str, bytes: u64, total: u64, done: bool) {
    set_status(
        app,
        ModelDlStatus {
            running: !done,
            done,
            stage: stage.into(),
            bytes,
            total,
            error: None,
            ..ModelDlStatus::skeleton(spec)
        },
    );
}

/// 两路并行下载，先完成者赢；返回赢家临时文件路径（带正确扩展名）
async fn download_first_wins(
    app: &AppHandle,
    spec: &DlSpec,
    idx: usize,
    file: &DlFile,
    dir: &Path,
    client: &reqwest::Client,
    base_bytes: u64,
) -> Result<PathBuf, String> {
    let key = part_key(spec.name, idx);
    let ext = Path::new(file.local)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    let d0 = dir.join(format!(".dl_{key}_0.{ext}"));
    let d1 = dir.join(format!(".dl_{key}_1.{ext}"));
    let urls = spec.file_urls(file);
    let url0 = urls[0].clone();
    let url1 = urls.get(1).cloned().unwrap_or_else(|| url0.clone());

    // hf-mirror 对同一来源的密集请求会瞬时 403（实测：连续 HEAD/Range/GET 后 403，稍候恢复），
    // 而这里是 16MB~800MB 的大文件 —— 失败即白下，故加有限重试（3 次，退避 2s/4s）。
    let mut last_err = String::from("下载失败");
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            logger_info(&format!(
                "[model_dl] 重试第 {attempt} 次下载 {}（{}，上次：{last_err}）",
                spec.name, file.local
            ));
        }
        let f0 = download_one(
            app.clone(),
            spec,
            base_bytes,
            url0.clone(),
            d0.clone(),
            client.clone(),
        );
        // 先完成者赢：tokio::select 返回第一个完成的 future，对位的另一路被取消；
        // 单源直链（官方 == 镜像）降为单路，避免同 URL 双倍流量
        let r = if url0 == url1 {
            f0.await.map(|_| 0u8)
        } else {
            let f1 = download_one(
                app.clone(),
                spec,
                base_bytes,
                url1.clone(),
                d1.clone(),
                client.clone(),
            );
            tokio::select! {
                r0 = f0 => r0.map(|_| 0u8),
                r1 = f1 => r1.map(|_| 1u8),
            }
        };
        match r {
            Ok(win_idx) => {
                let (winner_dest, loser) = if win_idx == 0 { (d0, d1) } else { (d1, d0) };
                // 败者连同其续传元数据一起清掉，不留半包占空间
                remove_part(&loser);
                // 赢家即将改名落位，续传元数据不再需要
                let _ = std::fs::remove_file(part_meta_path(&winner_dest));
                return Ok(winner_dest);
            }
            Err(e) => {
                last_err = e;
                // FEAT-062：失败/中断不再删分片——本轮重试与下次点击都能续传；
                // 真正的坏文件（错误页/半包）由 validate_model_file 落地前拦截并清理。
                if is_cancelled(spec.name) {
                    return Err("已取消（已保留断点，可再次点击下载续传）".into());
                }
            }
        }
    }
    Err(last_err)
}

/// 单路流式下载（带进度 + 可取消）
async fn download_one(
    app: AppHandle,
    spec: &DlSpec,
    base_bytes: u64,
    url: String,
    dest: PathBuf,
    client: reqwest::Client,
) -> Result<(), String> {
    // FEAT-061：连接/首字节超时。旧实现只给整单 3600s 超时，源不可达时
    //（TCP 可连、TLS 被重置或不响应）会静默挂 1 小时，UI 表现为「点了没反应」。
    // FEAT-062：断点续传——分片存在且来源一致时用 Range 续传；换源/无元数据则重置重下。
    let mut have = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    let same_source = read_part_meta(&dest).map(|m| m.url == url).unwrap_or(false);
    if have > 0 && !same_source {
        remove_part(&dest);
        have = 0;
    }
    let mut req = client.get(&url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let resp = tokio::time::timeout(FIRST_BYTE_TIMEOUT, req.send())
        .await
        .map_err(|_| format!("连接/首字节超时（{}s）: {url}", FIRST_BYTE_TIMEOUT.as_secs()))?
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status().as_u16();
    if status == 416 {
        // 服务器拒绝该区间：通常是本地分片已等于整文件大小。
        // 不在这里判死，交给上层 validate_model_file 校验体积/文件头后决定去留。
        return Ok(());
    }
    if !resp.status().is_success() {
        return Err(format!("HTTP {status}"));
    }
    let content_len = resp.content_length().unwrap_or(0);
    let resumed = status == 206 && have > 0;
    let range_total = if resumed {
        resp.headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range_total)
    } else {
        None
    };
    let mut stream = resp.bytes_stream();
    let (mut f, mut bytes, total) = if resumed {
        let f = std::fs::OpenOptions::new()
            .append(true)
            .open(&dest)
            .map_err(|e| format!("打开分片续传失败: {e}"))?;
        (f, have, range_total.unwrap_or(have + content_len))
    } else {
        let f = std::fs::File::create(&dest).map_err(|e| format!("创建临时文件失败: {e}"))?;
        (f, 0u64, content_len)
    };
    write_part_meta(&dest, &url, total);
    // 起手先报一次进度：续传时 UI 立刻能看到「从 X MB 继续」
    emit_progress(&app, spec, base_bytes + bytes);
    let mut last_emit = Instant::now();
    while let Some(chunk) = stream.next().await {
        if is_cancelled(spec.name) {
            // FEAT-062：取消保留已下分片（下次点击「下载」从断点续传）
            let _ = f.sync_all();
            return Err("已取消（已保留断点，可再次点击下载续传）".into());
        }
        let c = chunk.map_err(|e| format!("下载中断: {e}"))?;
        f.write_all(&c).map_err(|e| format!("写文件失败: {e}"))?;
        bytes += c.len() as u64;
        if last_emit.elapsed() >= Duration::from_millis(150) {
            last_emit = Instant::now();
            emit_progress(&app, spec, base_bytes + bytes);
        }
    }
    let _ = f.sync_all();
    // 分片完整：清掉续传元数据，文件本体交上层校验与落位
    let _ = std::fs::remove_file(part_meta_path(&dest));
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

/// 取模板前缀（供测试校验 URL 形状；生产只用 `file_urls()`）
#[cfg(test)]
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
    let file = spec.main_file();
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    // 绝对 URL（ModelScope 等）没有镜像模板，单源探测；HF 走内置 + 自定义模板
    let probes: Vec<(String, bool)> = if file.abs_url.is_some() {
        vec![(file.abs_url.unwrap().to_string(), true)]
    } else {
        let custom = load_custom_sources();
        source_templates()
            .into_iter()
            .map(|tpl| {
                let builtin = !custom.contains(&tpl);
                (fill_source(&tpl, spec.repo, file.remote), builtin)
            })
            .collect()
    };
    let mut out = Vec::new();
    for (url, builtin) in probes {
        let host = url_host(&url);
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

// ---------------------------------------------------------------------------
// 断点续传（FEAT-062）
// ---------------------------------------------------------------------------

/// 分片续传元数据：只记来源 URL 与总大小
///
/// 换源即重置（不同镜像的字节不保证一致，混拼会得到坏文件）；没有元数据的分片
/// 一律视为不可信（可能是旧实现/异常中断留下的半包），直接丢弃重下。
#[derive(Serialize, serde::Deserialize)]
struct PartMeta {
    url: String,
    total: u64,
}

fn part_meta_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".meta");
    PathBuf::from(s)
}

fn read_part_meta(dest: &Path) -> Option<PartMeta> {
    let text = std::fs::read_to_string(part_meta_path(dest)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_part_meta(dest: &Path, url: &str, total: u64) {
    if let Ok(text) = serde_json::to_string(&PartMeta {
        url: url.to_string(),
        total,
    }) {
        let _ = std::fs::write(part_meta_path(dest), text);
    }
}

/// 删除分片 + 其续传元数据
fn remove_part(dest: &Path) {
    let _ = std::fs::remove_file(dest);
    let _ = std::fs::remove_file(part_meta_path(dest));
}

/// 清掉某模型的所有残留分片（成功落位 / 校验失败时调用）
fn cleanup_partials(dir: &Path, name: &str) {
    let prefix = format!(".dl_{name}_");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// 解析 `Content-Range: bytes 100-999/1000` 的总大小
fn parse_content_range_total(v: &str) -> Option<u64> {
    v.rsplit('/').next()?.trim().parse::<u64>().ok()
}

/// 统一的下载进度上报（续传起点与流式写入共用）
fn emit_progress(app: &AppHandle, spec: &DlSpec, bytes: u64) {
    let _ = app.emit(
        "model-dl-progress",
        &ModelDlStatus {
            running: true,
            stage: "downloading".into(),
            bytes,
            ..ModelDlStatus::skeleton(spec)
        },
    );
}


pub mod commands {

// =====================================================================
// 以下命令自 lib.rs 迁入（lib.rs 瘦身）：模型下载命令层
// =====================================================================


/// FEAT-052：开始下载模型（后台，官方/镜像择一快者）
#[tauri::command]
pub async fn start_model_download(name: String, app: tauri::AppHandle) -> Result<(), String> {
    let _t = log_call!("start_model_download", &format!("name={name}"));
    let r = crate::model_dl::start(&app, &name).await;
    match &r {
        Ok(_) => crate::logger::log_call_end_with("start_model_download", _t, "OK"),
        Err(e) => crate::logger::log_call_end_with("start_model_download", _t, &format!("ERR | {e}")),
    }
    r
}


/// FEAT-052：模型下载状态列表
#[tauri::command]
pub fn list_model_downloads() -> Vec<crate::model_dl::ModelDlStatus> {
    crate::model_dl::list()
}


/// FEAT-052：取消模型下载
#[tauri::command]
pub fn cancel_model_download(name: String) -> Result<(), String> {
    crate::model_dl::cancel(&name);
    Ok(())
}


/// FEAT-061：下载源自检（逐个候选源 Range 探测，返回 HTTP 状态与耗时）
#[tauri::command]
pub async fn probe_model_sources(name: String) -> Result<Vec<crate::model_dl::SourceProbe>, String> {
    let _t = log_call!("probe_model_sources", &format!("name={name}"));
    let r = crate::model_dl::probe_sources(&name).await;
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "probe_model_sources",
            _t,
            &format!(
                "OK | {} 个源 / 可用 {}",
                v.len(),
                v.iter().filter(|p| p.ok).count()
            ),
        ),
        Err(e) => crate::logger::log_call_end_with("probe_model_sources", _t, &format!("ERR | {e}")),
    }
    r
}


/// FEAT-061：读取下载源配置（内置 + 自定义）
#[tauri::command]
pub fn get_model_sources() -> crate::model_dl::SourcesInfo {
    crate::model_dl::sources_info()
}


/// FEAT-061：保存自定义下载源（空数组 = 恢复默认）
#[tauri::command]
pub fn set_model_sources(sources: Vec<String>) -> Result<(), String> {
    crate::model_dl::save_custom_sources(sources)
}

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

    fn config_py() -> String {
        let config_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("python")
            .join("vcr")
            .join("config.py");
        std::fs::read_to_string(&config_path).expect("应能读取 python/vcr/config.py")
    }

    /// CLIP 落位必须与 python/vcr/config.py::CLIP_MODEL_META 严格一致：
    /// （模型根目录 + onnx 文件名）一一对应，且 tokenizer.json / vocab.txt 落在
    /// **模型根目录**（不是 onnx/ 子目录，见 BUG-2026-0920-002）。
    #[test]
    fn clip_specs_match_python_tier_table() {
        let text = config_py();
        let py_dirs = quoted_values(&text, "dir");
        let py_onnx = quoted_values(&text, "onnx");
        assert!(!py_dirs.is_empty(), "档位表不能为空");
        assert_eq!(py_dirs.len(), py_onnx.len(), "每档必须声明 onnx 文件名：{py_onnx:?}");

        let py_repos = quoted_values(&text, "repo");
        let all = specs();
        let clip: Vec<&DlSpec> = all
            .iter()
            .filter(|s| s.name.starts_with("chinese-clip"))
            .collect();
        assert_eq!(clip.len(), py_dirs.len(), "Rust 下载表与 Python 档位表数量必须一致");
        for spec in &clip {
            assert!(
                py_repos.contains(&spec.repo.to_string()),
                "Rust 下载表的仓库 {} 不在 Python 档位表中",
                spec.repo
            );
        }
        for (dir, onnx) in py_dirs.iter().zip(py_onnx.iter()) {
            let hit = clip
                .iter()
                .find(|s| s.main_file().local == format!("{dir}/onnx/{onnx}"))
                .unwrap_or_else(|| panic!("Rust 下载表缺少档位 {dir}/onnx/{onnx}"));
            // 下载 URL 必须是**仓库内**路径：onnx/<file>（不带本地落位目录名）
            // —— 回归 BUG-2026-0921-001：应用内点下载 404
            assert_eq!(hit.repo_file(), format!("onnx/{onnx}"));
            assert_eq!(
                hit.official(),
                format!("{}{}", source_prefix(hit.repo), format!("onnx/{onnx}")),
                "下载 URL 必须按仓库路径拼接"
            );
            assert!(
                !hit.official().contains(&format!("/{dir}/")),
                "下载 URL 不得包含本地落位目录名 {dir}：{}",
                hit.official()
            );
            // tokenizer / vocab 必须落模型根目录（修复前落在 onnx/ 子目录 → 找不到）
            let locals: Vec<&str> = hit.files.iter().map(|f| f.local).collect();
            let tok = format!("{dir}/tokenizer.json");
            let voc = format!("{dir}/vocab.txt");
            assert!(
                locals.contains(&tok.as_str()) && locals.contains(&voc.as_str()),
                "档位 {dir} 的 tokenizer/vocab 必须落模型根目录：{locals:?}"
            );
        }
    }

    /// 人脸 / OCR 落位必须与 python/vcr/config.py 的档位表一致：
    /// 文件名错了会「下载成功但通道仍显示未下载」。
    #[test]
    fn face_and_ocr_specs_match_python_config() {
        let text = config_py();
        let mut expect: Vec<String> = quoted_values(&text, "det");
        expect.extend(quoted_values(&text, "rec"));
        assert!(text.contains("OCR_MODEL = \"paddleocr-det.onnx\""));
        expect.push("paddleocr-det.onnx".into());
        // 人物检测（主链路，/health.ok 的唯一判据）：必须能在应用内下载到。
        // BUG-2026-1028-011：此前漏登记，轻量安装包用户永远补不齐 det。
        assert!(
            text.contains("DET_MODEL = \"yolov8n-det.onnx\""),
            "python 侧 DET_MODEL 改名后须同步本测试"
        );
        expect.push("yolov8n-det.onnx".into());

        let all = specs();
        let locals: Vec<String> = all
            .iter()
            .flat_map(|s| s.files.iter().map(|f| f.local.to_string()))
            .collect();
        for name in expect {
            assert!(
                locals.iter().any(|l| l == &name),
                "Rust 下载表缺少本地文件 {name}；现有 {locals:?}"
            );
        }
    }


    /// 下载条目自洽性：命名/落位唯一、HF 文件走多镜像、绝对 URL 单源、体积下限合理。
    #[test]
    fn download_specs_are_self_consistent() {
        let all = specs();
        let names: Vec<&str> = all.iter().map(|s| s.name).collect();
        let mut uniq_names = names.clone();
        uniq_names.sort_unstable();
        uniq_names.dedup();
        assert_eq!(uniq_names.len(), names.len(), "name 必须唯一：{names:?}");

        let mut locals: Vec<&str> = all
            .iter()
            .flat_map(|s| s.files.iter().map(|f| f.local))
            .collect();
        let total = locals.len();
        locals.sort_unstable();
        locals.dedup();
        assert_eq!(locals.len(), total, "本地落位路径不得重复：{locals:?}");

        for spec in &all {
            assert!(!spec.label.is_empty() && !spec.note.is_empty(), "{} 缺少 UI 文案", spec.name);
            assert!(spec.expect_bytes > 0, "{} 缺少预计体积", spec.name);
            for file in spec.files {
                assert!(file.expect_min > 0, "{} 的体积下限必须为正", spec.name);
                assert!(
                    file.expect_min <= spec.expect_bytes,
                    "{} 的单文件下限不应超过模型总体积",
                    spec.name
                );
                let urls = spec.file_urls(file);
                assert!(!urls.is_empty(), "{} 没有可用下载地址", spec.name);
                for url in &urls {
                    assert!(url.starts_with("http"), "地址必须是 http：{url}");
                }
                match file.abs_url {
                    Some(u) => {
                        assert_eq!(urls.len(), 1, "绝对 URL 不应再套镜像模板：{u}");
                        assert_eq!(urls[0], u);
                    }
                    None => {
                        assert!(spec.repo.contains('/'), "{} 缺少 HF 仓库名", spec.name);
                        // FEAT-061：HF 源至少两个，且都指向同一仓库内路径
                        assert!(urls.len() >= 2, "HF 文件应至少两个镜像源：{urls:?}");
                        for url in &urls {
                            assert!(
                                url.contains(file.remote),
                                "URL 应包含仓库内路径 {}：{url}",
                                file.remote
                            );
                        }
                        assert_ne!(urls[0], urls[1], "多源下两个地址不应相同");
                    }
                }
            }
        }
    }

    /// 内置源：至少两个、互不重复，且都是国内可连的镜像模板。
    #[test]
    fn builtin_sources_are_sane() {
        assert!(BUILTIN_SOURCES.len() >= 2);
        let mut uniq: Vec<&str> = BUILTIN_SOURCES.to_vec();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), BUILTIN_SOURCES.len(), "内置源模板不得重复");
        for tpl in BUILTIN_SOURCES {
            assert!(tpl.starts_with("https://"), "内置源必须是 https：{tpl}");
            assert!(tpl.contains("{repo}") && tpl.contains("{path}"), "模板缺占位符：{tpl}");
        }
    }
}
