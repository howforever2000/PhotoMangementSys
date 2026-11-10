//! AI 创作服务设置（FEAT-152）——每用户一份，api_key 加密落库
//!
//! 存储：复用 `db::app_settings` KV 表，键 `ai_studio:{user_id}`，值是一段 JSON。
//! 为什么按用户隔离：模型配置属于「个人账号的偏好」，多用户同机时不应互相看到/覆盖。
//! 为什么 api_key 加密：明文密钥落 SQLite 一旦库文件被读出即泄露；复用 `crypto`
//! （AES-256-GCM + 应用主密钥）把密钥加密后存 `api_key_enc`，只在请求时解密。
//!
//! 分层（CS1）：本模块只做**持久层 + 视图转换**；命令层在下面（薄壳），
//! 进程管理与 HTTP 调用在 `ai_service` / `ai_studio`。

use serde::{Deserialize, Serialize};

use crate::db::Database;

/* ============================ 输入（前端 → 后端） ============================ */

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiProviderInput {
    pub base_url: String,
    /// 前端回传的可能是掩码（含 `…`）；为空或掩码表示「沿用已保存的密钥」
    pub api_key: String,
    pub model: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiProvidersInput {
    pub vision: Option<AiProviderInput>,
    pub edit: Option<AiProviderInput>,
    pub upscale: Option<AiProviderInput>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettingsInput {
    pub providers: AiProvidersInput,
}

/* ============================ 存储结构（落库 JSON） ============================ */

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredProvider {
    base_url: String,
    api_key_enc: String,
    model: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredSettings {
    vision: Option<StoredProvider>,
    edit: Option<StoredProvider>,
    upscale: Option<StoredProvider>,
}

/* ============================ 输出（后端 → 前端，apiKey 脱敏） ============================ */

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderView {
    pub base_url: String,
    /// 掩码（前 4 + … + 后 4），绝不回传明文密钥
    pub api_key: String,
    pub model: String,
    pub has_key: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProvidersView {
    pub vision: Option<AiProviderView>,
    pub edit: Option<AiProviderView>,
    pub upscale: Option<AiProviderView>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettingsView {
    pub providers: AiProvidersView,
}

/* ============================ 明文 provider（供 ai_studio 组装请求） ============================ */

#[derive(Clone, Debug)]
pub struct PlainProvider {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/* ============================ 持久层 ============================ */

fn settings_key(uid: i64) -> String {
    format!("ai_studio:{uid}")
}

fn load_stored(db: &Database, uid: i64) -> Result<StoredSettings, String> {
    match db.get_setting(&settings_key(uid)).map_err(|e| e.to_string())? {
        None => Ok(StoredSettings::default()),
        Some(raw) if raw.trim().is_empty() => Ok(StoredSettings::default()),
        Some(raw) => serde_json::from_str::<StoredSettings>(&raw)
            .map_err(|e| format!("AI 设置解析失败: {e}")),
    }
}

fn save_stored(db: &Database, uid: i64, st: &StoredSettings) -> Result<(), String> {
    let raw = serde_json::to_string(st).map_err(|e| format!("AI 设置序列化失败: {e}"))?;
    db.set_setting(&settings_key(uid), &raw).map_err(|e| e.to_string())
}

fn decrypt_key(enc: &str) -> String {
    if enc.is_empty() {
        return String::new();
    }
    if crate::crypto::is_encrypted(enc) {
        crate::crypto::decrypt(enc).unwrap_or_default()
    } else {
        enc.to_string() // 兼容早期明文（理论上不会有）
    }
}

fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() <= 8 {
        return "••••".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

fn view_of(p: &StoredProvider) -> AiProviderView {
    let plain = decrypt_key(&p.api_key_enc);
    AiProviderView {
        base_url: p.base_url.clone(),
        api_key: mask(&plain),
        model: p.model.clone(),
        has_key: !plain.is_empty(),
    }
}

fn to_view(st: &StoredSettings) -> AiProvidersView {
    AiProvidersView {
        vision: st.vision.as_ref().map(view_of),
        edit: st.edit.as_ref().map(view_of),
        upscale: st.upscale.as_ref().map(view_of),
    }
}

/// 合并单个 provider：空/掩码 key 时沿用旧密钥；三项都空则视为「删除该项」。
fn merge_provider(
    input: &AiProviderInput,
    old: Option<&StoredProvider>,
) -> Result<Option<StoredProvider>, String> {
    let base = input.base_url.trim().to_string();
    let model = input.model.trim().to_string();
    let key_in = input.api_key.trim().to_string();
    let keep_old = key_in.is_empty() || key_in.contains('…') || key_in.contains("****");
    let enc = if keep_old {
        old.map(|o| o.api_key_enc.clone()).unwrap_or_default()
    } else {
        crate::crypto::encrypt(&key_in)?
    };
    if base.is_empty() && model.is_empty() && enc.is_empty() {
        return Ok(None);
    }
    Ok(Some(StoredProvider {
        base_url: base,
        api_key_enc: enc,
        model,
    }))
}

/* ============================ 对外读取（命令层 / ai_studio 共用） ============================ */

/// 取某用户某能力的明文 provider（api_key 已解密）。未配置返回 None。
pub(crate) fn provider_for(db: &Database, uid: i64, kind: &str) -> Result<Option<PlainProvider>, String> {
    let st = load_stored(db, uid)?;
    let sp = match kind {
        "vision" => st.vision,
        "edit" => st.edit,
        "upscale" => st.upscale,
        _ => None,
    };
    Ok(sp.map(|p| PlainProvider {
        base_url: p.base_url,
        api_key: decrypt_key(&p.api_key_enc),
        model: p.model,
    }))
}

/* ============================ 命令层（薄壳） ============================ */

/// 读取当前用户 AI 设置（apiKey 返回掩码）
#[tauri::command]
pub fn ai_get_settings(
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<AiSettingsView, String> {
    let _t = log_call!("ai_get_settings");
    let uid = crate::require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let st = load_stored(&db, uid)?;
    Ok(AiSettingsView {
        providers: to_view(&st),
    })
}

/// 保存当前用户 AI 设置（返回保存后的脱敏视图）
#[tauri::command]
pub fn ai_set_settings(
    input: AiSettingsInput,
    state: tauri::State<'_, crate::AppState>,
    session: tauri::State<'_, crate::SessionState>,
) -> Result<AiSettingsView, String> {
    let _t = log_call!("ai_set_settings");
    let uid = crate::require_user(&session)?;
    let db = state.0.lock().map_err(|e| e.to_string())?;
    let old = load_stored(&db, uid)?;
    let merged = StoredSettings {
        vision: match &input.providers.vision {
            Some(v) => merge_provider(v, old.vision.as_ref())?,
            None => old.vision.clone(),
        },
        edit: match &input.providers.edit {
            Some(v) => merge_provider(v, old.edit.as_ref())?,
            None => old.edit.clone(),
        },
        upscale: match &input.providers.upscale {
            Some(v) => merge_provider(v, old.upscale.as_ref())?,
            None => old.upscale.clone(),
        },
    };
    save_stored(&db, uid, &merged)?;
    crate::logger::log_info(&format!(
        "[ai-studio] 设置已保存 uid={uid} vision={} edit={} upscale={}",
        merged.vision.is_some(),
        merged.edit.is_some(),
        merged.upscale.is_some()
    ));
    Ok(AiSettingsView {
        providers: to_view(&merged),
    })
}
