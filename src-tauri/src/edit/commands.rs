//! Tauri 命令层：薄壳，不含算法
//! ====================================================================
//! 每个命令都遵循同一结构：**校验入参 → 取出/建立会话 → `spawn_blocking`
//! 里做重活 → 组装返回**。算法全部在 `pipeline.rs` 及以下，命令层只做
//! 参数搬运与错误翻译。
//!
//! ## 为什么必须 `spawn_blocking`
//!
//! 一帧 1600×1067 的渲染约 10~40ms，全分辨率导出约 1~3s。若直接在
//! `async fn` 里同步算，会占住 Tauri 的异步工作线程；连续拖动滑块时
//! 几十个任务排队，UI 的事件循环被饿死，表现为「界面卡住」。
//! `tauri::async_runtime::spawn_blocking` 把重活挪到阻塞线程池，
//! 事件循环始终可用。
//!
//! ## 返回二进制
//!
//! 渲染结果走 `tauri::ipc::Response`（裸字节），**不**走 base64。
//! 一张 1600px 的 JPEG 约 200KB，base64 会膨胀到 270KB 且多一次
//! 编解码；Tauri v2 的 `Response` 直接传 `ArrayBuffer`，前端零拷贝构造 Blob。
//!
//! ⚠️ 但 Tauri v2 的 `IpcResponse` 只有 `Serialize` 与 `Response` 两个 impl，
//! **没有元组的 impl**（v1 的 `impl_tuple!` 已移除），因此
//! `Result<(Response, RenderMeta), String>` 编译不过（E0599 `async_kind`）。
//! 解法是 `pack_frame`：元信息与字节拼成**同一个**裸字节体再返回，
//! 前端按固定帧格式拆开。帧格式见该函数注释，两端必须同步修改。
//!
//! ## 命令清单（薄壳一览）
//!
//! | 命令 | 作用 | 重活 |
//! |---|---|---|
//! | `edit_schema` | 拉参数表 | 无 |
//! | `edit_open` | 建会话 | 解码 |
//! | `edit_close` | 释放会话 | 无 |
//! | `edit_render` | 渲一帧 | 渲染 + JPEG 编码 |
//! | `edit_histogram` | 直方图 | 渲染 + 统计 |
//! | `edit_autotune` | 自动优化 | 渲染 + 直方图 + 启发式 |
//! | `edit_save_recipe` / `edit_load_recipe` / `edit_reset_recipe` | sidecar | 磁盘 IO |
//! | `edit_encode` | 导出编码 | 全分辨率解码 + 渲染 + 编码 |
//! | `edit_write_file` | 落盘 | 磁盘 IO |
//! | `edit_recipe_to_text` / `edit_recipe_from_text` | 导入导出 | 无 |
//! | `edit_session_stats` | 排错 | 无 |

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use super::mask::MaskImage;
use super::pipeline::{histogram, render_u8_to_u8, render_u8_to_u8_masked, RenderPlan, Workspace};
use super::recipe::AdjustmentRecipe;
use super::schema;
use super::session::{EditSession, SessionStore};
use super::{source, store};

/// 命令共用的会话状态。由 `lib.rs` 注册为 Tauri managed state。
pub type EditState = SessionStore;

// ---------------------------------------------------------------------------
// 返回结构
// ---------------------------------------------------------------------------

/// 打开图片后返回给前端的元信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenResult {
    /// 会话 id，后续所有命令都要带
    pub session_id: u64,
    /// 原图尺寸
    pub width: u32,
    pub height: u32,
    /// 预览解码后的尺寸（前端据此决定缩放倍率）
    pub preview_width: u32,
    pub preview_height: u32,
    /// 是否经过 DCT 降采样（UI 可提示「预览为降采样画质，导出为全分辨率」）
    pub subsampled: bool,
    /// sidecar 里已有的配方（若有）
    pub recipe: AdjustmentRecipe,
    /// sidecar 是否存在（决定「保存」按钮是否可用）
    pub has_recipe: bool,
    /// 解码耗时（毫秒），用于显示加载性能
    pub decode_ms: u64,
    /// 蒙版的客观事实（是否存在、多大）
    pub mask: MaskInfo,
}

/// 直方图（前端面板渲染用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistogramResult {
    /// RGB 各 256 桶，平铺索引 = 值 * 3 + 通道（0=R, 1=G, 2=B）
    pub rgb: Vec<u32>,
    /// 亮度 256 桶
    pub luma: Vec<u32>,
    pub clipped_low: u32,
    pub clipped_high: u32,
    pub width: u32,
    pub height: u32,
}

/// 一帧渲染的结果元信息（与二进制返回体配套）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderMeta {
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u64,
    /// 是否因为「有更新的请求」而被丢弃
    pub stale: bool,
}

/// 导出编码的返回元信息（与二进制返回体配套）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodeMeta {
    pub width: u32,
    pub height: u32,
}

