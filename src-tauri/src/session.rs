//! 记住登录模块（R2：默认 3 天免密登录，直接复用上次用户）
//!
//! 职责：
//! - `sessions` 表：登录时生成随机 token，记录 user_id / 过期时间（默认 3 天）
//! - 启动时读磁盘 token 文件 → 校验有效期 → 自动恢复登录会话（免密复用上次用户）
//! - 登出时清除 token（DB 行 + 磁盘文件）
//!
//! 解耦原则（C1）：
//! - 密码逻辑在 `auth`，本模块只管「记住登录」的 token 生命周期，互不依赖
//! - 不依赖 `db` 的 Database 类型，仅接收 `&Connection`，可独立测试
//! - 命令层（lib.rs）只做薄壳转发

use std::path::Path;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use rusqlite::{params, Connection};

/// 默认记住时长：3 天（Unix 秒）
pub const REMEMBER_TTL_SECS: i64 = 3 * 24 * 3600;

/// 磁盘上的 token 文件名（位于 app_data_dir 下）
pub const TOKEN_FILE: &str = "session.token";

/// 建表（`IF NOT EXISTS`，应用启动安全调用；在 db::open 时调用）
pub fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id    INTEGER NOT NULL,
            token      TEXT    NOT NULL UNIQUE,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 当前 Unix 时间戳（秒）
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 生成随机 token（32 字节随机 → 64 位 hex，不可猜测）
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 登录成功时调用：写入一条记住登录记录（默认 3 天），返回 token
///
/// 采用「单用户 token」策略：同一用户新的登录会替换旧的 token（按 user_id 先删后插），
/// 避免多 token 堆积；不影响其他用户（多用户可各留一个记住登录）。
pub fn create_remember_session(conn: &Connection, user_id: i64) -> Result<String, String> {
    let token = generate_token();
    let now = now_secs();
    let expires = now + REMEMBER_TTL_SECS;
    // 同一用户先清旧 token，再插新 token（单用户单 token，保持磁盘文件与 DB 一致）
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user_id])
        .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO sessions (user_id, token, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![user_id, token, now, expires],
    )
    .map_err(|e| e.to_string())?;
    Ok(token)
}

/// 启动时调用：校验 token 是否有效（存在且未过期），返回 user_id
pub fn validate_remember_session(conn: &Connection, token: &str) -> Result<Option<i64>, String> {
    if token.trim().is_empty() {
        return Ok(None);
    }
    let now = now_secs();
    let user_id: Option<i64> = conn
        .query_row(
            "SELECT user_id FROM sessions WHERE token = ?1 AND expires_at > ?2",
            params![token, now],
            |r| r.get(0),
        )
        .map(Some)
        .unwrap_or(None);
    // 已过期/不存在 → 顺手清理脏行与磁盘文件由调用方决定（返回 None 即可）
    Ok(user_id)
}

/// 登出时调用：按 token 删除记住登录记录
pub fn clear_remember_session(conn: &Connection, token: &str) -> Result<(), String> {
    conn.execute("DELETE FROM sessions WHERE token = ?1", params![token])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 将 token 写入磁盘文件（app_data_dir/session.token）
pub fn write_token_file(dir: &Path, token: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("无法创建会话目录: {e}"))?;
    std::fs::write(dir.join(TOKEN_FILE), token).map_err(|e| format!("无法写入会话 token: {e}"))
}

/// 读取磁盘 token 文件（不存在/失败 → None）
pub fn read_token_file(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(TOKEN_FILE))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 删除磁盘 token 文件（登出 / 失效时）
pub fn clear_token_file(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(TOKEN_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn create_validate_clear_flow() {
        let conn = mem_conn();
        // 生成 token 并落库
        let token = create_remember_session(&conn, 1).unwrap();
        assert_eq!(token.len(), 64, "32 字节随机 → 64 hex");
        // 有效期内可校验回 user_id
        assert_eq!(validate_remember_session(&conn, &token).unwrap(), Some(1));
        // 清空后不可校验
        clear_remember_session(&conn, &token).unwrap();
        assert_eq!(validate_remember_session(&conn, &token).unwrap(), None);
        // 空 token → None
        assert_eq!(validate_remember_session(&conn, "").unwrap(), None);
    }

    #[test]
    fn single_token_per_user_and_isolation() {
        let conn = mem_conn();
        let t1 = create_remember_session(&conn, 1).unwrap();
        let t2 = create_remember_session(&conn, 1).unwrap();
        assert_ne!(t1, t2, "每次登录 token 应不同");
        // 同一用户新登录 → 旧 token 失效，新 token 有效
        assert_eq!(validate_remember_session(&conn, &t1).unwrap(), None, "旧 token 应被替换");
        assert_eq!(validate_remember_session(&conn, &t2).unwrap(), Some(1));
        // 多用户隔离：用户 2 的 token 不影响用户 1
        let t3 = create_remember_session(&conn, 2).unwrap();
        assert_eq!(validate_remember_session(&conn, &t3).unwrap(), Some(2));
        assert_eq!(validate_remember_session(&conn, &t2).unwrap(), Some(1), "用户1的 token 不受影响");
    }

    #[test]
    fn token_file_roundtrip() {
        let dir = std::env::temp_dir().join("pm_session_test");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read_token_file(&dir).is_none());
        write_token_file(&dir, "abc123").unwrap();
        assert_eq!(read_token_file(&dir).as_deref(), Some("abc123"));
        clear_token_file(&dir);
        assert!(read_token_file(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
