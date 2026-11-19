//! AI 创作服务设置（FEAT-152 → FEAT-196 供应商库）——每用户一份，api_key 加密落库
//!
//! 存储：复用 `db::app_settings` KV 表，键 `ai_studio:{user_id}`，值是一段 JSON。
//! 为什么按用户隔离：模型配置属于「个人账号的偏好」，多用户同机时不应互相看到/覆盖。
//! 为什么 api_key 加密：明文密钥落 SQLite 一旦库文件被读出即泄露；复用 `crypto`
//! （AES-256-GCM + 应用主密钥）把密钥加密后存 `api_key_enc`，只在请求时解密。
//!
//! ## FEAT-196：从「三格各填一套地址」升级为「供应商库 + 能力位引用」
//!
//! 修前的形状是三个能力位（vision / edit / upscale）各自内联 `base_url + api_key +
//! model`。真实使用下同一家服务商会被三个能力位复用，于是同一个 baseUrl 要填三遍、
//! 换 key 要改三处；用户也只能「手打地址」，没有可选项。
//!
//! 新形状（**向后兼容旧数据**，见 `migrate_legacy`）：
//! - `providers`: 供应商库 —— 每项 `{id, name, base_url, api_key_enc}`；
//!   「服务商 + 凭证」只存一份，多个能力位可共用。
//! - `bindings`: 能力位 → `{provider_id, model}`。模型名留在绑定上而不是供应商上 ——
//!   同一家服务商在「图像理解」和「图像编辑」用不同模型是常态。
//!
//! 为什么 id 由前端生成并允许为空：空 id 表示「这条还没落库」（用户刚点了「+ 新增
//! 供应商」），保存时后端补一个。生成规则见 `ensure_ids`（`p{n}`，避开已存在的）。
//!
//! 分层（CS1）：本模块只做**持久层 + 视图转换 + 旧数据迁移**；命令层在下面（薄壳），
//! 进程管理与 HTTP 调用在 `ai_service` / `ai_studio`。

use serde::{Deserialize, Serialize};

use crate::db::Database;

/* ============================ 输入（前端 → 后端） ============================ */

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiProviderEntryInput {
    /// 供应商 id；空 = 新建（保存时分配）
    pub id: String,
    /// 展示名，如「阿里百炼」「Ollama（本机）」；空时回落 base_url 的 host
    pub name: String,
    pub base_url: String,
    /// 前端回传的可能是掩码（含 `…`）；为空或掩码表示「沿用已保存的密钥」
    pub api_key: String,
}

/// 能力位绑定：指向供应商库里的某一项 + 该能力位要用的模型名。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiBindingInput {
    /// 供应商 id；空 = 该能力位未配置
    pub provider_id: String,
    pub model: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettingsInput {
    pub providers: Vec<AiProviderEntryInput>,
    pub vision: Option<AiBindingInput>,
    pub edit: Option<AiBindingInput>,
    pub upscale: Option<AiBindingInput>,
}

/* ============================ 存储结构（落库 JSON） ============================ */

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredProvider {
    id: String,
    name: String,
    base_url: String,
    api_key_enc: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredBinding {
    provider_id: String,
    model: String,
}

/// 落库根对象。
///
/// `#[serde(default)]` 让「字段缺失」不报错 —— 但这**不足以**区分新旧格式：
/// 旧版把 `vision` 存成 `{base_url, api_key_enc, model}`，而新版的 `vision` 是
/// `{provider_id, model}`；由于子结构也带 default，旧对象会被**静默**解析成
/// `{provider_id: "", model: "旧模型名"}`，绑定随即被 `binding_view` 过滤掉，
/// 用户的配置看起来「凭空消失」。所以格式判定在 `load_stored` 里显式做（看有没有
/// `providers` 键），不依赖 serde 的宽容。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoredSettings {
    providers: Vec<StoredProvider>,
    vision: Option<StoredBinding>,
    edit: Option<StoredBinding>,
    upscale: Option<StoredBinding>,
}

/// 旧版三格的原始形状（只在迁移里用）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct LegacyProvider {
    base_url: String,
    api_key_enc: String,
    model: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct LegacyTriple {
    vision: Option<LegacyProvider>,
    edit: Option<LegacyProvider>,
    upscale: Option<LegacyProvider>,
}