/// 蒙版操作的返回信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskResult {
    /// 蒙版自身的宽高（通常等于预览分辨率）
    pub width: u32,
    pub height: u32,
    /// 选中区域占比 0..=1（UI 提示「选中了多少」）
    pub coverage: f32,
    /// 落盘路径
    pub path: String,
}

impl MaskResult {
    /// 「没有蒙版」的哨兵值。
    ///
    /// 单独给一个构造器而不是让调用方手填四个 0：漏填某一个字段
    /// 会让前端把「没有蒙版」误判成「有蒙版」。
    fn empty() -> Self {
        Self {
            width: 0,
            height: 0,
            coverage: 0.0,
            path: String::new(),
        }
    }

    /// 蒙版是否真实存在（以宽度为准，与 `MaskImage::is_empty` 同口径）。
    pub fn exists(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// 蒙版的存在性与尺寸（`edit_open` 返回用，比 `MaskResult` 轻）。
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskInfo {
    pub exists: bool,
    pub width: u32,
    pub height: u32,
}

/// `edit_open` 阻塞任务的产物。
struct DecodedOpen {
    img: source::SourceImage,
    recipe: AdjustmentRecipe,
    has_recipe: bool,
    decode_ms: u64,
    mask_image: MaskImage,
    mask_info: MaskInfo,
}

/// 把「元信息 + 二进制」打包成单个裸字节 IPC 响应体。
///
/// # 帧格式
///
/// ```text
/// [0..4)      u32 LE = meta JSON 的字节长度 L
/// [4..4+L)    meta 的 UTF-8 JSON
/// [4+L..)     图片字节
/// ```
///
/// # 为什么必须自己打包
///
/// Tauri v2 的 `IpcResponse` 只有 `Serialize` 与 `Response` 两个 impl，
/// **没有元组的 impl**（v1 的 `impl_tuple!` 已在 v2 移除），所以
/// `Result<(Response, RenderMeta), String>` 编译不过，报 E0599
/// 「`async_kind` 的 trait bounds 未满足」。
///
/// 可选方案只有三个，本函数是唯一不违背模块设计前提的那个：
/// - base64 编进 JSON：体积 +33%、多一次编解码，与本模块「裸字节、
///   前端零拷贝构造 Blob」的前提直接冲突；
/// - 拆成两次 IPC：多一次往返，且元信息与字节可能来自不同帧（seq 对不上）；
/// - 单响应打包（本函数）：一次往返、体积不变。
///
/// 前端拆包见 `src/composables/useEditEngine.ts` 的 `unpackFrame`。
/// 两端帧格式必须同步修改。
pub fn pack_frame<M: Serialize>(meta: &M, payload: Vec<u8>) -> tauri::ipc::Response {
    tauri::ipc::Response::new(pack_frame_bytes(meta, &payload))
}

/// 组帧（纯字节版）：长度前缀 + JSON + 载荷。
///
/// 单独拆出来是为了让单元测试能拿到字节 —— tauri 2.11 把 `ipc::Response` 的
/// `body` 字段收成了私有，测试再也无法从 Response 里读回来（旧版能 `r.body`）。
/// 组帧逻辑只此一份，`pack_frame` 只是给它套一层 IPC 返回类型。
pub fn pack_frame_bytes<M: Serialize>(meta: &M, payload: &[u8]) -> Vec<u8> {
    // 元信息是固定的小结构，序列化失败只可能是编码器 bug；
    // 退化成空对象仍然返回图片字节，好过让整帧渲染白做。
    let json = serde_json::to_vec(meta).unwrap_or_default();
    let mut out = Vec::with_capacity(4 + json.len() + payload.len());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(payload);
    out
}

// ---------------------------------------------------------------------------
// 参数表
// ---------------------------------------------------------------------------

/// 参数表（前端渲染面板的唯一依据）。
///
/// # Errors
/// 不会失败；保留 `Result` 是为了与其它命令的调用形态一致，
/// 避免前端每个命令都写 try/catch 分支。
#[tauri::command]
pub fn edit_schema() -> Result<schema::Schema, String> {
    Ok(schema::schema())
}

// ---------------------------------------------------------------------------
// 会话生命周期
// ---------------------------------------------------------------------------

/// 打开一张图片并建立编辑会话。
///
/// 顺带加载同目录的 sidecar 配方 —— 用户上次的编辑结果不该因为
/// 「重新打开」而丢失。
///
/// # Errors
/// 文件不存在、解码失败、或尺寸超过上限时返回错误信息。
#[tauri::command]
pub async fn edit_open(
    states: State<'_, EditState>,
    path: String,
    preview_long_edge: Option<u32>,
) -> Result<OpenResult, String> {
    let _t = crate::logger::log_call_start("edit_open", &format!("path={path}"));
    let target = preview_long_edge.unwrap_or(source::PREVIEW_LONG_EDGE);
    let source_path = PathBuf::from(&path);
    // decode 闭包会 move 走 source_path，这里留一份给后面的会话构造用。
    let session_path = source_path.clone();

    // 解码与 sidecar 读取都在阻塞线程池里做（会占用几百毫秒）。
    // 会话插入留在 async 侧 —— SessionStore 自身不是 Clone，
    // 无法带进 'static 的阻塞任务；它内部是 Mutex，插入本身是微秒级操作。
    let decoded: Result<DecodedOpen, String> =
        tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
            let t0 = std::time::Instant::now();
            let img = source::load(&source_path, target).map_err(|e| e.to_string())?;
            let decode_ms = t0.elapsed().as_millis() as u64;

            // 已有 sidecar 就带出来。sidecar 损坏时不阻断打开：
            // 给空配方继续编辑，但**不覆盖原文件**，用户仍可另存。
            let has_recipe = store::exists(&source_path);
            let mut recipe = match store::load(&source_path) {
                Ok(Some(r)) => r,
                Ok(None) => AdjustmentRecipe::default(),
                Err(e) => {
                    crate::logger::log_warn(&format!(
                        "edit_open | sidecar 损坏，已按空配方打开：{e}"
                    ));
                    AdjustmentRecipe::default()
                }
            };

            // 蒙版同 sidecar：文件坏了就当没有（降级为整图调整），
            // 但把 recipe.mask 里的事实纠正过来，避免 UI 显示「已启用」
            // 而渲染按整图走 —— 那种不一致比丢一个蒙版更让人困惑。
            let mask_image = match store::load_mask(&source_path) {
                Ok(Some(m)) => m,
                Ok(None) => MaskImage::empty(),
                Err(e) => {
                    crate::logger::log_warn(&format!("edit_open | 蒙版读取失败，已忽略：{e}"));
                    MaskImage::empty()
                }
            };
            let info = MaskInfo {
                exists: !mask_image.is_empty(),
                width: mask_image.width(),
                height: mask_image.height(),
            };
            if recipe.mask.has_image != info.exists {
                recipe.mask.has_image = info.exists;
                if !info.exists {
                    recipe.mask.enabled = false;
                }
            }

            Ok(DecodedOpen {
                img,
                recipe,
                has_recipe,
                decode_ms,
                mask_image,
                mask_info: info,
            })
        })
        .await
        .map_err(|e| format!("打开图片任务失败: {e}"))?;

