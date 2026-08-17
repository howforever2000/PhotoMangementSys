//! 用户认证模块（多用户登录）
//!
//! 职责（对应需求：同一个 app 多用户注册/登录、忘记密码重置、相册空间隔离）：
//! - `users` 表：账户名 / 邮箱 / 手机号（三者均唯一）+ 密码哈希（Argon2id）
//! - 注册：账户名 + 邮箱 + 手机号 + 密码 + 密码确认
//! - 登录：账户名 / 邮箱 / 手机号 任一 + 密码
//! - 忘记密码：账户名 + 邮箱 + 手机号 三合一校验通过 → 重设密码
//!
//! 安全约定：
//! - 密码不落明文，仅存 Argon2id 哈希（每次随机盐）
//! - 序列化给前端的 `User` 永不包含 `password_hash`
//! - 登录失败统一提示「账户名/邮箱/手机号或密码错误」，避免账户枚举

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// 升级迁移时创建的内置管理员账户（仅当数据库存在无主的旧相册/分组数据时）
///
/// 多用户功能上线前已有数据没有归属用户，迁移时自动创建该账户接管旧数据，
/// 保证升级后旧相册不丢失。凭据固定为 admin / admin123（本地单机应用）。
pub const DEFAULT_ADMIN_USERNAME: &str = "admin";
pub const DEFAULT_ADMIN_PASSWORD: &str = "admin123";
pub const DEFAULT_ADMIN_EMAIL: &str = "admin@local.dev";
pub const DEFAULT_ADMIN_PHONE: &str = "13800000000";

/// 用户实体（序列化给前端，不含 password_hash）
#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: i64,
    /// 账户名（唯一）
    pub username: String,
    /// 邮箱（唯一，小写存储）
    pub email: String,
    /// 手机号（唯一）
    pub phone: String,
    /// 注册时间戳（Unix 秒）
    pub created_at: i64,
}

/// 带密码哈希的用户记录（仅内部使用，不出库）
#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub phone: String,
    pub password_hash: String,
    pub created_at: i64,
}

/// 注册输入（需求：账户名、邮箱、手机号、密码、密码确认）
#[derive(Debug, Deserialize)]
pub struct RegisterInput {
    pub username: String,
    pub email: String,
    pub phone: String,
    pub password: String,
    /// 密码确认（必须与 password 一致）
    pub confirm_password: String,
}

/// 登录输入：account 为账户名 / 邮箱 / 手机号 任一
#[derive(Debug, Deserialize)]
pub struct LoginInput {
    pub account: String,
    pub password: String,
}

/// 忘记密码重置输入（需求：填手机号、账户名、邮箱校验通过后重设密码）
#[derive(Debug, Deserialize)]
pub struct ResetPasswordInput {
    pub username: String,
    pub email: String,
    pub phone: String,
    pub new_password: String,
    pub confirm_password: String,
}

/// 校验账户名：2-30 字符，仅允许字母 / 数字 / 下划线 / 中文
pub fn validate_username(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    let len = name.chars().count();
    if !(2..=30).contains(&len) {
        return Err("账户名长度需为 2-30 个字符".into());
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || ('\u{4e00}'..='\u{9fa5}').contains(&c));
    if !ok {
        return Err("账户名只能包含字母、数字、下划线或中文".into());
    }
    Ok(name.to_string())
}

/// 校验邮箱格式（简单校验：local@domain.tld），统一转小写存储
pub fn validate_email(raw: &str) -> Result<String, String> {
    let email = raw.trim().to_lowercase();
    if email.len() > 100 {
        return Err("邮箱长度不能超过 100 个字符".into());
    }
    let Some(at_pos) = email.find('@') else {
        return Err("邮箱格式不正确".into());
    };
    if at_pos == 0 || at_pos == email.len() - 1 {
        return Err("邮箱格式不正确".into());
    }
    let domain = &email[at_pos + 1..];
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return Err("邮箱格式不正确".into());
    }
    Ok(email)
}

/// 校验手机号：中国大陆 11 位手机号（1[3-9]xxxxxxxxx），允许中间带空格/短横线
pub fn validate_phone(raw: &str) -> Result<String, String> {
    let digits: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    let valid = digits.len() == 11
        && digits.starts_with('1')
        && matches!(digits.as_bytes().get(1), Some(b'3'..=b'9'));
    if !valid {
        return Err("手机号格式不正确（需为 11 位大陆手机号）".into());
    }
    Ok(digits)
}

/// 校验密码强度：6-64 字符
pub fn validate_password(raw: &str) -> Result<(), String> {
    let len = raw.chars().count();
    if !(6..=64).contains(&len) {
        return Err("密码长度需为 6-64 个字符".into());
    }
    Ok(())
}

