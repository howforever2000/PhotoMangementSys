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

use super::pipeline::{histogram, render_u8_to_u8, RenderPlan, Workspace};
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
    let decoded: Result<(source::SourceImage, AdjustmentRecipe, bool, u64), String> =
        tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
            let t0 = std::time::Instant::now();
            let img = source::load(&source_path, target).map_err(|e| e.to_string())?;
            let decode_ms = t0.elapsed().as_millis() as u64;

            // 已有 sidecar 就带出来。sidecar 损坏时不阻断打开：
            // 给空配方继续编辑，但**不覆盖原文件**，用户仍可另存。
            let has_recipe = store::exists(&source_path);
            let recipe = match store::load(&source_path) {
                Ok(Some(r)) => r,
                Ok(None) => AdjustmentRecipe::default(),
                Err(e) => {
                    crate::logger::log_warn(&format!(
                        "edit_open | sidecar 损坏，已按空配方打开：{e}"
                    ));
                    AdjustmentRecipe::default()
                }
            };
            Ok((img, recipe, has_recipe, decode_ms))
        })
        .await
        .map_err(|e| format!("打开图片任务失败: {e}"))?;

    let result = decoded.map(|(img, recipe, has_recipe, decode_ms)| {
        let (pw, ph) = (img.decoded_width(), img.decoded_height());
        let (ow, oh) = (img.original_width, img.original_height);
        let subsampled = img.subsampled;
        let session = EditSession::new(session_path.clone(), img, recipe.clone());
        let session_id = states.insert(session);
        OpenResult {
            session_id,
            width: ow,
            height: oh,
            preview_width: pw,
            preview_height: ph,
            subsampled,
            recipe,
            has_recipe,
            decode_ms,
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

/// 渲染一帧并返回 JPEG 字节 + 元信息。
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
) -> Result<(tauri::ipc::Response, RenderMeta), String> {
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
        return Ok((tauri::ipc::Response::new(jpeg), meta));
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
    Ok((tauri::ipc::Response::new(jpeg), meta))
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

    let (sw, sh) = session.preview_size();
    let mut ws = session
        .workspace
        .lock()
        .map_err(|_| "工作区锁被占用（另一帧正在渲染）".to_string())?;
    let (w, h, pixels) = render_u8_to_u8(
        session.source.pixels.as_raw(),
        sw,
        sh,
        &r,
        max_long,
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
        let (w, h, pixels) = {
            let mut ws = s
                .workspace
                .lock()
                .map_err(|_| "工作区锁被占用".to_string())?;
            render_u8_to_u8(s.source.pixels.as_raw(), sw, sh, &r, 0, &mut ws)
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
) -> Result<(tauri::ipc::Response, u32, u32), String> {
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
    Ok((tauri::ipc::Response::new(bytes), w, h))
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

    // 几何决定输出尺寸。先编译一次拿到尺寸，渲染管线内部会再编译一次
    // （因为锐化 σ 依赖输出短边）—— 两次编译各约 0.3ms，可接受。
    let (ow, oh) = RenderPlan::compile(recipe, sw, sh, sw, sh).output_size(max_long_edge);
    if ow == 0 || oh == 0 {
        return Err("导出尺寸计算失败".into());
    }

    let mut ws = Workspace::new();
    let (rw, rh, pixels) =
        render_u8_to_u8(full.pixels.as_raw(), sw, sh, recipe, max_long_edge, &mut ws);

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
    Ok(RenderPlan::compile(&recipe, sw, sh, sw, sh).output_size(max_long_edge.unwrap_or(0)))
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
        };
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["previewWidth"], 1600);
        assert_eq!(j["hasRecipe"], false);
        assert_eq!(j["decodeMs"], 123);
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
            "maxSessions": crate::session::MAX_SESSIONS,
        });
        assert_eq!(j["sessions"], 0);
        assert_eq!(j["maxSessions"], 8);
    }
}