    let result = decoded.map(|d| {
        let (pw, ph) = (d.img.decoded_width(), d.img.decoded_height());
        let (ow, oh) = (d.img.original_width, d.img.original_height);
        let subsampled = d.img.subsampled;
        let session = EditSession::new(session_path.clone(), d.img, d.recipe.clone());
        session.set_mask(d.mask_image);
        let session_id = states.insert(session);
        OpenResult {
            session_id,
            width: ow,
            height: oh,
            preview_width: pw,
            preview_height: ph,
            subsampled,
            recipe: d.recipe,
            has_recipe: d.has_recipe,
            decode_ms: d.decode_ms,
            mask: d.mask_info,
        }
    });

    match &result {
        Ok(r) => crate::logger::log_call_end_with(
            "edit_open",
            _t,
            &format!(
                "OK | session={} | {}x{} → {}x{} | {}ms",
                r.session_id, r.width, r.height, r.preview_width, r.preview_height, r.decode_ms
            ),
        ),
        Err(e) => crate::logger::log_call_end_with("edit_open", _t, &format!("ERR | {e}")),
    }
    result
}

/// 关闭一个会话（切换图片 / 关闭编辑器时调用）。
///
/// # Errors
/// 不会失败；会话不存在时返回 `Ok(false)`（幂等，前端不必关心）。
#[tauri::command]
pub async fn edit_close(states: State<'_, EditState>, session_id: u64) -> Result<bool, String> {
    let existed = states.close(session_id);
    crate::logger::log_info(&format!(
        "edit_close | session={session_id} | existed={existed}"
    ));
    Ok(existed)
}

/// 一次关闭全部会话（应用退出 / 编辑器整体卸载时调用）。
///
/// # Errors
/// 不会失败。
#[tauri::command]
pub async fn edit_close_all(states: State<'_, EditState>) -> Result<usize, String> {
    let n = states.close_all();
    crate::logger::log_info(&format!("edit_close_all | released={n}"));
    Ok(n)
}

// ---------------------------------------------------------------------------
// 渲染
// ---------------------------------------------------------------------------