/// Argon2id 哈希密码（每次生成随机盐，返回 PHC 字符串）
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("密码哈希失败: {e}"))
}

/// 校验明文密码与存储哈希是否匹配
pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// 当前 Unix 时间戳（秒）
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 从一行记录构造 UserRecord
fn row_to_user_record(row: &rusqlite::Row) -> rusqlite::Result<UserRecord> {
    Ok(UserRecord {
        id: row.get(0)?,
        username: row.get(1)?,
        email: row.get(2)?,
        phone: row.get(3)?,
        password_hash: row.get(4)?,
        created_at: row.get(5)?,
    })
}

/// 注册新用户（校验格式 → 校验唯一性 → 哈希入库），返回不含哈希的用户
pub fn register_user(conn: &Connection, input: RegisterInput) -> Result<User, String> {
    let username = validate_username(&input.username)?;
    let email = validate_email(&input.email)?;
    let phone = validate_phone(&input.phone)?;
    validate_password(&input.password)?;
    if input.password != input.confirm_password {
        return Err("两次输入的密码不一致".into());
    }

    // 唯一性检查（本地单机应用，忽略并发注册竞争）
    let checks = [
        ("username", username.as_str(), "该账户名已被注册"),
        ("email", email.as_str(), "该邮箱已被注册"),
        ("phone", phone.as_str(), "该手机号已被注册"),
    ];
    for (col, val, msg) in checks {
        let exists: bool = conn
            .query_row(
                &format!("SELECT COUNT(*) > 0 FROM users WHERE {col} = ?1"),
                params![val],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists {
            return Err(msg.into());
        }
    }

    let hash = hash_password(&input.password)?;
    let now = now_secs();
    conn.execute(
        "INSERT INTO users (username, email, phone, password_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![username, email, phone, hash, now],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    Ok(User {
        id,
        username,
        email,
        phone,
        created_at: now,
    })
}

