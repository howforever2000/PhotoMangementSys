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

use std::time::Duration;

use serde::Deserialize;

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
#[tauri::command]
pub async fn ai_run_job(
    req: AiJobInput,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<String, String> {
    let _t = log_call!(
        "ai_run_job",
        &format!("tool={} n={} out={}", req.tool, req.paths.len(), req.output_dir)
    );
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

    // 按工具校验必需能力
    match req.tool.as_str() {
        "journal" => {
            if vision.is_none() {
                return Err("请先在「模型设置」配置『图像理解』模型".to_string());
            }
            if edit.is_none() {
                return Err("请先在「模型设置」配置『图像编辑』模型".to_string());
            }
        }
        "beauty" | "style_transfer" => {
            if edit.is_none() {
                return Err("请先在「模型设置」配置『图像编辑』模型".to_string());
            }
        }
        "upscale" => {
            if upscale.is_none() && edit.is_none() {
                return Err("请先在「模型设置」配置『超分』或『图像编辑』模型".to_string());
            }
        }
        "smart_edit" => {
            // 智能修图 = 多模态模型「规划/复核」 + 图像编辑模型「执行/微调」，两者都必须有
            if vision.is_none() {
                return Err(
                    "智能修图需要『图像理解』模型（规划与复核），请先在「模型设置」配置".to_string(),
                );
            }
            if edit.is_none() {
                return Err(
                    "智能修图需要『图像编辑』模型（执行），请先在「模型设置」配置".to_string(),
                );
            }
        }
        // 本地增强（FEAT-163）：纯本机推理，无需任何外部 provider
        "film_sim" | "color_transfer" | "restore" => {}
        other => return Err(format!("未知的 AI 工具: {other}")),
    }

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
    crate::logger::log_info(&format!("[ai-studio] 作业已提交 tool={} job={job_id}", req.tool));
    Ok(job_id)
}

/// 查询作业状态（前端轮询，约 700ms 一次）
#[tauri::command]
pub async fn ai_job_status(
    job_id: String,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let base = ai_service::ensure_ready(&app).await?;
    let client = http_client(15)?;
    let resp = client
        .get(format!("{base}/jobs/{job_id}"))
        .send()
        .await
        .map_err(|e| format!("查询 AI 作业失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("查询 AI 作业失败: HTTP {}", resp.status()));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("解析 AI 作业状态失败: {e}"))
}

/// 取消作业
#[tauri::command]
pub async fn ai_cancel_job(job_id: String, app: tauri::AppHandle) -> Result<bool, String> {
    let _t = log_call!("ai_cancel_job", &format!("job={job_id}"));
    let base = ai_service::ensure_ready(&app).await?;
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
    let _t = log_call!("ai_test_provider", &format!("kind={kind}"));
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
    let base = ai_service::ensure_ready(&app).await?;
    let client = http_client(15)?;
    let resp = client
        .get(format!("{base}/local/models"))
        .send()
        .await
        .map_err(|e| format!("读取本地模型状态失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("读取本地模型状态失败: HTTP {}", resp.status()));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("解析本地模型状态失败: {e}"))
}