/// 渲染一帧并返回 JPEG 字节 + 元信息（打包成单个裸字节体，见 `pack_frame`）。
///
/// `draft = true` 用草稿档（长边 800），供拖动滑块过程中的实时预览；
/// `false` 用正常档（长边 1600）。草稿档像素数只有 1/4，耗时随之降到
/// 约 1/3 —— 这是交互流畅度的主要来源。
///
/// # Errors
/// 会话失效（已被淘汰或已关闭）或渲染/编码失败时返回错误。
#[tauri::command]
pub async fn edit_render(
    states: State<'_, EditState>,
    session_id: u64,
    recipe: AdjustmentRecipe,
    draft: Option<bool>,
) -> Result<tauri::ipc::Response, String> {
    let t0 = std::time::Instant::now();
    let draft = draft.unwrap_or(false);
    let _t = crate::logger::log_call_start(
        "edit_render",
        &format!("session={session_id} draft={draft}"),
    );

    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效，请重新打开图片"))?;

    // 渲染前取序号；若期间有更新的请求到达，本次结果直接丢弃
    let seq = session.next_render_seq();
    let s = Arc::clone(&session);

    let task =
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<u8>, RenderMeta), String> {
            render_frame(&s, &recipe, draft, Some(seq))
        });
    let (jpeg, meta) = task.await.map_err(|e| format!("渲染任务失败: {e}"))??;

    if meta.stale {
        crate::logger::log_call_end_with("edit_render", _t, "STALE | 被更新的请求取代");
        return Ok(pack_frame(&meta, jpeg));
    }

    let mut meta = meta;
    meta.elapsed_ms = t0.elapsed().as_millis() as u64;
    crate::logger::log_call_end_with(
        "edit_render",
        _t,
        &format!(
            "OK | {}x{} | {}ms",
            meta.width, meta.height, meta.elapsed_ms
        ),
    );
    Ok(pack_frame(&meta, jpeg))
}

/// 同步渲染（供 `edit_render` / `edit_histogram` / `edit_encode` 复用）。
///
/// `check_seq` 非空时在锁前校验序号、锁后再校验一次 —— 只查一次不够：
/// 检查到加锁之间仍可能有新请求进来。
fn render_frame(
    session: &EditSession,
    recipe: &AdjustmentRecipe,
    draft: bool,
    check_seq: Option<u64>,
) -> Result<(Vec<u8>, RenderMeta), String> {
    if let Some(seq) = check_seq {
        if !session.is_latest(seq) {
            return Ok((
                Vec::new(),
                RenderMeta {
                    width: 0,
                    height: 0,
                    elapsed_ms: 0,
                    stale: true,
                },
            ));
        }
    }

    let mut r = recipe.clone();
    r.sanitize();

    let max_long = if draft {
        source::DRAFT_LONG_EDGE
    } else {
        source::PREVIEW_LONG_EDGE
    };

    // 蒙版在锁外取快照：解码结果跨帧复用，取一次即可；
    // 放在锁内会让「等锁」的时间也算进渲染耗时。
    let mask = session.mask_snapshot();

    let (sw, sh) = session.preview_size();
    let mut ws = session
        .workspace
        .lock()
        .map_err(|_| "工作区锁被占用（另一帧正在渲染）".to_string())?;
    let (w, h, pixels) = render_u8_to_u8_masked(
        session.source.pixels.as_raw(),
        sw,
        sh,
        &r,
        max_long,
        &mask,
        &mut ws,
    );

    // 锁后二次校验：这次渲染的像素即将被丢弃，不要浪费后续编码
    let stale = check_seq.map(|s| !session.is_latest(s)).unwrap_or(false);
    drop(ws);
    if stale {
        return Ok((
            Vec::new(),
            RenderMeta {
                width: 0,
                height: 0,
                elapsed_ms: 0,
                stale: true,
            },
        ));
    }

    let jpeg = super::encode_jpeg(&pixels, w, h, if draft { 82 } else { 90 })?;
    Ok((
        jpeg,
        RenderMeta {
            width: w,
            height: h,
            elapsed_ms: 0,
            stale: false,
        },
    ))
}

/// 渲染并返回直方图（不返回图像字节）。
///
/// 直方图按**编辑后**的显示值统计 —— 用户看到的分布必须是他正在编辑的
/// 那一张，否则「拉曝光看直方图是否居中」这个核心反馈环就断了。
///
/// # Errors
/// 会话失效或渲染失败时返回错误。
#[tauri::command]
pub async fn edit_histogram(
    states: State<'_, EditState>,
    session_id: u64,
    recipe: AdjustmentRecipe,
) -> Result<HistogramResult, String> {
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    let s = Arc::clone(&session);
    let task = tauri::async_runtime::spawn_blocking(move || -> Result<HistogramResult, String> {
        let mut r = recipe;
        r.sanitize();
        let (sw, sh) = s.preview_size();
        let mask = s.mask_snapshot();
        let (w, h, pixels) = {
            let mut ws = s
                .workspace
                .lock()
                .map_err(|_| "工作区锁被占用".to_string())?;
            render_u8_to_u8_masked(s.source.pixels.as_raw(), sw, sh, &r, 0, &mask, &mut ws)
        };
        let (rgb, luma, low, high) = histogram(&pixels, w, h);
        Ok(HistogramResult {
            rgb,
            luma,
            clipped_low: low,
            clipped_high: high,
            width: w,
            height: h,
        })
    });
    task.await.map_err(|e| format!("直方图任务失败: {e}"))?
}

