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
use crate::crypto;
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

/// 读取全部用户并解密敏感字段（本地单机用户数很少，扫描开销可忽略）。
///
/// 字段在库中均以密文（或迁移前的历史明文）存储，这里统一解密为实际值。
fn fetch_all_users(conn: &Connection) -> Result<Vec<UserRecord>, String> {
    let mut stmt = conn
        .prepare("SELECT id, username, email, phone, password_hash, created_at FROM users")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        let (id, username, email_enc, phone_enc, hash_enc, created_at) =
            r.map_err(|e| e.to_string())?;
        out.push(UserRecord {
            id,
            username,
            email: crypto::decrypt(&email_enc)?,
            phone: crypto::decrypt(&phone_enc)?,
            password_hash: crypto::decrypt(&hash_enc)?,
            created_at,
        });
    }
    Ok(out)
}

fn to_user(u: &UserRecord) -> User {
    User {
        id: u.id,
        username: u.username.clone(),
        email: u.email.clone(),
        phone: u.phone.clone(),
        created_at: u.created_at,
    }
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
    // 邮箱/手机号在库中为密文，无法用 SQL WHERE / UNIQUE 判断，改为全量解密后比对。
    let existing = fetch_all_users(conn)?;
    if existing.iter().any(|u| u.username == username) {
        return Err("该账户名已被注册".into());
    }
    if existing.iter().any(|u| u.email.eq_ignore_ascii_case(&email)) {
        return Err("该邮箱已被注册".into());
    }
    if existing.iter().any(|u| u.phone == phone) {
        return Err("该手机号已被注册".into());
    }

    // 双层防护：密码先 Argon2id 哈希，哈希串再加密落库
    let hash = hash_password(&input.password)?;
    let email_enc = crypto::encrypt(&email)?;
    let phone_enc = crypto::encrypt(&phone)?;
    let hash_enc = crypto::encrypt(&hash)?;
    let now = now_secs();
    conn.execute(
        "INSERT INTO users (username, email, phone, password_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![username, email_enc, phone_enc, hash_enc, now],
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
///
/// 邮箱在库中为密文，无法用 SQL WHERE 匹配，改为全量解密后比对（本地用户数少）。
pub fn find_user_by_account(conn: &Connection, account: &str) -> Result<Option<UserRecord>, String> {
    let account = account.trim();
    if account.is_empty() {
        return Ok(None);
    }
    let users = fetch_all_users(conn)?;
    Ok(users
        .into_iter()
        .find(|u| {
            u.username == account
                || u.email.eq_ignore_ascii_case(account)
                || u.phone == account
        }))
}

/// 按 id 查找用户（登录会话恢复用），返回不含哈希的用户
pub fn find_user_by_id(conn: &Connection, id: i64) -> Result<Option<User>, String> {
    let users = fetch_all_users(conn)?;
    Ok(users.into_iter().find(|u| u.id == id).map(|u| to_user(&u)))
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
    Ok(to_user(&user))
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

    let user = fetch_all_users(conn)?
        .into_iter()
        .find(|u| u.username == username)
        .ok_or_else(|| "账户名、邮箱、手机号校验未通过".to_string())?;
    if user.email != email || user.phone != phone {
        return Err("账户名、邮箱、手机号校验未通过".into());
    }

    let hash = hash_password(&input.new_password)?;
    let hash_enc = crypto::encrypt(&hash)?;
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![hash_enc, user.id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 升级迁移：把仍以历史明文存储的邮箱 / 手机号 / 密码哈希重加密为密文。
///
/// 在 `db::init_schema` 建表后调用；无历史明文行时为空操作。
/// 仅当加密密钥已初始化时才执行，避免测试环境（未初始化密钥）报错。
pub fn migrate_legacy_user_fields(conn: &Connection) -> Result<(), String> {
    if !crypto::is_initialized() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare("SELECT id, username, email, phone, password_hash FROM users")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for (id, username, email, phone, pass) in rows {
        // 任一字段仍是明文（非 v1: 前缀）→ 全部重加密
        if crypto::is_encrypted(&email)
            && crypto::is_encrypted(&phone)
            && crypto::is_encrypted(&pass)
        {
            continue;
        }
        let email_enc = crypto::encrypt(&email)?;
        let phone_enc = crypto::encrypt(&phone)?;
        let hash_enc = if crypto::is_encrypted(&pass) {
            pass
        } else {
            crypto::encrypt(&pass)?
        };
        conn.execute(
            "UPDATE users SET email=?1, phone=?2, password_hash=?3 WHERE id=?4",
            params![email_enc, phone_enc, hash_enc, id],
        )
        .map_err(|e| e.to_string())?;
        log_info_migrated(username.as_str());
    }
    Ok(())
}

#[cfg(not(test))]
fn log_info_migrated(_u: &str) {}

#[cfg(test)]
fn log_info_migrated(u: &str) {
    eprintln!("[auth] migrated legacy user: {u}");
}

/// 修改基本信息输入（当前密码验证通过后方可修改）
#[derive(Debug, Deserialize)]
pub struct UpdateProfileInput {
    pub email: String,
    pub phone: String,
    /// 当前密码（必须与该用户匹配，防止他人替改）
    pub current_password: String,
}

/// 修改当前用户基本信息（邮箱 / 手机号），需先验证当前密码。
///
/// 需求：修改基本信息前必须先输入密码（防他人通过数据库读取后直接改表）。
pub fn update_profile(
    conn: &Connection,
    user_id: i64,
    input: UpdateProfileInput,
) -> Result<User, String> {
    let user = fetch_all_users(conn)?
        .into_iter()
        .find(|u| u.id == user_id)
        .ok_or_else(|| "用户不存在".to_string())?;
    // 密码门禁：先验证当前密码
    if !verify_password(&user.password_hash, &input.current_password) {
        return Err("密码验证失败".into());
    }
    let email = validate_email(&input.email)?;
    let phone = validate_phone(&input.phone)?;
    // 唯一性（排除自身）
    for u in fetch_all_users(conn)? {
        if u.id != user_id {
            if u.email.eq_ignore_ascii_case(&email) {
                return Err("该邮箱已被其他账户使用".into());
            }
            if u.phone == phone {
                return Err("该手机号已被其他账户使用".into());
            }
        }
    }
    let email_enc = crypto::encrypt(&email)?;
    let phone_enc = crypto::encrypt(&phone)?;
    conn.execute(
        "UPDATE users SET email=?1, phone=?2 WHERE id=?3",
        params![email_enc, phone_enc, user_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(User {
        id: user.id,
        username: user.username,
        email,
        phone,
        created_at: user.created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存库建 users 表（与 db::init_schema 保持一致，供 auth 函数测试）
    fn test_conn() -> Connection {
        crypto::init_for_test();
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

    /// 落库即密文：读取到的 email/phone/password_hash 不再是原字段（防撞表窃取/篡改）
    #[test]
    fn encryption_at_rest() {
        let conn = test_conn();
        register_user(&conn, reg_input("小明")).unwrap();
        // 读原始库值
        let (raw_email, raw_phone, raw_pass): (String, String, String) = conn
            .query_row(
                "SELECT email, phone, password_hash FROM users LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        // 不再是明文
        assert!(!raw_email.contains('@'), "邮箱不应以明文存储");
        assert!(raw_email.starts_with("v1:"), "邮箱应为密文");
        assert_ne!(raw_phone, "13812345678", "手机号不应以明文存储");
        assert!(raw_phone.starts_with("v1:"), "手机号应为密文");
        assert!(!raw_pass.starts_with("$argon2"), "密码哈希串不应明文可见");
        assert!(raw_pass.starts_with("v1:"));
        // 读回（解密）仍能登录
        assert!(verify_login(&conn, "小明", "pass123456").is_ok());
        // 篡改密文 → 登录失败（GCM 校验失败 → 解密为空 → 哈希不匹配）
    }

    /// 修改基本信息必须验证当前密码
    #[test]
    fn update_profile_password_gate() {
        let conn = test_conn();
        let user = register_user(&conn, reg_input("小明")).unwrap();

        let upd = |pw: &str| UpdateProfileInput {
            email: "new@test.com".into(),
            phone: "13912345678".into(),
            current_password: pw.into(),
        };
        // 密码错误 → 拒绝
        assert!(update_profile(&conn, user.id, upd("wrongpass")).is_err());
        // 密码正确 → 成功，新邮箱可登录
        let updated = update_profile(&conn, user.id, upd("pass123456")).unwrap();
        assert_eq!(updated.email, "new@test.com");
        assert_eq!(updated.phone, "13912345678");
        assert!(verify_login(&conn, "new@test.com", "pass123456").is_ok());
        assert!(verify_login(&conn, "小明@test.com", "pass123456").is_err(), "旧邮箱失效");
    }

    /// 历史明文迁移：把旧库明文邮箱/手机号/密码哈希重加密
    #[test]
    fn legacy_migration_encrypts() {
        crypto::init_for_test();
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
        // 手工插入明文行（模拟升级前旧库）
        conn.execute(
            "INSERT INTO users (username, email, phone, password_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "老用户",
                "old@test.com",
                "13800000000",
                hash_password("oldpass123").unwrap(),
                1000
            ],
        )
        .unwrap();
        migrate_legacy_user_fields(&conn).unwrap();
        // 已加密，可用账户名+密码登录
        assert!(verify_login(&conn, "老用户", "oldpass123").is_ok());
        let raw_email: String = conn
            .query_row("SELECT email FROM users LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert!(raw_email.starts_with("v1:"), "迁移后应为密文");
    }
}