/// 按账户名 / 邮箱 / 手机号 任一查找用户（带哈希，内部用）
pub fn find_user_by_account(conn: &Connection, account: &str) -> Result<Option<UserRecord>, String> {
    let account = account.trim();
    if account.is_empty() {
        return Ok(None);
    }
    let result = conn.query_row(
        "SELECT id, username, email, phone, password_hash, created_at
         FROM users
         WHERE username = ?1 OR email = ?1 COLLATE NOCASE OR phone = ?1",
        params![account],
        row_to_user_record,
    );
    match result {
        Ok(u) => Ok(Some(u)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// 按 id 查找用户（登录会话恢复用），返回不含哈希的用户
pub fn find_user_by_id(conn: &Connection, id: i64) -> Result<Option<User>, String> {
    let result = conn.query_row(
        "SELECT id, username, email, phone, password_hash, created_at
         FROM users WHERE id = ?1",
        params![id],
        row_to_user_record,
    );
    match result {
        Ok(u) => Ok(Some(User {
            id: u.id,
            username: u.username,
            email: u.email,
            phone: u.phone,
            created_at: u.created_at,
        })),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// 校验登录凭据：account（账户名/邮箱/手机号）+ 密码
///
/// 失败统一返回「账户名/邮箱/手机号或密码错误」，不区分账户是否存在，避免枚举。
pub fn verify_login(conn: &Connection, account: &str, password: &str) -> Result<User, String> {
    let user = find_user_by_account(conn, account)?
        .ok_or_else(|| "账户名/邮箱/手机号或密码错误".to_string())?;
    if !verify_password(&user.password_hash, password) {
        return Err("账户名/邮箱/手机号或密码错误".into());
    }
    Ok(User {
        id: user.id,
        username: user.username,
        email: user.email,
        phone: user.phone,
        created_at: user.created_at,
    })
}

/// 忘记密码重置：账户名 + 邮箱 + 手机号 全部匹配同一用户 → 更新密码哈希
///
/// 三者任一不匹配（或用户不存在）统一提示校验未通过。
pub fn reset_password(conn: &Connection, input: ResetPasswordInput) -> Result<(), String> {
    let username = validate_username(&input.username)?;
    let email = validate_email(&input.email)?;
    let phone = validate_phone(&input.phone)?;
    validate_password(&input.new_password)?;
    if input.new_password != input.confirm_password {
        return Err("两次输入的新密码不一致".into());
    }

    let user = conn
        .query_row(
            "SELECT id, username, email, phone, password_hash, created_at
             FROM users WHERE username = ?1",
            params![username],
            row_to_user_record,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                "账户名、邮箱、手机号校验未通过".to_string()
            }
            other => other.to_string(),
        })?;
    if user.email != email || user.phone != phone {
        return Err("账户名、邮箱、手机号校验未通过".into());
    }

    let hash = hash_password(&input.new_password)?;
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![hash, user.id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存库建 users 表（与 db::init_schema 保持一致，供 auth 函数测试）
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                username      TEXT    NOT NULL UNIQUE,
                email         TEXT    NOT NULL UNIQUE,
                phone         TEXT    NOT NULL UNIQUE,
                password_hash TEXT    NOT NULL,
                created_at    INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    fn reg_input(username: &str) -> RegisterInput {
        RegisterInput {
            username: username.into(),
            email: format!("{username}@test.com"),
            phone: "13812345678".into(),
            password: "pass123456".into(),
            confirm_password: "pass123456".into(),
        }
    }

    /// 密码哈希：可验证、每次盐不同
    #[test]
    fn password_hash_roundtrip() {
        let h1 = hash_password("secret123").unwrap();
        let h2 = hash_password("secret123").unwrap();
        assert_ne!(h1, h2, "随机盐应使两次哈希不同");
        assert!(verify_password(&h1, "secret123"));
        assert!(!verify_password(&h1, "wrong"));
        assert!(!verify_password("not-a-hash", "secret123"));
    }

    /// 校验规则
    #[test]
    fn validation_rules() {
        assert!(validate_username("张三_01").is_ok());
        assert!(validate_username("abc123").is_ok());
        assert!(validate_username("a").is_err(), "过短");
        assert!(validate_username("a b").is_err(), "含空格");
        assert!(validate_username("名字".repeat(16).as_str()).is_err(), "过长");

        assert!(validate_email("a@b.com").is_ok());
        assert!(validate_email("ABC@x.cn").is_ok());
        assert!(validate_email("no-at").is_err());
        assert!(validate_email("@b.com").is_err());
        assert!(validate_email("a@b").is_err(), "域名无点");

        assert!(validate_phone("13812345678").is_ok());
        assert!(validate_phone("138 1234 5678").is_ok(), "允许空格");
        assert!(validate_phone("12812345678").is_err(), "第二位非法");
        assert!(validate_phone("12345678901").is_err(), "非 1[3-9] 开头");
        assert!(validate_phone("1381234567").is_err(), "位数不足");

        assert!(validate_password("123456").is_ok());
        assert!(validate_password("12345").is_err());
        assert!(validate_password(&"x".repeat(65)).is_err(), "过长");
    }

    /// 注册 → 登录 → 忘记密码重置 全流程
    #[test]
    fn register_login_reset_flow() {
        let conn = test_conn();

        // 注册
        let user = register_user(&conn, reg_input("小明")).unwrap();
        assert_eq!(user.username, "小明");
        assert_eq!(user.email, "小明@test.com");
        assert_eq!(user.phone, "13812345678");

        // 重复账户名 / 邮箱 / 手机号均报错
        assert!(register_user(&conn, reg_input("小明")).is_err(), "重复账户名");
        let mut dup_email = reg_input("小红");
        dup_email.email = "小明@test.com".into();
        assert!(register_user(&conn, dup_email).is_err(), "重复邮箱");
        let mut dup_phone = reg_input("小红");
        dup_phone.phone = "13812345678".into();
        assert!(register_user(&conn, dup_phone).is_err(), "重复手机号");

        // 密码确认不一致
        let mut bad = reg_input("小红");
        bad.confirm_password = "different".into();
        assert!(register_user(&conn, bad).is_err());

        // 登录：账户名 / 邮箱 / 手机号 任一均可
        let by_name = verify_login(&conn, "小明", "pass123456").unwrap();
        assert_eq!(by_name.id, user.id);
        let by_email = verify_login(&conn, "小明@test.com", "pass123456").unwrap();
        assert_eq!(by_email.id, user.id);
        let by_phone = verify_login(&conn, "13812345678", "pass123456").unwrap();
        assert_eq!(by_phone.id, user.id);
        // 错误密码 / 不存在账户统一报错（防枚举）
        assert!(verify_login(&conn, "小明", "wrong").is_err());
        assert!(verify_login(&conn, "不存在", "pass123456").is_err());

        // 忘记密码：三者匹配才可重置
        assert!(reset_password(&conn, ResetPasswordInput {
            username: "小明".into(),
            email: "wrong@test.com".into(),
            phone: "13812345678".into(),
            new_password: "newpass888".into(),
            confirm_password: "newpass888".into(),
        }).is_err(), "邮箱不匹配");
        reset_password(&conn, ResetPasswordInput {
            username: "小明".into(),
            email: "小明@test.com".into(),
            phone: "13812345678".into(),
            new_password: "newpass888".into(),
            confirm_password: "newpass888".into(),
        }).unwrap();
        // 旧密码失效，新密码可登录
        assert!(verify_login(&conn, "小明", "pass123456").is_err());
        assert!(verify_login(&conn, "小明", "newpass888").is_ok());
    }
}