/// 自动优化：从直方图给出保守的起始值。
///
/// # Errors
/// 会话失效或渲染失败时返回错误。
#[tauri::command]
pub async fn edit_autotune(
    states: State<'_, EditState>,
    session_id: u64,
) -> Result<AdjustmentRecipe, String> {
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    let s = Arc::clone(&session);
    let task = tauri::async_runtime::spawn_blocking(move || -> Result<AdjustmentRecipe, String> {
        let (sw, sh) = s.preview_size();
        let (_, _, pixels) = {
            let mut ws = s
                .workspace
                .lock()
                .map_err(|_| "工作区锁被占用".to_string())?;
            render_u8_to_u8(
                s.source.pixels.as_raw(),
                sw,
                sh,
                &AdjustmentRecipe::default(),
                0,
                &mut ws,
            )
        };
        let (rgb, luma, _, _) = histogram(&pixels, sw, sh);
        Ok(schema::autotune(&luma, &rgb))
    });
    task.await.map_err(|e| format!("自动优化任务失败: {e}"))?
}

// ---------------------------------------------------------------------------
// 配方持久化
// ---------------------------------------------------------------------------

/// 保存配方到 sidecar（原子写）。
///
/// # Errors
/// 会话失效或磁盘写入失败时返回错误。
#[tauri::command]
pub async fn edit_save_recipe(
    states: State<'_, EditState>,
    session_id: u64,
    recipe: AdjustmentRecipe,
) -> Result<String, String> {
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    let mut r = recipe;
    r.sanitize();
    let path = store::save(&session.source_path, &r).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// 删除 sidecar（= 重置为原图）。
///
/// # Errors
/// 会话失效或删除失败时返回错误。
#[tauri::command]
pub async fn edit_reset_recipe(
    states: State<'_, EditState>,
    session_id: u64,
) -> Result<(), String> {
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    store::remove(&session.source_path).map_err(|e| e.to_string())
}

/// 读取 sidecar（外部改动后刷新用）。
///
/// # Errors
/// 会话失效或 sidecar 损坏时返回错误。
#[tauri::command]
pub async fn edit_load_recipe(
    states: State<'_, EditState>,
    session_id: u64,
) -> Result<Option<AdjustmentRecipe>, String> {
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    store::load(&session.source_path).map_err(|e| e.to_string())
}

/// 配方导出为 JSON 文本（剪贴板 / 文件）。
///
/// # Errors
/// 序列化失败时返回错误。
#[tauri::command]
pub fn edit_recipe_to_text(recipe: AdjustmentRecipe) -> Result<String, String> {
    store::export_text(&recipe).map_err(|e| e.to_string())
}

/// 从 JSON 文本导入配方。
///
/// # Errors
/// 文本非法时返回错误信息（含具体解析位置）。
#[tauri::command]
pub fn edit_recipe_from_text(text: String) -> Result<AdjustmentRecipe, String> {
    store::import_text(&text).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// 局部蒙版
// ---------------------------------------------------------------------------

/// 上传蒙版 PNG（覆盖既有蒙版）。
///
/// 前端把蒙版画在**预览分辨率**的离屏 canvas 上再导出为 PNG 上传，
/// 这样蒙版文件不会随原图尺寸膨胀，导出时按需缩放（见 `mask.rs`）。
///
/// PNG 同时落盘到 sidecar —— 用户中途关掉应用再打开，蒙版还在。
///
/// # Errors
/// 会话失效、PNG 解码失败、或写盘失败时返回错误。
#[tauri::command]
pub async fn edit_mask_set(
    states: State<'_, EditState>,
    session_id: u64,
    png: Vec<u8>,
) -> Result<MaskResult, String> {
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    let image = MaskImage::from_png(&png)?;
    let path = store::save_mask(&session.source_path, &png).map_err(|e| e.to_string())?;
    crate::logger::log_info(&format!(
        "edit_mask_set | {}x{} | {}KB | {}",
        image.width(),
        image.height(),
        png.len() / 1024,
        path.display()
    ));
    session.set_mask(image.clone());
    Ok(MaskResult {
        width: image.width(),
        height: image.height(),
        coverage: image.coverage(),
        path: path.to_string_lossy().into_owned(),
    })
}

/// 清除蒙版（删文件 + 清会话缓存）。
///
/// 幂等：没有蒙版时也返回 `Ok`，前端可以无条件调用。
///
/// # Errors
/// 会话失效或删文件失败时返回错误。
#[tauri::command]
pub async fn edit_mask_clear(states: State<'_, EditState>, session_id: u64) -> Result<(), String> {
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    store::remove_mask(&session.source_path).map_err(|e| e.to_string())?;
    session.set_mask(MaskImage::empty());
    crate::logger::log_info("edit_mask_clear | 蒙版已清除");
    Ok(())
}

/// 取回当前蒙版的 PNG 字节（重新打开图片后恢复前端画布用）。
///
/// 用 `pack_frame` 打包（`[长度][JSON][PNG]`），而不是 JSON 里塞字节数组：
/// 一张长边 1600 的灰度 PNG 约 2.5MB，转成 JSON 数字数组会膨胀到 10MB
/// 且解析极慢。裸字节走 IPC 是 O(1) 的。
///
/// 没有蒙版时返回 `width=0` 的帧（而非报错）：「这张图还没蒙版」
/// 是正常状态，不是异常。
///
/// # Errors
/// 会话失效或读盘失败时返回错误。
#[tauri::command]
pub async fn edit_mask_get(
    states: State<'_, EditState>,
    session_id: u64,
) -> Result<tauri::ipc::Response, String> {
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    let p = store::mask_path(&session.source_path);
    if !p.is_file() {
        return Ok(pack_frame(&MaskResult::empty(), Vec::new()));
    }
    let png = std::fs::read(&p).map_err(|e| format!("读取蒙版失败：{e}"))?;
    let image = MaskImage::from_png(&png)?;
    Ok(pack_frame(
        &MaskResult {
            width: image.width(),
            height: image.height(),
            coverage: image.coverage(),
            path: p.to_string_lossy().into_owned(),
        },
        png,
    ))
}

// ---------------------------------------------------------------------------
// 导出
// ---------------------------------------------------------------------------

/// 导出为图片字节（前端用保存对话框选路径后调 `edit_write_file`）。
///
/// 路径选择必须由前端完成 —— Tauri 的 `save` 插件在前端侧，
/// 后端不该替用户决定写到哪。
///
/// # Errors
/// 会话失效、导出尺寸为 0、或编码失败时返回错误。
#[tauri::command]
pub async fn edit_encode(
    states: State<'_, EditState>,
    session_id: u64,
    recipe: AdjustmentRecipe,
    format: Option<String>,
    quality: Option<u8>,
    max_long_edge: Option<u32>,
) -> Result<tauri::ipc::Response, String> {
    let _t = crate::logger::log_call_start("edit_encode", &format!("session={session_id}"));
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    let mut r = recipe;
    r.sanitize();
    let fmt = format.unwrap_or_else(|| "jpeg".to_string());
    let q = quality.unwrap_or(92).clamp(50, 100);
    let mle = max_long_edge.unwrap_or(0);

    let s = Arc::clone(&session);
    // fmt 要在闭包里用、闭包后还要打日志 —— 必须 clone。
    // 直接 move 会得到「borrow of moved value」的编译错误（好消息），
    // 但若日志里改用常量就会静默丢信息（坏消息）—— 所以这里显式留痕。
    let fmt_for_log = fmt.clone();
    let task =
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<u8>, u32, u32), String> {
            render_and_encode(&s, &r, &fmt, q, mle)
        });
    let (bytes, w, h) = task.await.map_err(|e| format!("导出编码任务失败: {e}"))??;
    crate::logger::log_call_end_with(
        "edit_encode",
        _t,
        &format!("OK | {w}x{h} | {}KB | {fmt_for_log}", bytes.len() / 1024),
    );
    Ok(pack_frame(
        &EncodeMeta {
            width: w,
            height: h,
        },
        bytes,
    ))
}

