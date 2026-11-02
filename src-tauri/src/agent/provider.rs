//! 模型端点配置：落盘加密 + 只向前端暴露"非敏感字段"。
//!
//! ## 三条硬约束（docs/v5.md §2、§8）
//!
//! 1. **落盘只有 Rust**：`app_settings` KV，值 = `crypto::encrypt(JSON)`，
//!    密钥只在一处（`app.key`）加密。Python 侧拿不到任何持久化凭据。
//! 2. **明文只在一次 submit 的请求体里出现**：per-job 注入、用完即弃；
//!    Python 不写日志、不进异常栈（见 `agent/logutil.redact`）。
//! 3. **前端永远拿不到 Key**：`ProviderPublic` 只带 `hasKey` 与打码尾号，
//!    连"编辑时回填"都靠"留空=不改"。
//!
//! ## 为什么是"一个用户一份数组"而不是每端点一行
//!
//! 端点数量是个位数，整体读写更简单（一次解密拿到全部），也避免
//! "索引与内容不同步"这种只有并发才暴露的问题。

use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::db::Database;
use rand_core::RngCore as _;

/// 设置键前缀：`agent.providers.<uid>` → 加密后的 JSON 数组
fn setting_key(uid: i64) -> String {
    format!("agent.providers.{uid}")
}

/// 端点的完整配置（含明文 Key）。**只存在于内存与本机加密存储里。**
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderConfig {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub model: String,
    /// 明文 Key。落盘前整体加密；给前端前剔除。
    pub api_key: String,
    /// 每张图（或每次规划调用）的单价，用户自填；0 = 未知
    pub unit_price: f64,
    /// image | planner | both
    pub kind: String,
    pub async_poll: bool,
}

/// 给前端的视图：**不含 Key**
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPublic {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub model: String,
    pub unit_price: f64,
    pub kind: String,
    pub async_poll: bool,
    /// 是否已存 Key（不告诉前端 Key 是什么）
    pub has_key: bool,
    /// 打码尾号，帮用户辨认"是不是那把"
    pub key_hint: String,
}

impl ProviderConfig {
    fn public(&self) -> ProviderPublic {
        ProviderPublic {
            id: self.id.clone(),
            label: self.label.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            unit_price: self.unit_price,
            kind: self.kind.clone(),
            async_poll: self.async_poll,
            has_key: !self.api_key.is_empty(),
            key_hint: mask_key(&self.api_key),
        }
    }

