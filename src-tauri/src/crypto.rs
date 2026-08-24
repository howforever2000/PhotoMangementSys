//! 用户敏感字段加密（AES-256-GCM）
//!
//! 需求：除账户名外，用户信息不以原字段存储在数据库中 —— 邮箱 / 手机号 / 密码哈希
//! 落库前用应用加密密钥做 AES-256-GCM 加密，防止他人直接读取数据库表窃取或篡改。
//!
//! 安全约定：
//! - 主密钥保存在应用数据目录下的 `app.key`（独立于数据库文件）。仅拿走数据库文件
//!   无法解密；需同时拿到密钥文件才有意义（At-Rest 加密的常规边界）。
//! - 采用 AES-256-GCM，每次加密随机生成 12 字节 nonce，密文自带 16 字节认证标签，
//!   可检测篡改与密钥不匹配。
//! - 存储格式 = `v1:` + base64(nonce(12) || ciphertext || tag(16))。「v1:」前缀用于
//!   与升级迁移前的历史明文数据区分（见 auth 中的单次迁移）。
//! - 密码本身不落明文：先做 Argon2id 哈希，再将该哈希串加密落库（双层防护）。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand_core::RngCore;
use std::path::Path;
use std::sync::OnceLock;

/// 主密钥（32 字节）。全局仅初始化一次，多线程共享只读。
static MASTER_KEY: OnceLock<[u8; 32]> = OnceLock::new();

/// 密钥文件名（位于应用数据目录）
const KEY_FILE: &str = "app.key";
/// 密文前缀标记（版本号 + 与历史明文区分的哨兵）
const PREFIX: &str = "v1:";
/// nonce 长度（AES-GCM 标准推荐 12 字节）
const NONCE_LEN: usize = 12;
/// GCM 认证标签长度
const TAG_LEN: usize = 16;

/// 初始化 / 加载主密钥（首次运行生成并写入 `app_data_dir/app.key`）。
/// 应早于数据库建表与迁移调用（迁移需要密钥来加密历史明文）。
pub fn init(app_data_dir: &Path) -> Result<(), String> {
    if MASTER_KEY.get().is_some() {
        return Ok(()); // 已初始化（多次调用 / 测试）
    }
    let key_path = app_data_dir.join(KEY_FILE);
    let key = if key_path.exists() {
        read_key_file(&key_path)?
    } else {
        let k = generate_key();
        write_key_file(&key_path, &k)?;
        k
    };
    let _ = MASTER_KEY.set(key);
    Ok(())
}

/// 测试用：以固定密钥初始化（无需写磁盘）。在测试 binary 中只需设置一次。
#[cfg(test)]
pub fn init_for_test() {
    let _ = MASTER_KEY.set([7u8; 32]);
}

fn generate_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut k);
    k
}

fn read_key_file(path: &Path) -> Result<[u8; 32], String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取加密密钥失败: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| "加密密钥文件损坏（长度应为 32 字节）".to_string())
}

fn write_key_file(path: &Path, key: &[u8; 32]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建密钥目录失败: {e}"))?;
    }
    std::fs::write(path, key).map_err(|e| format!("写入加密密钥失败: {e}"))
}

fn cipher() -> Result<Aes256Gcm, String> {
    let key = MASTER_KEY.get().ok_or("应用加密密钥未初始化")?;
    Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())
}

/// 加密明文 → `v1:` + base64(nonce||ct||tag)。空串原样返回（避免无效密文）。
pub fn encrypt(plain: &str) -> Result<String, String> {
    if plain.is_empty() {
        return Ok(String::new());
    }
    let c = cipher()?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand_core::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = c
        .encrypt(nonce, plain.as_bytes())
        .map_err(|e| format!("加密失败: {e}"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(format!("{PREFIX}{}", STANDARD.encode(out)))
}

/// 解密 `v1:` 密文。若遇到历史明文（升级迁移前遗留），原样返回以兼容旧数据。
pub fn decrypt(data: &str) -> Result<String, String> {
    if data.is_empty() {
        return Ok(String::new());
    }
    if let Some(enc) = data.strip_prefix(PREFIX) {
        let raw = STANDARD
            .decode(enc)
            .map_err(|e| format!("密文解码失败: {e}"))?;
        if raw.len() < NONCE_LEN + TAG_LEN {
            return Err("密文长度非法".into());
        }
        let (nonce_bytes, ct) = raw.split_at(NONCE_LEN);
        let c = cipher()?;
        let nonce = Nonce::from_slice(nonce_bytes);
        let plain = c
            .decrypt(nonce, ct)
            .map_err(|e| format!("解密失败（密钥不匹配或数据被篡改）: {e}"))?;
        String::from_utf8(plain).map_err(|e| format!("解密结果非 UTF-8: {e}"))
    } else {
        Ok(data.to_string())
    }
}

/// 判断某字段是否已是密文（迁移探测用）
pub fn is_encrypted(data: &str) -> bool {
    data.starts_with(PREFIX)
}

/// 是否已完成密钥初始化（迁移等只在密钥就绪后才做加密）
pub fn is_initialized() -> bool {
    MASTER_KEY.get().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_randomized() {
        init_for_test();
        let e1 = encrypt("user@example.com").unwrap();
        let e2 = encrypt("user@example.com").unwrap();
        assert!(e1.starts_with("v1:"));
        // 随机 nonce → 同一明文两次密文不同
        assert_ne!(e1, e2, "随机 nonce 应使同一明文产生不同密文");
        assert_eq!(decrypt(&e1).unwrap(), "user@example.com");
        assert_eq!(decrypt(&e2).unwrap(), "user@example.com");
        assert!(is_encrypted(&e1));
    }

    #[test]
    fn empty_and_legacy_plaintext() {
        init_for_test();
        // 空串
        assert_eq!(encrypt("").unwrap(), "");
        assert_eq!(decrypt("").unwrap(), "");
        // 历史明文按安全值返回（迁移前兼容）
        assert_eq!(decrypt("user@example.com").unwrap(), "user@example.com");
        assert!(!is_encrypted("user@example.com"));
        // 篡改的密文 → detect
        let e = encrypt("secret").unwrap();
        let mut raw = STANDARD.decode(e.strip_prefix("v1:").unwrap()).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xFF;
        let tampered = format!("v1:{}", STANDARD.encode(raw));
        assert!(decrypt(&tampered).is_err(), "篡改应触发 GCM tag 校验失败");
    }
}
