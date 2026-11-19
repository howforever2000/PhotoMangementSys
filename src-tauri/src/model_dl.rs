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
    /// 某文件的全部候选地址（HF 走内置多源；GitHub 直链展开加速镜像；其余单源）
    ///
    /// 顺序 = 实测吞吐从快到慢（2026-11-19 本机实测，单连接 / 4 连接）：
    ///   ghfast.top      0.30~0.33 / 0.77 MB/s
    ///   gh-proxy.com    0.17 / 0.41 MB/s
    ///   github.com 直连 0.06~0.14 MB/s
    ///
    /// 为什么必须展开而不是只用直连：创意工坊 4 个本地增强模型全是 GitHub Release
    /// 直链（`abs_url`），单源路径下「双路竞速」根本不生效，340MB 的 GFPGAN 要下 90 分钟。
    fn file_urls(&self, f: &DlFile) -> Vec<String> {
        let Some(u) = f.abs_url else {
            return source_templates()
                .iter()
                .map(|t| fill_source(t, self.repo, f.remote))
                .collect();
        };
        expand_github_mirrors(u)
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
    // FEAT-137：语义模型只留一档，且用 **fp32 权重**。
    //
    // 删掉 fp16 档的依据（真实 256px 缩略图，批 8 / 批 32，实测）：
    //   fp16/CPU  74.4 / 113.7 ms/张
    //   fp32/CPU  60.6 /  76.4 ms/张   ← 比 fp16 快 18%~33%（x86 上 ORT 跑 fp16 要反复 cast）
    //   两档精度等价：32 张图 cos 最小 0.99999887
    // 且 fp16 档又永远拿不到 GPU（fp16 在 DirectML 上有算子级数值 bug）⇒ 旧默认档双输。
    //
    // 落位目录沿用 `chinese-clip-fp32/`：已经下过 fp32 档的用户不用重下 719MB；
    // 下过 fp16 档的用户需重下一次（旧的文件成了垃圾，可在设置里清理）。
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
    // FEAT-136：OpenVINO 裁剪运行时 —— **可选组件**，不是模型。
    //
    // 它服务的是 det / ocr / face_rec 三条热通道（实测 983 张真扫描 -58.4%，
    // 见 docs/FEAT-135）。不装也完全正常：`config.accel_setting()` 返回未启用，
    // 全部回落 ONNX Runtime，行为与今天逐字一致。
    //
    // 为什么是一个 zip：下载单元仍是「文件」，Rust 不引 zip 依赖；解压交给 Python
    // 标准库 zipfile（见 vcr/ov_backend.py::ensure_runtime_extracted），失败面只有一处且可重试。
    //
    // 为什么只给 Intel 推荐：OpenVINO 的 NPU 插件只认 Intel 硬件（UI 层按 CPU 厂商过滤）。
    //
    // 尺寸口径：完整 openvino 244.8MB → 裁掉编译器(95.7MB)+CPU 插件(44.2MB)+GPU 插件(33.1MB)
    // +多余前端后 **45.4MB 装上 / 15.5MB zip**；裁剪版已实测能 compile_model(onnx, "NPU")，
    // 且输出与完整版一致（det 53/53 人框零差异、r50 cos 0.99997220）。
    // expect_min 只防 HTML 错误页，不当作精确体积。
    const OPENVINO_RUNTIME: &[DlFile] = &[DlFile {
        remote: "",
        local: "_runtime/openvino-npu-win-x64.zip",
        // releases/latest/download 是 GitHub 的稳定别名，不随版本号改 URL；
        // 资产由 python/scripts/build_ov_runtime_zip.py 产出并随 release 上传。
        abs_url: Some(
            "https://github.com/howforever2000/PhotoMangementSys/releases/latest/download/openvino-npu-win-x64.zip",
        ),
        expect_min: 5_000_000,
    }];
    const PERSON_DET: &[DlFile] = &[DlFile {
        remote: "yolov8n-onnx-web/yolov8n.onnx",
        local: "yolov8n-det.onnx",
        abs_url: None,
        expect_min: 11_000_000,
    }];

    // 本地增强大模型（FEAT-164）：创意工坊「工具」组的画质修复 + AI 智能修图的
    // 本地组件共用同一批权重。
    //
    // 为什么进本表而不是继续「给外链让用户放文件」：
    //   - 扫描侧已经有的能力（后台下载 / 进度 / 断点续传 / 取消 / 自定义源）在这里
    //     全部复用，用户不必自己找目录、手动改名；
    //   - 落位目录 `local/` 与 python/ai_studio/config.py 的 LOCAL_MODELS_DIR 严格一致，
    //     且 ai_service 会把同一个模型根注入 AI_STUDIO_MODELS_DIR，避免「下完找不到」。
    //
    // 源为 facefusion-assets 的稳定 release 资产（与 python/ai_studio/config.py 的
    // _RELEASE 同一条），体积取自 release 页实测。
    const LOCAL_ESRGAN: &[DlFile] = &[DlFile {
        remote: "",
        local: "local/real_esrgan_x4.onnx",
        abs_url: Some(
            "https://github.com/facefusion/facefusion-assets/releases/download/models-3.0.0/real_esrgan_x4.onnx",
        ),
        expect_min: 60_000_000,
    }];
    const LOCAL_GFPGAN: &[DlFile] = &[DlFile {
        remote: "",
        local: "local/gfpgan_1.4.onnx",
        abs_url: Some(
            "https://github.com/facefusion/facefusion-assets/releases/download/models-3.0.0/gfpgan_1.4.onnx",
        ),
        expect_min: 250_000_000,
    }];
    // 本地美颜（FEAT-165）：BiSeNet ResNet-34 十九类人脸解析。
    // 源为 yakhyo/face-parsing 的 weights tag（MIT，与 facefusion 的
    // bisenet_resnet_34.onnx 同一套权重，但许可标注更明确）。
    const LOCAL_FACEPARSE: &[DlFile] = &[DlFile {
        remote: "",
        local: "local/face_parsing_bisenet_r34.onnx",
        abs_url: Some(
            "https://github.com/yakhyo/face-parsing/releases/download/weights/resnet34.onnx",
        ),
        expect_min: 80_000_000,
    }];
    // 本地形变（FEAT-167）：peppa_wutz 68 点人脸关键点。
    // 与 python/ai_studio/config.py 的 LOCAL_MODELS 条目一一对应（落位同为 local/）。
    const LOCAL_LANDMARK: &[DlFile] = &[DlFile {
        remote: "",
        local: "local/peppa_wutz.onnx",
        abs_url: Some(
            "https://github.com/facefusion/facefusion-assets/releases/download/models-3.0.0/peppa_wutz.onnx",
        ),
        expect_min: 12_000_000,
    }];
    // 文字提取（FEAT-171）：PP-OCRv4 识别小模型（工坊区，local- 前缀）。
    // CTC 字典 ppocr_keys_v1.txt 随包内置不下载（换字典是漂移源）；
    // det 模型不在这里 —— 它是扫描区共享件（rust 名 "ocr"）。
    const LOCAL_OCR_REC: &[DlFile] = &[DlFile {
        remote: "",
        local: "local/paddleocr-rec.onnx",
        abs_url: Some(
            "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/rec/ch_PP-OCRv4_rec_mobile.onnx",
        ),
        expect_min: 9_000_000,
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
            name: "chinese-clip-fp32",
            label: "语义模型 · B/16",
            note: "754MB · 零样本检索唯一档（fp32 权重；验收：CPU 比旧 fp16 快 18~33%）",
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
        DlSpec {
            name: "openvino-runtime",
            label: "推理加速 · OpenVINO（Intel 可选）",
            note: "15MB 下载 / 45MB 装后 · 仅 Intel：检测/OCR/人脸编码可走 NPU，实测整批 -58%",
            repo: "",
            files: OPENVINO_RUNTIME,
            required: false,
            expect_bytes: 15_000_000,
        },
        // 本地增强：创意工坊「工具」组画质修复与 AI 本地组件共用（FEAT-164）
        DlSpec {
            name: "local-esrgan",
            label: "本地增强 · Real-ESRGAN ×4 超分",
            note: "69MB · 本机超分（画质修复 / AI 本地组件），无需联网、免费",
            repo: "",
            files: LOCAL_ESRGAN,
            required: false,
            expect_bytes: 69_000_000,
        },
        DlSpec {
            name: "local-gfpgan",
            label: "本地增强 · GFPGAN 人脸修复",
            note: "340MB · 本机人脸修复（配合超分用，可选）",
            repo: "",
            files: LOCAL_GFPGAN,
            required: false,
            expect_bytes: 340_000_000,
        },
        // 本地美颜（FEAT-165）：只做磨皮时也能单独装，不强制连带超分
        DlSpec {
            name: "local-faceparse",
            label: "本地美颜 · 人脸解析",
            note: "93MB · 分区磨皮用（识别人脸皮肤/五官），只影响本地美颜",
            repo: "",
            files: LOCAL_FACEPARSE,
            required: false,
            expect_bytes: 93_632_554,
        },
        // 本地形变（FEAT-167）：瘦脸 / 大眼的关键点
        DlSpec {
            name: "local-landmark",
            label: "本地形变 · 68 点关键点",
            note: "14MB · 瘦脸/大眼用（定位下颌线与五官），只影响人像形变",
            repo: "",
            files: LOCAL_LANDMARK,
            required: false,
            expect_bytes: 13_755_190,
        },
        // 文字提取（FEAT-171）：工坊区识别小模型（det 在扫描区，模型管理下载）
        DlSpec {
            name: "local-ocr-rec",
            label: "文字提取 · OCR 识别",
            note: "10.4MB · 文字提取用（认字，配合检测模型出可复制文本），只影响文字提取",
            repo: "",
            files: LOCAL_OCR_REC,
            required: false,
            expect_bytes: 10_857_958,
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
    /// 「正在下载」的权威集合（与 status.running 的区别见 `begin_inflight`）
    inflight: std::collections::HashSet<String>,
}
static STORE: std::sync::LazyLock<Mutex<Store>> = std::sync::LazyLock::new(|| Mutex::new(Store::default()));

/// 在持锁状态下登记「正在下载」，防止并发 `start` 双开（TOCTOU）。
///
/// 为什么不能只看 `status.running`：首次下载时 `status` 里**还没有**该模型的条目
/// （`list()` 只做 clone，`set_status` 才 insert），两次快速调用都会读到 false，
/// 于是同一模型起两个任务交错写同一份 `.dl_*` 分片、cancel 标记互相覆盖。
fn begin_inflight(st: &mut Store, name: &str) -> Result<(), String> {
    if st.inflight.contains(name) || st.status.get(name).map(|s| s.running).unwrap_or(false) {
        return Err("该模型正在下载中".into());
    }
    st.inflight.insert(name.to_string());
    st.cancel.insert(name.to_string(), Arc::new(AtomicBool::new(false)));
    Ok(())
}

/// 模型根目录（下载落位与各 Python 服务读取的唯一口径）
///
/// `pub` 的原因：ai_service 拉起 ai_studio 子进程时要把同一个根注入
/// `AI_STUDIO_MODELS_DIR`，否则打包版两侧各算一套路径（下完找不到）。
pub fn models_dir() -> PathBuf {
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
        begin_inflight(&mut st, name)?;
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
            st.inflight.remove(&name);
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

/// 下载一个文件：**大文件走分段并行（多镜像同时出力）**，其余走单流竞速。
///
/// 快路径顺序（2026-11-19 实测为依据）：
///   1. 探测总长 + 是否支持 Range；
///   2. ≥16MB 且支持 Range → 切成 4 段、轮转分给各镜像并发拉，再按序拼回
///      （实测 ghfast 单连接 0.33 MB/s → 4 连接 0.77 MB/s）；
///   3. 否则 / 失败 → 逐个候选源单流下载，先完成者赢（原有行为，保留作兜底）。
/// 返回赢家临时文件路径（带正确扩展名）。
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
    let urls = dedup_urls(spec.file_urls(file));
    if urls.is_empty() {
        return Err(format!("{} 没有可用下载地址", spec.name));
    }

    // --- 快路径：分段并行（仅大文件，且源必须支持 Range）---
    // 探测也要容错：首个镜像可能刚好被限流（实测 403/429/502），
    // 因此按候选顺序挨个试，任一源能给出总长即可进入分段。
    match probe_size_any(client, &urls).await {
        Ok(total) if total >= crate::model_dl_seg::SEG_MIN_BYTES => {
            logger_info(&format!(
                "[model_dl] {} 分段并行：{:.1}MB / {} 源",
                spec.name,
                total as f64 / 1e6,
                urls.len()
            ));
            match download_segmented(app, spec, &urls, total, dir, &key, ext, client, base_bytes)
                .await
            {
                Ok(p) => return Ok(p),
                Err(e) if is_cancelled(spec.name) => return Err(e),
                Err(e) => logger_info(&format!(
                    "[model_dl] {} 分段失败，回落单流：{e}",
                    spec.name
                )),
            }
        }
        Ok(_) => {}
        Err(e) => logger_info(&format!("[model_dl] {} 体积探测失败，继续尝试单流：{e}", spec.name)),
    }

    // --- 兜底：逐个候选源单流下载，先完成者赢 ---
    // hf-mirror / 加速镜像对密集请求会瞬时 403/429（实测），大文件失败即白下，
    // 故加有限重试（3 次，退避 2s/4s）。
    let mut last_err = String::from("下载失败");
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            logger_info(&format!(
                "[model_dl] 重试第 {attempt} 次下载 {}（{}，上次：{last_err}）",
                spec.name, file.local
            ));
        }
        // 每轮从「本轮该优先的源」开始轮转，让各镜像都得到机会
        let order = rotate(&urls, attempt as usize);
        let d0 = dir.join(format!(".dl_{key}_0.{ext}"));
        let d1 = dir.join(format!(".dl_{key}_1.{ext}"));
        let u0 = order[0].clone();
        let u1 = order.get(1).cloned().unwrap_or_else(|| u0.clone());
        let f0 = download_one(app.clone(), spec, base_bytes, u0.clone(), d0.clone(), client.clone());
        // 先完成者赢：任一成功即返回；单源直链（u0 == u1）退化为单路，避免双倍流量
        let r = if u0 == u1 {
            f0.await.map(|_| 0u8)
        } else {
            let f1 =
                download_one(app.clone(), spec, base_bytes, u1.clone(), d1.clone(), client.clone());
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

/// 去掉重复地址（同一 URL 出现两次会让两路竞速变成「自己跟自己抢」）
fn dedup_urls(urls: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(urls.len());
    for u in urls {
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

/// 把候选源轮转 `n` 位（让重试/多轮不再永远先试同一个最慢的源）
fn rotate(urls: &[String], n: usize) -> Vec<String> {
    if urls.is_empty() {
        return Vec::new();
    }
    let k = n % urls.len();
    urls[k..].iter().chain(urls[..k].iter()).cloned().collect()
}

/// 探测文件总大小，并验证源确实支持 Range 分段（返回 `(总长, 是否支持 Range)`）。
///
/// 只发 1 字节的 Range 请求：既拿到总长（`Content-Range: bytes 0-0/TOTAL`），
/// 又顺带验证「服务器会不会老实返回 206」——不支持分段的源在这里就被识别出来，
/// 免得把整个文件切成 4 段后才发现每段都拿回完整文件。
async fn probe_size(client: &reqwest::Client, url: &str) -> Result<(u64, bool), String> {
    let resp = tokio::time::timeout(
        FIRST_BYTE_TIMEOUT,
        client.get(url).header("Range", "bytes=0-0").send(),
    )
    .await
    .map_err(|_| format!("体积探测超时（{}s）: {url}", FIRST_BYTE_TIMEOUT.as_secs()))?
    .map_err(|e| format!("体积探测失败: {e}"))?;
    let status = resp.status().as_u16();
    if status != 206 {
        return Err(format!("体积探测没有得到 206（HTTP {status}）: {url}"));
    }
    let total = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_range_total)
        .ok_or_else(|| format!("响应缺少 Content-Range，无法确定大小: {url}"))?;
    Ok((total, true))
}

/// 按候选顺序探测总长：任一源能给出大小即可（首个镜像可能正被限流）。
async fn probe_size_any(client: &reqwest::Client, urls: &[String]) -> Result<u64, String> {
    let mut last = String::from("没有可用下载地址");
    for url in urls {
        match probe_size(client, url).await {
            Ok((total, _)) => return Ok(total),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 分段并行下载编排：切段 → 并发拉 → 按序拼 → 返回合并后的临时文件。
///
/// 进度按 150ms 节流上报（4 路并发每 64KB 回调一次，不节流会刷爆事件通道）。
async fn download_segmented(
    app: &AppHandle,
    spec: &DlSpec,
    urls: &[String],
    total: u64,
    dir: &Path,
    key: &str,
    ext: &str,
    client: &reqwest::Client,
    base_bytes: u64,
) -> Result<PathBuf, String> {
    let plan = crate::model_dl_seg::plan_segments(total, urls);
    if plan.is_empty() {
        return Err("分段计划为空".into());
    }
    let counter = Arc::new(crate::model_dl_seg::ByteCounter::default());
    let throttle = Arc::new(crate::model_dl_seg::ProgressThrottle::default());
    // 起手先报一次，点下去界面立刻有反应
    emit_progress(app, spec, base_bytes);

    let name = spec.name.to_string();
    let app_cb = app.clone();
    // 进度回调需要跨线程持有 spec 的最小信息（label/note/总量），
    // 直接把 DlSpec 的静态字段搬进一个轻量快照，避免在闭包里借住 spec 生命周期。
    let snap = ProgressSnapshot::from_spec(spec);
    let counter_cb = counter.clone();
    let on_bytes: Arc<dyn Fn(u64) + Send + Sync> = Arc::new(move |delta: u64| {
        counter_cb.add(delta);
        if throttle.should_emit() {
            snap.emit(&app_cb, base_bytes + counter_cb.get());
        }
    });
    let cancel_name = name.clone();
    let cancel: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || is_cancelled(&cancel_name));

    let merged =
        crate::model_dl_seg::fetch_and_concat(client, plan, dir, key, ext, cancel, on_bytes).await?;
    // 收尾补报一次，避免最后一小段没赶上节流窗口
    emit_progress(app, spec, base_bytes + counter.get());
    // 合并完成后清掉所有分片与续传元数据（清理键与单流路径共用同一前缀）
    cleanup_partials(dir, &spec.name);
    Ok(merged)
}

/// 进度上报所需的 spec 快照（跨线程闭包持有，避免借住 `&DlSpec` 的生命周期）
struct ProgressSnapshot {
    name: &'static str,
    label: &'static str,
    note: &'static str,
    file: String,
    files: Vec<String>,
    required: bool,
    total: u64,
}

impl ProgressSnapshot {
    fn from_spec(spec: &DlSpec) -> Self {
        Self {
            name: spec.name,
            label: spec.label,
            note: spec.note,
            file: spec.rel_file(),
            files: spec.files.iter().map(|f| f.local.to_string()).collect(),
            required: spec.required,
            total: spec.expect_bytes,
        }
    }

    fn emit(&self, app: &AppHandle, bytes: u64) {
        let _ = app.emit(
            "model-dl-progress",
            &ModelDlStatus {
                name: self.name.to_string(),
                label: self.label.to_string(),
                note: self.note.to_string(),
                file: self.file.clone(),
                files: self.files.clone(),
                required: self.required,
                running: true,
                done: false,
                stage: "downloading".into(),
                bytes,
                total: self.total,
                error: None,
            },
        );
    }
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

/// GitHub Release 资产的加速前缀（按实测吞吐从快到慢排列，2026-11-19 本机）
///
/// 只有 `github.com` / `objects.githubusercontent.com` 的直链才套用；
/// ModelScope 等其它绝对 URL 原样返回（它们本身就是国内直连快源）。
const GITHUB_MIRRORS: [&str; 2] = [
    "https://ghfast.top/",
    "https://gh-proxy.com/",
];

/// 把 GitHub 绝对直链展开成「加速镜像优先 + 直连兜底」的候选列表。
///
/// 为什么不只留镜像：镜像站是第三方公益服务，随时可能下线或限流；
/// 直连虽慢（0.06~0.14 MB/s）但永远是最权威的兜底，保住它才不会把下载能力押在别人身上。
fn expand_github_mirrors(url: &str) -> Vec<String> {
    if !is_github_download(url) {
        return vec![url.to_string()];
    }
    let mut out: Vec<String> = GITHUB_MIRRORS
        .iter()
        .map(|p| format!("{p}{url}"))
        .collect();
    out.push(url.to_string());
    out
}

/// 判断是否为 GitHub 上可被通用代理加速的下载直链
fn is_github_download(url: &str) -> bool {
    const HOSTS: [&str; 3] = [
        "https://github.com/",
        "https://objects.githubusercontent.com/",
        "https://raw.githubusercontent.com/",
    ];
    HOSTS.iter().any(|h| url.starts_with(h))
}

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

/// FEAT-153：NPU 硬件探测（结果进程内缓存，只探一次）。
/// 供性能设置决定是否显示 openvino-runtime 下载按钮 —— 运行时未装时
/// Python 侧 /accel 的 available.NPU 恒为 false（鸡生蛋），必须走硬件探测。
#[tauri::command]
pub fn detect_npu_hardware() -> bool {
    crate::model_dl::npu_hardware_cached()
}

}

// =====================================================================
// NPU 硬件探测（FEAT-153）
// =====================================================================

/// NPU 硬件探测缓存：本会话只探一次（CPU/设备热插拔不作为场景）
pub fn npu_hardware_cached() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(npu_hardware_probe)
}

/// 探测本体：OpenVINO 的 NPU 插件只认 Intel（上面 specs() 注释落地的实现），
/// 故先验 CPU 厂商，再查设备管理器里是否有 NPU 设备。
fn npu_hardware_probe() -> bool {
    cpu_is_intel() && npu_device_present()
}

/// CPU 厂商是否 GenuineIntel（reg query 读注册表，开销小于拉起 PowerShell）
fn cpu_is_intel() -> bool {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("reg")
            .args([
                "query",
                r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
                "/v",
                "VendorIdentifier",
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("GenuineIntel"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::fs::read_to_string("/proc/cpuinfo")
            .map(|s| s.contains("GenuineIntel"))
            .unwrap_or(false)
    }
}

/// 设备管理器是否存在 NPU 设备：Intel NPU 在 Win11「神经处理器」类下
/// 名为「Intel(R) AI Boost」，部分驱动版本显示为含 NPU 的名称 —— 两种口径都认。
/// 用 WQL 服务端过滤（-Filter），避免枚举全部 PnP 实体的数秒级开销。
fn npu_device_present() -> bool {
    #[cfg(target_os = "windows")]
    {
        let script = "Get-CimInstance Win32_PnPEntity -Filter \"Name LIKE '%AI Boost%' OR Name LIKE '%NPU%'\" | Select-Object -First 1 -ExpandProperty Name";
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", script])
            .output()
            .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
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

    fn ai_studio_config_py() -> String {
        let config_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("python")
            .join("ai_studio")
            .join("config.py");
        std::fs::read_to_string(&config_path).expect("应能读取 python/ai_studio/config.py")
    }

    /// 截取 `LOCAL_MODELS = [` 到下一个顶层 `]` 之间的文本。
    ///
    /// 必须限定范围：`LUTS` 清单里也有 `"file"` 键（`.cube` 文件名），
    /// 全局扫 `"file"` 会把胶片 LUT 误判成模型权重。
    fn local_models_block(text: &str) -> String {
        let start = text.find("LOCAL_MODELS = [").expect("python 侧应有 LOCAL_MODELS 清单");
        let rest = &text[start..];
        let end = rest.find("\n]").map(|i| i + 2).unwrap_or(rest.len());
        rest[..end].to_string()
    }

    /// 本地增强模型三处同步守卫（`knowledge/local-enhance.md` §5 的纪律）。
    ///
    /// 新增本地模型必须在三处登记：`python/ai_studio/config.py` 的 LOCAL_MODELS、
    /// 本文件的 `LOCAL_*` 下载表、前端 `LocalModelHint.vue` 的 `DL_NAMES`。
    /// 漏了会怎样：设置里显示「已下载」但工具报「模型未就绪」，或者根本找不到下载入口 ——
    /// 两者都不报错，只表现为功能性缺失（BUG-2026-1112-005 就是这么来的）。
    #[test]
    fn local_models_match_ai_studio_config() {
        let block = local_models_block(&ai_studio_config_py());
        let all = specs();
        let locals: Vec<String> = all
            .iter()
            .flat_map(|s| s.files.iter().map(|f| f.local.to_string()))
            .collect();

        // Python 侧 LOCAL_MODELS 的每个 "file" 都必须在 Rust 下载表里可下载
        let files = quoted_values(&block, "file");
        assert!(!files.is_empty(), "LOCAL_MODELS 解析为空，测试本身失效");
        for file in &files {
            let want = format!("local/{file}");
            assert!(
                locals.iter().any(|l| l == &want),
                "python LOCAL_MODELS 声明了 {file}，但 Rust 下载表没有 {want}；现有 {locals:?}"
            );
        }

        // 反向：Rust 的 local-* 条目必须都能在 python 侧找到同名 file
        for spec in all.iter().filter(|s| s.name.starts_with("local-")) {
            for file in spec.files {
                let name = file.local.trim_start_matches("local/");
                assert!(
                    files.iter().any(|f| f == name),
                    "Rust 下载项 {} 的 {name} 未在 python LOCAL_MODELS 登记；现有 {files:?}",
                    spec.name
                );
            }
        }
    }

    /// FEAT-137：向量空间标签（单档 fp32）必须与 python 侧逐字一致。
    ///
    /// 三个字符串跨语言对齐：python `CLIP_MODEL_META["b16"].id` 与 `CLIP_RETIRED_IDS`，
    /// Rust `db::embedding::CLIP_MODEL_ID` / `CLIP_OLD_MODEL_IDS`。
    /// 漂了会怎样：宿主重打标签的目标 id 与服务端回报的 model_id 不一致 →
    /// 搜索结果全部落空，而这个 bug 不会报错，只会「搜不到」。
    #[test]
    fn clip_model_ids_match_python() {
        let text = config_py();
        assert!(
            text.contains("\"id\": \"chinese-clip-vit-b16\""),
            "python 侧唯一档的 id 应为 chinese-clip-vit-b16"
        );
        assert!(
            text.contains("CLIP_RETIRED_IDS = (\"chinese-clip-vit-b16-fp16\"")
                && text.contains("\"chinese-clip-vit-b16-fp32\")"),
            "python 侧退役标签表变了，Rust 的迁移清单要同步"
        );
        assert_eq!(crate::db::embedding::CLIP_MODEL_ID, "chinese-clip-vit-b16");
        assert_eq!(
            crate::db::embedding::CLIP_OLD_MODEL_IDS,
            &["chinese-clip-vit-b16-fp16", "chinese-clip-vit-b16-fp32"]
        );
        // 单一档位：旧的两档选择已下线
        assert!(!text.contains("\"dir\": \"chinese-clip\""), "fp16 档目录已下线");
        assert!(!text.contains("model_fp16.onnx"), "fp16 权重不应再被任何档位引用");
    }

    /// FEAT-136：OpenVINO 运行时的落地路径必须与 python 侧逐字一致。
    ///
    /// 三处必须同名，任一处写错都只会在用户机器上表现为「下载完了但用不了」：
    ///   1. 本文件的 DlFile.local
    ///   2. python/vcr/ov_backend.py 的 ZIP_NAME
    ///   3. python/scripts/build_ov_runtime_zip.py 产出的文件名
    #[test]
    fn openvino_runtime_spec_matches_python() {
        let f = specs()
            .into_iter()
            .find(|s| s.name == "openvino-runtime")
            .expect("下载表必须有 openvino-runtime（可选组件）");
        assert!(!f.required, "组件必须是可选的，不能阻塞主流程");
        assert_eq!(f.files.len(), 1);
        assert_eq!(f.files[0].local, "_runtime/openvino-npu-win-x64.zip");
        assert!(
            f.files[0]
                .abs_url
                .unwrap_or("")
                .ends_with("/releases/latest/download/openvino-npu-win-x64.zip"),
            "必须走 releases/latest/download 稳定别名"
        );
        let py_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("python")
            .join("vcr");
        let backend = std::fs::read_to_string(py_dir.join("ov_backend.py"))
            .expect("应能读取 python/vcr/ov_backend.py");
        assert!(
            backend.contains("ZIP_NAME = \"openvino-npu-win-x64.zip\""),
            "ov_backend.py 的 ZIP_NAME 必须与 DlFile.local 同名"
        );
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
                        // 绝对 URL 不套 HF 镜像模板：结果里每一项都必须「包含」原始地址
                        // （GitHub 直链会被展开成 `<加速前缀>+原地址`，直连是最后一项兜底）
                        assert!(!urls.is_empty(), "{} 没有可用下载地址", spec.name);
                        if is_github_download(u) {
                            assert!(
                                urls.len() >= 2,
                                "GitHub 直链应展开出加速镜像：{urls:?}"
                            );
                            assert_eq!(
                                urls.last().map(String::as_str),
                                Some(u),
                                "直连必须保留为最后兜底：{urls:?}"
                            );
                        } else {
                            assert_eq!(urls.len(), 1, "非 GitHub 绝对 URL 应单源：{u}");
                            assert_eq!(urls[0], u);
                        }
                        for url in &urls {
                            assert!(
                                url.ends_with(u),
                                "绝对 URL 的每个变体都应指向同一路径：{url}"
                            );
                        }
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

    /// 并发 start 的防重入守卫：首次登记成功，第二次必须被拒；任务结束清掉后可再开。
    /// 只测纯逻辑（不构造 AppHandle），避免把网络/进程依赖拖进单测。
    #[test]
    fn begin_inflight_rejects_concurrent_start() {
        let mut st = Store::default();
        assert!(begin_inflight(&mut st, "chinese-clip-fp32").is_ok());
        assert!(
            begin_inflight(&mut st, "chinese-clip-fp32").is_err(),
            "同一模型第二次 start 必须被拒（否则双开写坏分片）"
        );
        st.inflight.remove("chinese-clip-fp32");
        st.cancel.remove("chinese-clip-fp32");
        assert!(
            begin_inflight(&mut st, "chinese-clip-fp32").is_ok(),
            "上一轮结束清掉 inflight 后应允许重下"
        );
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

    /// GitHub 直链必须展开成「加速镜像在前、直连兜底在后」。
    ///
    /// 这条守的是创意工坊四个本地增强模型的下载速度：它们全是 GitHub Release 直链，
    /// 单源路径下双路竞速不生效，340MB 的 GFPGAN 实测要 90 分钟（0.06MB/s）。
    #[test]
    fn github_links_expand_to_mirrors_with_direct_fallback() {
        let direct = "https://github.com/facefusion/facefusion-assets/releases/download/models-3.0.0/gfpgan_1.4.onnx";
        let urls = expand_github_mirrors(direct);
        assert_eq!(
            urls.len(),
            GITHUB_MIRRORS.len() + 1,
            "应为「每个镜像 + 直连」：{urls:?}"
        );
        assert_eq!(urls.last().map(String::as_str), Some(direct), "直连必须留作兜底");
        for (i, m) in GITHUB_MIRRORS.iter().enumerate() {
            assert_eq!(urls[i], format!("{m}{direct}"), "镜像应当前缀拼接");
        }
        // 去重后仍能覆盖所有候选（同一 URL 不重复尝试）
        assert_eq!(dedup_urls(urls.clone()).len(), urls.len());
    }

    /// 非 GitHub 的绝对 URL（ModelScope / 本仓库 Release 资产）保持单源，不套加速前缀。
    #[test]
    fn non_github_links_stay_single_source() {
        for url in [
            "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/rec/ch_PP-OCRv4_rec_mobile.onnx",
            "https://example.com/models/foo.onnx",
        ] {
            let urls = expand_github_mirrors(url);
            assert_eq!(urls, vec![url.to_string()], "非 GitHub 源不应被改写：{url}");
        }
        assert!(!is_github_download("https://example.com/x.onnx"));
        assert!(is_github_download(
            "https://github.com/a/b/releases/download/t/x.onnx"
        ));
    }

    /// 重试轮转：第 n 轮从第 n 个候选源开始，避免永远先撞同一个最慢的源。
    #[test]
    fn rotate_shifts_候选源() {
        let urls: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(rotate(&urls, 0), urls);
        assert_eq!(rotate(&urls, 1), vec!["b", "c", "a"]);
        assert_eq!(rotate(&urls, 4), vec!["b", "c", "a"], "超出长度应取模");
        assert!(rotate(&[], 3).is_empty());
    }

    /// 去重：重复地址只保留一份，且不改变顺序。
    #[test]
    fn dedup_urls_keeps_order() {
        let dup: Vec<String> = ["x", "y", "x"].iter().map(|s| s.to_string()).collect();
        assert_eq!(dedup_urls(dup), vec!["x", "y"]);
        assert!(dedup_urls(Vec::new()).is_empty());
    }
}