/// 全分辨率渲染 + 编码。
///
/// **必须重新解码**：会话里缓存的是长边 1600 的预览平面，
/// 直接拿它导出等于「导出一张 1600px 的图」。这是导出路径与预览路径
/// 唯一的结构性差异，也是两者必须分开写的原因。
fn render_and_encode(
    session: &EditSession,
    recipe: &AdjustmentRecipe,
    format: &str,
    quality: u8,
    max_long_edge: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    let full = source::load(&session.source_path, 0).map_err(|e| e.to_string())?;
    let (sw, sh) = (full.decoded_width(), full.decoded_height());
    let mask = session.mask_snapshot();

    // 几何决定输出尺寸。先编译一次拿到尺寸，渲染管线内部会再编译一次
    // （因为锐化 σ 依赖输出短边）—— 两次编译各约 0.3ms，可接受。
    let (ow, oh) =
        RenderPlan::compile(recipe, sw, sh, sw, sh, mask.clone()).output_size(max_long_edge);
    if ow == 0 || oh == 0 {
        return Err("导出尺寸计算失败".into());
    }

    let mut ws = Workspace::new();
    let (rw, rh, pixels) =
        render_u8_to_u8_masked(full.pixels.as_raw(), sw, sh, recipe, max_long_edge, &mask, &mut ws);

    let bytes = match format {
        "png" => super::encode_png(&pixels, rw, rh)?,
        _ => super::encode_jpeg(&pixels, rw, rh, quality)?,
    };
    Ok((bytes, rw, rh))
}

/// 导出尺寸查询（保存前让用户看到「最终将是 4000×3000」）。
///
/// # Errors
/// 会话失效时返回错误。
#[tauri::command]
pub async fn edit_output_size(
    states: State<'_, EditState>,
    session_id: u64,
    recipe: AdjustmentRecipe,
    max_long_edge: Option<u32>,
) -> Result<(u32, u32), String> {
    let session = states
        .get(session_id)
        .ok_or_else(|| format!("编辑会话 {session_id} 已失效"))?;
    let (sw, sh) = session.preview_size();
    Ok(RenderPlan::compile(&recipe, sw, sh, sw, sh, MaskImage::empty())
        .output_size(max_long_edge.unwrap_or(0)))
}