/* ============================ 输出（后端 → 前端，apiKey 脱敏） ============================ */

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderEntryView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    /// 掩码（前 4 + … + 后 4），绝不回传明文密钥
    pub api_key: String,
    pub has_key: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiBindingView {
    pub provider_id: String,
    pub model: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettingsView {
    pub providers: Vec<AiProviderEntryView>,
    pub vision: Option<AiBindingView>,
    pub edit: Option<AiBindingView>,
    pub upscale: Option<AiBindingView>,
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
        Some(raw) => parse_stored(&raw),
    }
}

/// 解析落库 JSON：**先判格式再解析**（见 `StoredSettings` 的注释）。
///
/// 判据只看一个键：新版一定有 `providers`（哪怕是空数组，保存时必写）；
/// 旧版（≤ FEAT-192）只有三个能力位，没有这个键。用「有没有 providers 键」而不是
/// 「解析是否成功」—— 后者新旧都能成功，区分不了。
fn parse_stored(raw: &str) -> Result<StoredSettings, String> {
    let probe: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("AI 设置解析失败: {e}"))?;
    if probe.get("providers").is_some() {
        serde_json::from_str::<StoredSettings>(raw).map_err(|e| format!("AI 设置解析失败: {e}"))
    } else {
        let legacy: LegacyTriple =
            serde_json::from_value(probe).map_err(|e| format!("AI 设置解析失败: {e}"))?;
        Ok(migrate_legacy(legacy))
    }
}

fn save_stored(db: &Database, uid: i64, st: &StoredSettings) -> Result<(), String> {
    let raw = serde_json::to_string(st).map_err(|e| format!("AI 设置序列化失败: {e}"))?;
    db.set_setting(&settings_key(uid), &raw).map_err(|e| e.to_string())
}

/// 旧版三格 → 供应商库 + 绑定（内存态迁移，下次保存时落成新结构）。
///
/// 迁移口径：
/// - 相同 `base_url + api_key_enc` 的三个能力位**合并成一个供应商**（这正是用户
///   手工重复填了三遍的那一家）；不同的各成一个。
/// - 供应商 `name` 取 base_url 的 host（旧版没有名字字段），空 host 回落 `p{n}`。
/// - 绑定带上旧值里的 `model`（模型名是「能力位 × 供应商」的属性，不是供应商的）。
fn migrate_legacy(legacy: LegacyTriple) -> StoredSettings {
    let mut providers: Vec<StoredProvider> = Vec::new();
    let mut out = StoredSettings::default();

    /// 把一个能力位搬进供应商库，返回它对应的绑定。
    fn bind(
        providers: &mut Vec<StoredProvider>,
        p: &LegacyProvider,
    ) -> StoredBinding {
        let idx = providers
            .iter()
            .position(|x| x.base_url == p.base_url && x.api_key_enc == p.api_key_enc);
        let provider_id = match idx {
            Some(i) => providers[i].id.clone(),
            None => {
                let id = format!("p{}", providers.len() + 1);
                providers.push(StoredProvider {
                    id: id.clone(),
                    name: host_of(&p.base_url),
                    base_url: p.base_url.clone(),
                    api_key_enc: p.api_key_enc.clone(),
                });
                id
            }
        };
        StoredBinding {
            provider_id,
            model: p.model.clone(),
        }
    }

    fn take(
        providers: &mut Vec<StoredProvider>,
        slot: Option<LegacyProvider>,
    ) -> Option<StoredBinding> {
        let p = slot?;
        // 地址与模型都空 = 该能力位从未配置
        if p.base_url.trim().is_empty() && p.model.trim().is_empty() {
            return None;
        }
        Some(bind(providers, &p))
    }

    out.vision = take(&mut providers, legacy.vision);
    out.edit = take(&mut providers, legacy.edit);
    out.upscale = take(&mut providers, legacy.upscale);
    out.providers = providers;
    out
}