    /// 传给 Python 的端点视图（含明文 Key，仅随一次请求存活）。
    pub fn to_endpoint_json(&self) -> serde_json::Value {
        serde_json::json!({
            "baseUrl": self.base_url,
            "apiKey": self.api_key,
            "model": self.model,
            "unitPrice": self.unit_price,
            "kind": self.kind,
            "asyncPoll": self.async_poll,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.base_url.trim().is_empty() {
            return Err("请填写模型地址（例如 https://host/v1 或 http://127.0.0.1:11434/v1）".into());
        }
        if self.model.trim().is_empty() {
            return Err("请填写模型名（每个端点的模型名不同，需要你按服务商文档填）".into());
        }
        if !(self.kind == "image" || self.kind == "planner" || self.kind == "both") {
            return Err("端点类型只能是 image / planner / both".into());
        }
        Ok(())
    }
}

fn mask_key(k: &str) -> String {
    let n = k.chars().count();
    if n == 0 {
        return String::new();
    }
    if n <= 10 {
        return "*".repeat(n);
    }
    let head: String = k.chars().take(6).collect();
    let tail: String = k.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// 读某个用户的全部端点（解密失败视为"配置损坏"，返回空而不是崩）。
pub fn load(db: &Database, uid: i64) -> Vec<ProviderConfig> {
    let Some(raw) = db.get_setting(&setting_key(uid)).ok().flatten() else {
        return Vec::new();
    };
    let plain = match crypto::decrypt(&raw) {
        Ok(p) => p,
        Err(e) => {
            crate::logger::log_error(
                "agent_provider_load",
                &format!("端点配置解密失败（当作未配置处理）：{e}"),
            );
            return Vec::new();
        }
    };
    serde_json::from_str(&plain).unwrap_or_default()
}

fn save_all(db: &Database, uid: i64, items: &[ProviderConfig]) -> Result<(), String> {
    let plain = serde_json::to_string(items).map_err(|e| format!("序列化端点配置失败: {e}"))?;
    let encrypted = crypto::encrypt(&plain)?;
    db.set_setting(&setting_key(uid), &encrypted)
        .map_err(|e| format!("保存端点配置失败: {e}"))
}

pub fn list_public(db: &Database, uid: i64) -> Vec<ProviderPublic> {
    load(db, uid).iter().map(|c| c.public()).collect()
}

/// 新增或更新。`id` 为空 = 新增（生成随机 id）。
/// Key 留空表示"沿用原来的 Key"（前端不回填 Key，只能这样改其它字段）。
pub fn upsert(
    db: &Database,
    uid: i64,
    mut input: ProviderConfig,
) -> Result<Vec<ProviderPublic>, String> {
    input.base_url = input.base_url.trim().trim_end_matches('/').to_string();
    input.model = input.model.trim().to_string();
    input.label = input.label.trim().to_string();
    if input.label.is_empty() {
        input.label = input.model.clone();
    }
    if input.kind.trim().is_empty() {
        input.kind = "image".into();
    }

    let mut items = load(db, uid);
    let is_new = input.id.trim().is_empty();
    if is_new {
        let mut buf = [0u8; 8];
        rand_core::OsRng.fill_bytes(&mut buf);
        input.id = buf.iter().map(|b| format!("{b:02x}")).collect();
    } else if let Some(old) = items.iter().find(|c| c.id == input.id) {
        if input.api_key.trim().is_empty() {
            // 留空 = 不改 Key（前端拿不到明文，不能回填）
            input.api_key = old.api_key.clone();
        }
    }
    input.validate()?;
    if input.api_key.trim().is_empty() && !looks_local(&input.base_url) {
        return Err("公网端点必须填写 API Key（本地地址可以不填）".into());
    }

    if is_new {
        items.push(input);
    } else {
        match items.iter_mut().find(|c| c.id == input.id) {
            Some(slot) => *slot = input,
            None => return Err("要更新的端点不存在（可能已被删除）".into()),
        }
    }
    save_all(db, uid, &items)?;
    Ok(items.iter().map(|c| c.public()).collect())
}

pub fn delete(db: &Database, uid: i64, id: &str) -> Result<Vec<ProviderPublic>, String> {
    let mut items = load(db, uid);
    let before = items.len();
    items.retain(|c| c.id != id);
    if items.len() == before {
        return Err("端点不存在".into());
    }
    save_all(db, uid, &items)?;
    Ok(items.iter().map(|c| c.public()).collect())
}

pub fn find(db: &Database, uid: i64, id: &str) -> Option<ProviderConfig> {
    load(db, uid).into_iter().find(|c| c.id == id)
}

fn looks_local(url: &str) -> bool {
    let u = url.to_lowercase();
    ["127.0.0.1", "localhost", "0.0.0.0", "[::1]"]
        .iter()
        .any(|h| u.contains(h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 每个用例一个独立库文件：避免并行测试在同一个 SQLite 上抢锁。
    fn db() -> Database {
        crate::crypto::init_for_test();
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "pm-provider-test-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("photos.db");
        let _ = std::fs::remove_file(&path);
        Database::open(&path).expect("建测试库失败")
    }

    fn input(base: &str, key: &str, price: f64) -> ProviderConfig {
        ProviderConfig {
            label: "我的端点".into(),
            base_url: base.into(),
            model: "img-model".into(),
            api_key: key.into(),
            unit_price: price,
            kind: "image".into(),
            ..Default::default()
        }
    }

    #[test]
    fn save_encrypts_and_never_returns_the_key() {
        let db = db();
        let list = upsert(&db, 1, input("https://api.example.com/v1", "sk-secret-123456", 0.15)).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].has_key);
        assert_eq!(list[0].key_hint, "sk-sec…3456");
        // 落盘内容里不能出现明文
        let raw = db.get_setting(&setting_key(1)).unwrap().unwrap();
        assert!(!raw.contains("sk-secret-123456"), "落盘必须是密文");
        assert!(crypto::is_encrypted(&raw));
    }

    #[test]
    fn update_with_empty_key_keeps_old_key() {
        let db = db();
        let list = upsert(&db, 1, input("https://api.example.com/v1", "sk-secret-123456", 0.15)).unwrap();
        let id = list[0].id.clone();
        let mut upd = input("https://api.example.com/v1", "", 0.2);
        upd.id = id.clone();
        upd.model = "img-model-v2".into();
        upsert(&db, 1, upd).unwrap();
        let cfg = find(&db, 1, &id).unwrap();
        assert_eq!(cfg.model, "img-model-v2");
        assert_eq!(cfg.api_key, "sk-secret-123456", "留空应沿用原 Key");
        assert!((cfg.unit_price - 0.2).abs() < 1e-9);
    }

    #[test]
    fn public_endpoint_requires_key_but_local_does_not() {
        let db = db();
        assert!(upsert(&db, 1, input("https://api.example.com/v1", "", 0.0)).is_err());
        let list = upsert(&db, 1, input("http://127.0.0.1:11434/v1", "", 0.0)).unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].has_key);
    }

    #[test]
    fn endpoints_are_isolated_per_user() {
        let db = db();
        upsert(&db, 1, input("https://a.example.com/v1", "sk-aaaaaaaaaaaa", 0.1)).unwrap();
        upsert(&db, 2, input("https://b.example.com/v1", "sk-bbbbbbbbbbbb", 0.2)).unwrap();
        assert_eq!(list_public(&db, 1).len(), 1);
        assert_eq!(list_public(&db, 1)[0].base_url, "https://a.example.com/v1");
        assert_eq!(list_public(&db, 2)[0].base_url, "https://b.example.com/v1");
    }

    #[test]
    fn delete_removes_only_that_one() {
        let db = db();
        let a = upsert(&db, 1, input("https://a.example.com/v1", "sk-aaaaaaaaaaaa", 0.1)).unwrap();
        let id_a = a[0].id.clone();
        upsert(&db, 1, input("https://b.example.com/v1", "sk-bbbbbbbbbbbb", 0.2)).unwrap();
        let left = delete(&db, 1, &id_a).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].base_url, "https://b.example.com/v1");
        assert!(delete(&db, 1, &id_a).is_err(), "重复删除应报错");
    }

    #[test]
    fn validate_rejects_missing_fields() {
        let mut c = input("", "sk-x", 0.0);
        c.base_url = "  ".into();
        assert!(c.validate().is_err());
        let mut c2 = input("https://a/v1", "sk-x", 0.0);
        c2.model = "".into();
        assert!(c2.validate().is_err());
    }

    #[test]
    fn endpoint_json_carries_plain_key_for_one_request() {
        let c = input("https://api.example.com/v1", "sk-secret-123456", 0.15);
        let v = c.to_endpoint_json();
        assert_eq!(v["apiKey"], "sk-secret-123456");
        assert_eq!(v["unitPrice"], 0.15);
        assert_eq!(v["kind"], "image");
    }
}