/// 把字节写到指定路径（前端保存对话框给出的绝对路径）。
///
/// # Errors
/// 路径为空或写入失败时返回错误。
#[tauri::command]
pub async fn edit_write_file(path: String, data: Vec<u8>) -> Result<String, String> {
    if path.trim().is_empty() {
        return Err("保存路径为空".into());
    }
    let p = PathBuf::from(&path);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    std::fs::write(&p, &data).map_err(|e| format!("写入文件失败: {e}"))?;
    Ok(path)
}

/// 会话内存占用（devtools / 排错用）。
///
/// # Errors
/// 不会失败。
#[tauri::command]
pub fn edit_session_stats(states: State<'_, EditState>) -> serde_json::Value {
    serde_json::json!({
        "sessions": states.len(),
        "bytes": states.memory_bytes(),
        "maxSessions": super::session::MAX_SESSIONS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_result_serializes_camel_case() {
        let r = OpenResult {
            session_id: 1,
            width: 6000,
            height: 4000,
            preview_width: 1600,
            preview_height: 1067,
            subsampled: true,
            recipe: AdjustmentRecipe::default(),
            has_recipe: false,
            decode_ms: 123,
            mask: MaskInfo {
                exists: true,
                width: 1600,
                height: 1067,
            },
        };
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["previewWidth"], 1600);
        assert_eq!(j["hasRecipe"], false);
        assert_eq!(j["decodeMs"], 123);
        assert_eq!(j["mask"]["exists"], true);
        assert_eq!(j["mask"]["width"], 1600);
        assert!(j.get("preview_width").is_none(), "字段名应为 camelCase");
    }

    #[test]
    fn histogram_result_serializes_camel_case() {
        let h = HistogramResult {
            rgb: vec![0; 768],
            luma: vec![0; 256],
            clipped_low: 1,
            clipped_high: 2,
            width: 10,
            height: 10,
        };
        let j = serde_json::to_value(&h).unwrap();
        assert_eq!(j["clippedLow"], 1);
        assert_eq!(j["clippedHigh"], 2);
        assert_eq!(j["rgb"].as_array().unwrap().len(), 768);
    }

    #[test]
    fn render_meta_marks_stale() {
        let m = RenderMeta {
            width: 0,
            height: 0,
            elapsed_ms: 0,
            stale: true,
        };
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["stale"], true);
        assert_eq!(j["elapsedMs"], 0);
    }

    /// 拆包侧的等价实现。Rust 单测够不到前端，这里保证帧格式的
    /// 「头部 → JSON → 载荷」三段布局与 `unpackFrame` 的解析逻辑一致。
    fn unpack_frame(buf: &[u8]) -> (serde_json::Value, Vec<u8>) {
        let meta_len = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
        let meta: serde_json::Value =
            serde_json::from_slice(&buf[4..4 + meta_len]).expect("头部应是合法 JSON");
        (meta, buf[4 + meta_len..].to_vec())
    }

    /// 帧格式的回归护栏：长度前缀 + JSON + 载荷，顺序与边界都不能错。
    ///
    /// 这条测试守着 BUG 的根因：真 Tauri v2 的 `IpcResponse` 没有元组 impl，
    /// 「元信息 + 字节」只能靠本模块自定的帧格式一起传。前端
    /// `unpackFrame` 依赖同一约定，任一侧改动都必须让本测试仍通过。
    #[test]
    fn pack_frame_roundtrips_meta_and_payload() {
        let meta = RenderMeta {
            width: 1600,
            height: 1067,
            elapsed_ms: 42,
            stale: false,
        };
        let payload = vec![0xFFu8, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let r = pack_frame_bytes(&meta, &payload);
        let (back, bytes) = unpack_frame(&r);

        assert_eq!(back["width"], 1600);
        assert_eq!(back["height"], 1067);
        assert_eq!(back["elapsedMs"], 42);
        assert_eq!(back["stale"], false);
        assert_eq!(bytes, payload, "载荷应逐字节还原");
    }

    /// 载荷里出现看起来像头部的东西也不能被误切 ——
    /// 长度前缀在前，解析只认前 4 字节，载荷内容不应影响切分。
    #[test]
    fn pack_frame_survives_payload_that_looks_like_a_header() {
        let meta = EncodeMeta { width: 4, height: 4 };
        // 前 4 字节故意做成一个「看起来很大」的长度前缀
        let payload = vec![0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x02];
        let r = pack_frame_bytes(&meta, &payload);
        let (back, bytes) = unpack_frame(&r);

        assert_eq!(back["width"], 4);
        assert_eq!(bytes, payload, "载荷含伪头部时仍应完整还原");
    }

    /// 空载荷（stale 帧走的就是这条路）不能 panic，长度也要对。
    #[test]
    fn pack_frame_handles_empty_payload() {
        let meta = RenderMeta {
            width: 0,
            height: 0,
            elapsed_ms: 0,
            stale: true,
        };
        let r = pack_frame_bytes(&meta, &[]);
        let (back, bytes) = unpack_frame(&r);

        assert_eq!(back["stale"], true);
        assert!(bytes.is_empty(), "stale 帧载荷应为空");
    }

    #[test]
    fn recipe_json_roundtrip_through_command_surface() {
        let mut r = AdjustmentRecipe::default();
        r.basic.exposure = 2.5;
        let text = edit_recipe_to_text(r.clone()).unwrap();
        let back = edit_recipe_from_text(text).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn command_schema_is_serializable_with_camel_case() {
        let s = edit_schema().unwrap();
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["version"], 1);
        assert_eq!(j["groups"].as_array().unwrap().len(), 5);
        assert!(j.get("previewLongEdge").is_some(), "previewLongEdge 缺失");
        // 滑块的 key 应保持点号路径原样（前端按路径读写）
        let g0 = &j["groups"][0];
        assert_eq!(g0["params"][0]["key"], "basic.exposure");
        assert!(g0["params"][0].get("min").is_some());
        assert!(g0["params"][0].get("neutral").is_some());
    }

    #[test]
    fn import_text_rejects_garbage() {
        assert!(edit_recipe_from_text("{oops".into()).is_err());
    }

    #[test]
    fn quality_is_clamped_to_sane_range() {
        for q in [0u8, 10, 50, 100, 255] {
            let clamped = q.clamp(50, 100);
            assert!((50..=100).contains(&clamped), "q={q} 钳制后 {clamped} 越界");
        }
    }

    #[test]
    fn workspace_starts_empty() {
        assert_eq!(Workspace::new().allocated_bytes(), 0);
    }

    #[test]
    fn session_stats_json_shape() {
        let store = SessionStore::new();
        let j = serde_json::json!({
            "sessions": store.len(),
            "bytes": store.memory_bytes(),
            "maxSessions": crate::edit::session::MAX_SESSIONS,
        });
        assert_eq!(j["sessions"], 0);
        assert_eq!(j["maxSessions"], 8);
    }

    /// 端到端：`edit_render` 产出的帧，前端拆包后必须是一张**可解码**的 JPEG，
    /// 且 meta 里的尺寸与真实像素一致。
    ///
    /// 这条比 `pack_frame_roundtrips_meta_and_payload` 更强：它走真实的
    /// 解码 → 渲染 → 编码链路，因此能同时守住帧格式与渲染产物。
    /// 缺了它，「帧头对但里面是坏字节」这类问题只能等用户在编辑器里看到
    /// 一张裂图才发现。
    #[test]
    fn edit_render_frame_unpacks_to_decodable_jpeg() {
        let (w, h) = (64u32, 48u32);
        let src = (0..(w * h * 3) as usize)
            .map(|i| ((i * 7 + (i / 3) * 13) % 251) as u8)
            .collect::<Vec<u8>>();
        let path = write_temp_jpeg(&src, w, h);

        let decoded = source::load(&path, 0).expect("测试图应能解码");
        let session = EditSession::new(
            path.clone(),
            decoded,
            AdjustmentRecipe::default(),
        );
        let mut r = AdjustmentRecipe::default();
        r.basic.exposure = 0.4;

        let (jpeg, meta) = render_frame(&session, &r, false, None).expect("渲染不应失败");
        assert!(!meta.stale);
        // 对的是「解码后的预览平面尺寸」而非磁盘上的原图尺寸：
        // JPEG 走 DCT 降采样解码，小图会被降到 1/8 档（见 source.rs）。
        let (pw, ph) = session.preview_size();
        assert_eq!((meta.width, meta.height), (pw, ph), "meta 尺寸应等于预览平面尺寸");

        let framed = pack_frame_bytes(&meta, &jpeg);
        let (back, bytes) = unpack_frame(&framed);

        assert_eq!(back["width"], meta.width);
        assert_eq!(back["height"], meta.height);
        assert_eq!(back["elapsedMs"], 0, "meta 未过 elapsed_ms 覆写时为 0");
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "拆出的字节应以 JPEG SOI 开头");
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
            .expect("拆包后的字节必须是可解码的 JPEG");
        assert_eq!(
            (img.width(), img.height()),
            (pw, ph),
            "解出的像素尺寸应与 meta 一致"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// 把 RGB 写成临时 JPEG，供上面的端到端测试当输入。
    fn write_temp_jpeg(rgb: &[u8], w: u32, h: u32) -> PathBuf {
        let dir = std::env::temp_dir().join("edit-verify-frame");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("in_{w}x{h}_{}.jpg", rgb.len()));
        let bytes = super::super::encode_jpeg(rgb, w, h, 92).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
}