/// 从 baseUrl 里取 host 作默认展示名（旧版没有 name 字段）。
fn host_of(base_url: &str) -> String {
    let s = base_url.trim();
    if s.is_empty() {
        return String::new();
    }
    let after_scheme = s.split("://").nth(1).unwrap_or(s);
    let hostport = after_scheme.split('/').next().unwrap_or(after_scheme);
    let host = hostport.split(':').next().unwrap_or(hostport);
    if host.is_empty() {
        s.to_string()
    } else {
        host.to_string()
    }
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

fn provider_view(p: &StoredProvider) -> AiProviderEntryView {
    let plain = decrypt_key(&p.api_key_enc);
    AiProviderEntryView {
        id: p.id.clone(),
        name: if p.name.trim().is_empty() {
            host_of(&p.base_url)
        } else {
            p.name.clone()
        },
        base_url: p.base_url.clone(),
        api_key: mask(&plain),
        has_key: !plain.is_empty(),
    }
}

fn binding_view(b: &Option<StoredBinding>) -> Option<AiBindingView> {
    b.as_ref()
        .filter(|b| !b.provider_id.trim().is_empty())
        .map(|b| AiBindingView {
            provider_id: b.provider_id.clone(),
            model: b.model.clone(),
        })
}

fn to_view(st: &StoredSettings) -> AiSettingsView {
    AiSettingsView {
        providers: st.providers.iter().map(provider_view).collect(),
        vision: binding_view(&st.vision),
        edit: binding_view(&st.edit),
        upscale: binding_view(&st.upscale),
    }
}

/// 给缺失/重复的 id 补一个稳定值。
///
/// 为什么需要：前端允许「刚点 + 新增、还没保存」的空 id 条目；同一批里也可能因为
/// 复制粘贴出现重复 id。直接落库会让绑定指向一个不存在的供应商，表现为
/// 「选了供应商但运行时报未配置」。
fn ensure_ids(list: &mut [StoredProvider]) {
    let mut used: Vec<String> = Vec::new();
    for p in list.iter_mut() {
        if p.id.trim().is_empty() || used.contains(&p.id) {
            let mut n = used.len() + 1;
            let mut candidate = format!("p{n}");
            while used.contains(&candidate) {
                n += 1;
                candidate = format!("p{n}");
            }
            p.id = candidate;
        }
        used.push(p.id.clone());
    }
}

/// 合并供应商库：按 id 对齐，掩码/空 key 沿用旧密钥；不在入参里的供应商被删除。
fn merge_providers(
    inputs: &[AiProviderEntryInput],
    old: &[StoredProvider],
) -> Result<Vec<StoredProvider>, String> {
    let mut out: Vec<StoredProvider> = Vec::with_capacity(inputs.len());
    for inp in inputs {
        let base = inp.base_url.trim().to_string();
        let name = inp.name.trim().to_string();
        let key_in = inp.api_key.trim().to_string();
        // 整条视为删除：没地址也没 key（模型名在绑定上，不参与这条判定）
        if base.is_empty() {
            continue;
        }
        let keep_old = key_in.is_empty() || key_in.contains('…') || key_in.contains("****");
        let enc = if keep_old {
            old.iter()
                .find(|o| o.id == inp.id)
                .map(|o| o.api_key_enc.clone())
                .unwrap_or_default()
        } else {
            crate::crypto::encrypt(&key_in)?
        };
        out.push(StoredProvider {
            id: inp.id.trim().to_string(),
            name: if name.is_empty() { host_of(&base) } else { name },
            base_url: base,
            api_key_enc: enc,
        });
    }
    ensure_ids(&mut out);
    Ok(out)
}

/// 归一化一条绑定：供应商 id 不在库里 → 视为未配置（不静默指向空气）。
fn merge_binding(
    input: &Option<AiBindingInput>,
    old: &Option<StoredBinding>,
    providers: &[StoredProvider],
) -> Option<StoredBinding> {
    let Some(inp) = input else {
        // 不传 = 保留旧值，但旧值引用的供应商若已被删，同样要清掉
        return old
            .as_ref()
            .filter(|b| providers.iter().any(|p| p.id == b.provider_id))
            .cloned();
    };
    let pid = inp.provider_id.trim().to_string();
    if pid.is_empty() || !providers.iter().any(|p| p.id == pid) {
        return None;
    }
    Some(StoredBinding {
        provider_id: pid,
        model: inp.model.trim().to_string(),
    })
}

/* ============================ 对外读取（命令层 / ai_studio 共用） ============================ */

/// 取某用户某能力的明文 provider（api_key 已解密）。未配置返回 None。
///
/// 能力位 → 供应商 → 模型名的两级解析都收在这一处：`ai_studio` 只认 kind，
/// 不需要知道供应商库长什么样。
pub(crate) fn provider_for(db: &Database, uid: i64, kind: &str) -> Result<Option<PlainProvider>, String> {
    let st = load_stored(db, uid)?;
    let binding = match kind {
        "vision" => st.vision,
        "edit" => st.edit,
        "upscale" => st.upscale,
        _ => None,
    };
    let Some(b) = binding else { return Ok(None) };
    let Some(p) = st.providers.iter().find(|p| p.id == b.provider_id) else {
        return Ok(None);
    };
    Ok(Some(PlainProvider {
        base_url: p.base_url.clone(),
        api_key: decrypt_key(&p.api_key_enc),
        model: b.model,
    }))
}

/// 按供应商 id 取「地址 + 明文密钥」（不含模型名）。
///
/// 供「连通性测试」用：测试的对象是**供应商 + 模型**这一对，而模型名在绑定上。
/// 单独给出这个函数，是为了让测试命令能在「用户刚改了 baseUrl、还没保存」时
/// 用旧密钥配新地址试一次 —— 不必强迫用户先保存再测。
pub(crate) fn provider_entry(
    db: &Database,
    uid: i64,
    provider_id: &str,
) -> Result<Option<(String, String)>, String> {
    let st = load_stored(db, uid)?;
    Ok(st
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .map(|p| (p.base_url.clone(), decrypt_key(&p.api_key_enc))))
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
    Ok(to_view(&st))
}

/// 保存当前用户 AI 设置（返回保存后的脱敏视图）
///
/// 入参的 `providers` 是**全量替换**（前端一次提交整张供应商表 + 三个绑定）：
/// 供应商条目的增删改都在这一张表里表达，比「逐条 CRUD」少一半命令与竞态。
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

    let providers = merge_providers(&input.providers, &old.providers)?;
    let merged = StoredSettings {
        vision: merge_binding(&input.vision, &old.vision, &providers),
        edit: merge_binding(&input.edit, &old.edit, &providers),
        upscale: merge_binding(&input.upscale, &old.upscale, &providers),
        providers,
    };
    save_stored(&db, uid, &merged)?;
    crate::logger::log_info(&format!(
        "[ai-studio] 设置已保存 uid={uid} 供应商={} vision={} edit={} upscale={}",
        merged.providers.len(),
        merged.vision.as_ref().map(|b| b.provider_id.as_str()).unwrap_or("-"),
        merged.edit.as_ref().map(|b| b.provider_id.as_str()).unwrap_or("-"),
        merged.upscale.as_ref().map(|b| b.provider_id.as_str()).unwrap_or("-"),
    ));
    Ok(to_view(&merged))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_p(base: &str, model: &str) -> LegacyProvider {
        LegacyProvider {
            base_url: base.into(),
            api_key_enc: "enc-same".into(),
            model: model.into(),
        }
    }

    /// 三个能力位填同一家服务商 → 迁移后只应产出**一个**供应商 + 三条绑定。
    /// 这是用户真实抱怨的场景（同一 baseUrl 填三遍）。
    #[test]
    fn migrate_merges_same_endpoint() {
        let st = migrate_legacy(LegacyTriple {
            vision: Some(legacy_p("https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen-vl-max")),
            edit: Some(legacy_p("https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen-image-edit")),
            upscale: None,
        });
        assert_eq!(st.providers.len(), 1, "同地址同密钥应合并成一个供应商");
        assert_eq!(st.providers[0].name, "dashscope.aliyuncs.com");
        assert_eq!(st.vision.as_ref().unwrap().provider_id, st.providers[0].id);
        assert_eq!(st.vision.as_ref().unwrap().model, "qwen-vl-max");
        assert_eq!(st.edit.as_ref().unwrap().model, "qwen-image-edit");
        assert!(st.upscale.is_none());
    }

    /// 不同地址 → 两个供应商，id 不重复。
    #[test]
    fn migrate_keeps_distinct_endpoints() {
        let mut a = legacy_p("http://127.0.0.1:11434/v1", "m1");
        a.api_key_enc = "enc-a".into();
        let mut b = legacy_p("https://api.example.com/v1", "m2");
        b.api_key_enc = "enc-b".into();
        let st = migrate_legacy(LegacyTriple {
            vision: Some(a),
            edit: Some(b),
            upscale: None,
        });
        assert_eq!(st.providers.len(), 2);
        assert_ne!(st.vision.as_ref().unwrap().provider_id, st.edit.as_ref().unwrap().provider_id);
    }

    /// 绑定指向不存在的供应商必须被清掉，而不是留一个死引用。
    /// 症状会是「界面上选了供应商、运行时却报未配置」，很难排查。
    #[test]
    fn binding_to_missing_provider_is_dropped() {
        let providers = vec![StoredProvider {
            id: "p1".into(),
            name: "x".into(),
            base_url: "https://a/v1".into(),
            api_key_enc: "e".into(),
        }];
        let got = merge_binding(
            &Some(AiBindingInput {
                provider_id: "p9".into(),
                model: "m".into(),
            }),
            &None,
            &providers,
        );
        assert!(got.is_none());
    }

    /// 空 id 应被补成不重复的值，否则两条供应商会撞 id，绑定只能指向其中一条。
    #[test]
    fn ensure_ids_fills_blanks_and_dedupes() {
        let mut list = vec![
            StoredProvider { id: String::new(), ..Default::default() },
            StoredProvider { id: String::new(), ..Default::default() },
            StoredProvider { id: "p1".into(), ..Default::default() },
        ];
        ensure_ids(&mut list);
        let ids: Vec<_> = list.iter().map(|p| p.id.clone()).collect();
        let uniq: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(uniq.len(), ids.len(), "id 必须两两不同: {ids:?}");
        assert!(ids.iter().all(|i| !i.trim().is_empty()));
    }

    /// 旧数据的「掩码/空 key 沿用」：供应商库同样适用。
    #[test]
    fn merge_providers_keeps_old_key_on_mask() {
        let old = vec![StoredProvider {
            id: "p1".into(),
            name: "旧".into(),
            base_url: "https://a/v1".into(),
            api_key_enc: "enc-old".into(),
        }];
        let out = merge_providers(
            &[AiProviderEntryInput {
                id: "p1".into(),
                name: "新名".into(),
                base_url: "https://a/v1".into(),
                api_key: "••••…••••".into(),
            }],
            &old,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].api_key_enc, "enc-old", "掩码应沿用旧密钥");
        assert_eq!(out[0].name, "新名");
    }

    /// 地址被清空 = 删除该供应商。
    #[test]
    fn merge_providers_drops_empty_base() {
        let out = merge_providers(
            &[AiProviderEntryInput {
                id: "p1".into(),
                name: "x".into(),
                base_url: "   ".into(),
                api_key: "k".into(),
            }],
            &[],
        )
        .unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn host_of_handles_scheme_and_port() {
        assert_eq!(host_of("http://127.0.0.1:11434/v1"), "127.0.0.1");
        assert_eq!(host_of("https://dashscope.aliyuncs.com/compatible-mode/v1"), "dashscope.aliyuncs.com");
        assert_eq!(host_of(""), "");
        assert_eq!(host_of("example.com/v1"), "example.com");
    }

    /// **核心回归**：旧格式（只有三个能力位、没有 `providers` 键）必须被识别为旧格式
    /// 并迁移，不能被「新结构 + serde(default)」静默吃成空配置。
    ///
    /// 静默吃掉的症状：升级后打开设置，三格全空、供应商列表空 —— 用户会认为配置丢了。
    /// 判据必须是「有没有 providers 键」，不是「解析是否成功」（新旧都能成功）。
    #[test]
    fn parse_stored_detects_legacy_format() {
        let legacy_json = r#"{
            "vision": {"base_url": "https://a.example.com/v1", "api_key_enc": "enc1", "model": "m-v"},
            "edit":   {"base_url": "https://a.example.com/v1", "api_key_enc": "enc1", "model": "m-e"}
        }"#;
        let st = parse_stored(legacy_json).unwrap();
        assert_eq!(st.providers.len(), 1, "旧数据应迁移出 1 个供应商");
        assert_eq!(st.providers[0].id, "p1");
        assert_eq!(st.vision.as_ref().unwrap().model, "m-v");
        assert_eq!(st.edit.as_ref().unwrap().model, "m-e");
        assert!(st.upscale.is_none());
    }

    /// 新格式（有 providers 键，哪怕空数组）走新解析路径，不应触发迁移。
    #[test]
    fn parse_stored_accepts_new_format_even_if_empty() {
        let new_json = r#"{"providers": [], "vision": null, "edit": null, "upscale": null}"#;
        let st = parse_stored(new_json).unwrap();
        assert!(st.providers.is_empty());
        assert!(st.vision.is_none());
    }

    /// 新格式里「有供应商但没绑定」是合法中间态（用户加了供应商还没绑能力位）。
    #[test]
    fn parse_stored_keeps_provider_without_binding() {
        let j = r#"{
            "providers": [{"id":"p1","name":"百炼","base_url":"https://a/v1","api_key_enc":"e"}],
            "vision": null, "edit": null, "upscale": null
        }"#;
        let st = parse_stored(j).unwrap();
        assert_eq!(st.providers.len(), 1);
        assert!(st.vision.is_none());
    }
}
