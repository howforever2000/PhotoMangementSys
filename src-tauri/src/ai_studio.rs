//! 创意工坊 · AI 创作命令层（FEAT-152）
//!
//! 只做三件事（CS1 命令层薄壳）：参数与 provider 校验 → 确保 Python 服务就绪 →
//! 转发 `/jobs` 请求 / 读取作业状态。真正的编排与模型调用在 python/ai_studio。
//!
//! 四个工具共用同一套底层原语，区别只在 Rust 这里声明的「必需 provider」：
//!   - journal       需要 vision（理解/配文）+ edit（风格化）
//!   - beauty        需要 edit
//!   - style_transfer 需要 edit（vision 仅用于参考图描述回退）
//!   - upscale       需要 upscale（未配则回落 edit）
//!   - smart_edit    需要 vision（规划/复核）+ edit（执行/微调）
//!
//! 注（FEAT-169）：结构保全（滤镜/美颜不得改变物体形状）不在这层实现 ——
//! 提示词硬段在 Python `presets.STRUCTURE_LOCK`，出图后校验在 `critic.structure_compare`。
//! 本层保持「只查策略、只转发」，不引入任何图像判断。

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value as JsonValue;

/// 工具清单（与 Python `op_catalog.json` 同一份文件，编译期内联）。
/// 白名单、provider 策略都不再手抄：新增工具只改那一份 JSON。
const OP_CATALOG_JSON: &str = include_str!("../../python/ai_studio/op_catalog.json");

/// 查工具的 provider 策略；清单解析失败按「未知工具」处理（.fail closed，不放行）。
fn tool_policy(tool: &str) -> Option<String> {
    let data: JsonValue = serde_json::from_str(OP_CATALOG_JSON).ok()?;
    let tools = data.get("tools")?.as_array()?;
    for t in tools {
        if t.get("id").and_then(|x| x.as_str()) == Some(tool) {
            return Some(
                t.get("providerPolicy")
                    .and_then(|x| x.as_str())
                    .unwrap_or("none")
                    .to_string(),
            );
        }
    }
    None
}

fn check_policy(policy: &str, vision: &Option<PlainProvider>, edit: &Option<PlainProvider>, upscale: &Option<PlainProvider>) -> Result<(), String> {
    match policy {
        "none" => Ok(()),
        "edit" => {
            if edit.is_none() { return Err("请先在「模型设置」配置『图像编辑』模型".to_string()); }
            Ok(())
        }
        "vision+edit" => {
            if vision.is_none() {
                return Err("该工具需要『图像理解』模型（规划与复核），请先在「模型设置」配置".to_string());
            }
            if edit.is_none() {
                return Err("该工具需要『图像编辑』模型（执行），请先在「模型设置」配置".to_string());
            }
            Ok(())
        }
        "upscale|edit" => {
            if upscale.is_none() && edit.is_none() {
                return Err("请先在「模型设置」配置『超分』或『图像编辑』模型".to_string());
            }
            Ok(())
        }
        other => Err(format!("工具清单里的 providerPolicy 非法: {other}")),
    }
}

use crate::ai_service;
use crate::ai_settings::{provider_for, AiProviderInput, PlainProvider};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiJobInput {
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub output_dir: String,
    #[serde(default)]
    pub options: serde_json::Value,
}

fn prov_json(p: &Option<PlainProvider>) -> serde_json::Value {
    match p {
        Some(x) => serde_json::json!({
            "baseUrl": x.base_url,
            "apiKey": x.api_key,
            "model": x.model,
        }),
        None => serde_json::Value::Null,
    }
}

fn http_client(secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(secs))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

/// 提交一个 AI 创作作业，返回 jobId（进度用 `ai_job_status` 轮询）。
///
/// 命令层薄壳：只做 AOP 出入口日志，业务在 `run_job_inner`。这样 END 日志在
/// 成功与所有提前返回的错误路径上都会打（FEAT-178）。
#[tauri::command]
pub async fn ai_run_job(
    req: AiJobInput,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<String, String> {
    let t = log_call!(
        "ai_run_job",
        &format!("tool={} n={} out={}", req.tool, req.paths.len(), req.output_dir)
    );
    let result = run_job_inner(req, app, state, session).await;
    match &result {
        Ok(job_id) => crate::logger::log_call_end_with(
            "ai_run_job",
            t,
            &format!("OK job={job_id}"),
        ),
        Err(e) => crate::logger::log_call_end_with("ai_run_job", t, &format!("ERR {e}")),
    }
    result
}

async fn run_job_inner(
    req: AiJobInput,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<String, String> {
    let uid = crate::require_user(&session)?;
    if req.paths.is_empty() {
        return Err("没有可处理的照片".to_string());
    }
    if req.output_dir.trim().is_empty() {
        return Err("请先选择输出目录".to_string());
    }

    // 取 provider（解密）——锁只在同步块内持有，等待前释放
    let (vision, edit, upscale) = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        (
            provider_for(&db, uid, "vision")?,
            provider_for(&db, uid, "edit")?,
            provider_for(&db, uid, "upscale")?,
        )
    };

    // 按工具清单校验：白名单 + provider 策略都来自 op_catalog.json（唯一真相源）。
    // 未知工具在这里被拒，不会再出现「手抄白名单漏一项、用户点工具时才炸」。
    let policy = tool_policy(&req.tool)
        .ok_or_else(|| format!("未知的 AI 工具: {}", req.tool))?;
    check_policy(&policy, &vision, &edit, &upscale)?;

    let options = if req.options.is_null() {
        serde_json::json!({})
    } else {
        req.options
    };
    let body = serde_json::json!({
        "tool": req.tool,
        "paths": req.paths,
        "prompt": req.prompt,
        "outputDir": req.output_dir,
        "options": options,
        "providers": {
            "vision": prov_json(&vision),
            "edit": prov_json(&edit),
            "upscale": prov_json(&upscale),
        }
    });

    let base = ai_service::ensure_ready(&app).await?;
    let client = http_client(120)?;
    let resp = client
        .post(format!("{base}/jobs"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("提交 AI 作业失败: {e}"))?;
    if !resp.status().is_success() {
        let code = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("AI 服务返回 HTTP {code}: {text}"));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 AI 作业响应失败: {e}"))?;
    let job_id = v
        .get("jobId")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "AI 服务未返回 jobId".to_string())?
        .to_string();
    crate::logger::log_info(&format!(
        "[ai-studio] 作业已提交 tool={} policy={} job={job_id} | 模型 vision={} edit={} upscale={}",
        req.tool,
        policy,
        model_tag(&vision),
        model_tag(&edit),
        model_tag(&upscale)
    ));
    Ok(job_id)
}

/// provider 的展示标签：取模型名，未配置标 `-`（不打印 apiKey / baseUrl）。
fn model_tag(p: &Option<PlainProvider>) -> String {
    p.as_ref().map(|x| x.model.clone()).unwrap_or_else(|| "-".into())
}

/// 查询作业状态（前端轮询，约 700ms 一次）
///
/// 高频路径：成功轮询不记日志（避免冲垮日志窗口，见 bug-notes 高频成功路径条），
/// 仅 HTTP 失败 / 解析失败 / 慢调用（≥1s）时留痕。
#[tauri::command]
pub async fn ai_job_status(
    job_id: String,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let t = log_call!("ai_job_status", &format!("job={job_id}"));
    let r = status_inner(&format!("/jobs/{job_id}"), &app).await;
    finish_quiet("ai_job_status", t, &r);
    r
}

/// 高频查询共用：仅错误 / 慢调用时落日志，成功快路径静默。
fn finish_quiet<T>(name: &str, t: std::time::Instant, r: &Result<T, String>) {
    match r {
        Err(e) => crate::logger::log_call_end_with(name, t, &format!("ERR {e}")),
        Ok(_) => {
            let ms = t.elapsed().as_millis();
            if ms >= 1000 {
                crate::logger::log_call_end_with(name, t, "OK | SLOW 高频查询");
            }
        }
    }
}

async fn status_inner(
    path: &str,
    app: &tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let base = ai_service::ensure_ready(app).await?;
    let client = http_client(15)?;
    let resp = client
        .get(format!("{base}{path}"))
        .send()
        .await
        .map_err(|e| format!("查询 AI 状态失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("查询 AI 状态失败: HTTP {}", resp.status()));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("解析 AI 状态失败: {e}"))
}

/// 取消作业
#[tauri::command]
pub async fn ai_cancel_job(job_id: String, app: tauri::AppHandle) -> Result<bool, String> {
    let t = log_call!("ai_cancel_job", &format!("job={job_id}"));
    let r = cancel_inner(&job_id, &app).await;
    match &r {
        Ok(ok) => crate::logger::log_call_end_with("ai_cancel_job", t, &format!("OK cancelled={ok}")),
        Err(e) => crate::logger::log_call_end_with("ai_cancel_job", t, &format!("ERR {e}")),
    }
    r
}

async fn cancel_inner(job_id: &str, app: &tauri::AppHandle) -> Result<bool, String> {
    let base = ai_service::ensure_ready(app).await?;
    let client = http_client(15)?;
    let resp = client
        .post(format!("{base}/jobs/{job_id}/cancel"))
        .send()
        .await
        .map_err(|e| format!("取消 AI 作业失败: {e}"))?;
    Ok(resp.status().is_success())
}

/// 测试连通性：provider 为空时用已保存配置；密钥为掩码时沿用已保存密钥。
#[tauri::command]
pub async fn ai_test_provider(
    kind: String,
    provider: Option<AiProviderInput>,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<serde_json::Value, String> {
    let t = log_call!("ai_test_provider", &format!("kind={kind}"));
    let r = test_provider_inner(kind, provider, app, state, session).await;
    match &r {
        Ok(v) => {
            let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
            crate::logger::log_call_end_with("ai_test_provider", t, &format!("OK ok={ok}"))
        }
        Err(e) => crate::logger::log_call_end_with("ai_test_provider", t, &format!("ERR {e}")),
    }
    r
}

async fn test_provider_inner(
    kind: String,
    provider: Option<AiProviderInput>,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<serde_json::Value, String> {
    let uid = crate::require_user(&session)?;
    let saved = {
        let db = state.0.lock().map_err(|e| e.to_string())?;
        provider_for(&db, uid, &kind)?
    };

    let chosen: PlainProvider = match provider {
        Some(p) => {
            let base = p.base_url.trim().to_string();
            let model = p.model.trim().to_string();
            let mut key = p.api_key.trim().to_string();
            if key.is_empty() || key.contains('…') || key.contains("****") {
                key = saved.as_ref().map(|s| s.api_key.clone()).unwrap_or_default();
            }
            PlainProvider {
                base_url: base,
                api_key: key,
                model,
            }
        }
        None => saved.ok_or_else(|| "该能力尚未配置，请先填写 baseUrl / model / apiKey".to_string())?,
    };

    if chosen.base_url.trim().is_empty() {
        return Err("baseUrl 不能为空".to_string());
    }
    crate::logger::log_info(&format!(
        "[ai-studio] 连通性测试 kind={kind} model={} base={}",
        chosen.model, chosen.base_url
    ));

    let base = ai_service::ensure_ready(&app).await?;
    let client = http_client(60)?;
    let body = serde_json::json!({
        "kind": kind,
        "provider": {
            "baseUrl": chosen.base_url,
            "apiKey": chosen.api_key,
            "model": chosen.model,
        }
    });
    let resp = client
        .post(format!("{base}/providers/test"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("连通性测试请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("连通性测试失败: HTTP {}", resp.status()));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("解析连通性测试结果失败: {e}"))
}

/// 本地增强资源状态：转调 Python `/local/models`，返回模型是否就绪 + 下载地址 + 内置 LUT 清单。
/// 供前端「本地模型」面板展示下载链接与未就绪提示（FEAT-163）。
#[tauri::command]
pub async fn local_models_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let t = log_call!("local_models_status", "");
    let r = status_inner("/local/models", &app).await;
    finish_quiet("local_models_status", t, &r);
    r
}

/// 单张同步预览：转调 Python `/preview`，一次请求拿回本机算子效果图（data URL）。
///
/// 与 `ai_run_job` 的区别：不建作业、不写输出目录、不轮询进度。
/// 这里刻意**不取 provider、不做 provider 策略校验**——预览链路只跑本机算子，
/// 云端步会在 Python 侧被摘掉并进 `warnings`，所以本命令永远不会产生计费调用。
/// 返回结构与 Python `/preview` 一致：`{image,width,height,ms,dropped,warnings}`。
#[tauri::command]
pub async fn ai_preview(
    path: String,
    recipe_id: Option<String>,
    params_override: Option<serde_json::Value>,
    plan: Option<serde_json::Value>,
    max_edge: Option<u32>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let t = log_call!(
        "ai_preview",
        &format!(
            "path={path} recipe={recipe_id:?} plan={} maxEdge={max_edge:?}",
            plan.is_some()
        )
    );
    let r = preview_inner(path, recipe_id, params_override, plan, max_edge, app).await;
    match &r {
        Ok(v) => {
            let w = v.get("width").and_then(|x| x.as_u64()).unwrap_or(0);
            let h = v.get("height").and_then(|x| x.as_u64()).unwrap_or(0);
            crate::logger::log_call_end_with("ai_preview", t, &format!("OK {w}x{h}"));
        }
        Err(e) => crate::logger::log_call_end_with("ai_preview", t, &format!("ERR {e}")),
    }
    r
}

async fn preview_inner(
    path: String,
    recipe_id: Option<String>,
    params_override: Option<serde_json::Value>,
    plan: Option<serde_json::Value>,
    max_edge: Option<u32>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    if path.trim().is_empty() {
        return Err("没有可预览的照片".to_string());
    }
    let t0 = std::time::Instant::now();
    // 字段名按 HTTP 契约 camelCase 发；None 序列化为 null，Python 侧是 Optional，等价于「没传」。
    let body = serde_json::json!({
        "path": path,
        "recipeId": recipe_id,
        "paramsOverride": params_override,
        "plan": plan,
        "maxEdge": max_edge,
    });

    let base = ai_service::ensure_ready(&app).await?;
    // 60s：预览是同步交互，上限比作业（120s）短；滑杆连发时前端自己丢弃过期请求。
    let client = http_client(60)?;
    let resp = client
        .post(format!("{base}/preview"))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            crate::logger::log_info(&format!("[ai-studio] 预览请求失败 path={path} err={e}"));
            format!("预览失败: {e}")
        })?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        crate::logger::log_info(&format!(
            "[ai-studio] 预览失败 path={path} HTTP {status} {text}"
        ));
        return Err(format!("预览失败: HTTP {status}: {text}"));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| {
        crate::logger::log_info(&format!(
            "[ai-studio] 预览响应解析失败 path={path} err={e}"
        ));
        format!("解析预览响应失败: {e}")
    })?;
    let w = v.get("width").and_then(|x| x.as_u64()).unwrap_or(0);
    let h = v.get("height").and_then(|x| x.as_u64()).unwrap_or(0);
    let bytes = v
        .get("image")
        .and_then(|x| x.as_str())
        .map(|s| s.len())
        .unwrap_or(0);
    crate::logger::log_info(&format!(
        "[ai-studio] 预览完成 path={path} {w}x{h} ms={} dataUrl={bytes}B",
        t0.elapsed().as_millis()
    ));
    Ok(v)
}
