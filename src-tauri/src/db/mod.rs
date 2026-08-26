//! 相册数据持久化层
//!
//! 对应 SpringBoot 中的：
//! - `Album` 结构体  →  `@Entity` 实体类
//! - `CreateAlbumInput` / `UpdateAlbumInput`  →  `@RequestBody DTO`
//! - `Database` 结构体  →  `Repository` + `DataSource`
//! - `init_schema`  →  `schema.sql` 建表脚本
//! - `DbError`  →  自定义业务异常（配合全局异常处理）

pub mod content;
pub use content::{AlbumContentRow, ContentFilters, ContentSearchHit, PhotoContentRecord, SmartHit};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

// =====================================================================
// 实体类（对应 SpringBoot @Entity）
// =====================================================================

/// 相册实体 —— 对应 albums 表的一行记录
///
/// 字段严格遵循《主框架需求分析》§3.1 定义：
/// id / name / path / description / cover_path / created_at / updated_at
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: i64,
    pub name: String,
    /// 绑定的本地文件夹绝对路径（UNIQUE）
    pub path: String,
    /// 相册简介，可空
    pub description: Option<String>,
    /// 封面图片绝对路径，可空
    pub cover_path: Option<String>,
    /// 创建时间戳（Unix 秒）
    pub created_at: i64,
    /// 最后更新时间戳（Unix 秒）
    pub updated_at: i64,
    /// 相册内照片数量（从文件系统统计，非数据库字段）
    #[serde(default)]
    pub photo_count: i64,
    /// 相册拍摄时间（封面照片的 EXIF 拍摄时间，格式 YYYY-MM-DD）
    #[serde(default)]
    pub shoot_time: Option<String>,
    /// 相册文件夹总大小（字节，从文件系统统计，非数据库字段）
    #[serde(default)]
    pub size_bytes: u64,
    /// 相册地点标签（手动设置，数据库字段）
    pub location: Option<String>,
    /// 相册标签（最多 5 个，来自 album_tags 表）
    #[serde(default)]
    pub tags: Vec<String>,
    /// 所属分组 ID（手动排序，非数据库字段，命令层填充）
    #[serde(default)]
    pub folder_id: Option<i64>,
    /// 所属分组完整路径（如 "旅行/欧洲/巴黎"）
    #[serde(default)]
    pub folder_path: String,
    /// 合并来源相册列表（FEAT-A）：
    /// 该相册历史上由哪些相册合并而来，用于封面下显示"由 X 个相册合并而成"。
    /// 不在数据库行里，命令层从 `album_merged_sources` 表填充。
    #[serde(default)]
    pub merged_sources: Vec<MergedSource>,
}

/// 单个合并来源条目（FEAT-A）
///
/// 记录被合并掉的那个相册的 id / name / path，用于相册卡片上展示
/// 「由以下相册合并而来：xxx / yyy / ...」，每条路径可点击跳转。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedSource {
    pub id: i64,
    pub name: String,
    pub path: String,
}

/// 相册搜索结果：相册 + 所属分组路径
#[derive(Debug, Clone, Serialize)]
pub struct AlbumSearchResult {
    pub album: Album,
    /// 所属分组 ID（无则为 None）
    pub folder_id: Option<i64>,
    /// 分组路径（如 "旅行/欧洲/巴黎"），顶级相册为空字符串
    pub folder_path: String,
}

/// 创建相册的输入参数
///
/// id 与时间戳由后端生成，前端无需（也不应）传入
#[derive(Debug, Deserialize)]
pub struct CreateAlbumInput {
    /// 必填，1-100 字符（长度校验在命令层完成）
    pub name: String,
    /// 必填，文件夹绝对路径
    pub path: String,
    /// 选填，最多 500 字符
    pub description: Option<String>,
}

/// 更新相册的输入参数
///
/// 所有业务字段可选，"提供则更新，不提供则保留原值"
#[derive(Debug, Deserialize)]
pub struct UpdateAlbumInput {
    pub id: i64,
    pub name: Option<String>,
    pub description: Option<String>,
    pub cover_path: Option<String>,
    /// 地点标签（可空，空字符串表示清除）
    pub location: Option<String>,
}

/// 相册文件系统统计缓存（对应 album_stats 表）
///
/// 将 photo_count / size_bytes / shoot_time / 封面源图路径持久化缓存，
/// 避免每次列表/详情加载都全目录遍历。
#[derive(Debug, Clone)]
pub struct AlbumStats {
    pub photo_count: i64,
    pub size_bytes: u64,
    pub shoot_time: Option<String>,
    /// 封面源图绝对路径（无封面时用于生成/复用缩略图）
    pub cover_source: Option<String>,
    /// 上次扫描时的目录递归文件总数（变更探测信号）
    ///
    /// 每次加载时轻量统计当前文件数与此值比对：不一致说明目录内容变了，
    /// 才需要全量重扫该相册（替代 TTL 定时失效，消除滞后窗口与全量重扫峰值）
    pub file_count: i64,
}

// =====================================================================
// 错误类型（对应 SpringBoot 自定义异常 + 全局异常处理）
// =====================================================================

/// 数据层错误
///
/// 命令层通过 `.map_err(|e| e.to_string())` 将其转为字符串返回前端，
/// 对应需求 §7.2 的后端错误返回策略
#[derive(Debug, Error)]
pub enum DbError {
    #[error("路径不存在或不是文件夹: {0}")]
    PathNotExist(String),

    #[error("该文件夹已被相册『{0}』使用")]
    PathAlreadyUsed(String),

    #[error("相册不存在: ID {0}")]
    NotFound(i64),

    #[error("数据库错误: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("{0}")]
    Other(String),
}

// =====================================================================
// 数据库（对应 SpringBoot Repository + DataSource）
// =====================================================================

/// SQLite 数据库封装
///
/// 持有一个 `rusqlite::Connection`，提供相册的 CRUD 操作。
/// 通过 `Mutex<Database>` 注册为 Tauri 全局状态后，可被多个命令共享。
pub struct Database {
    conn: Connection,
}

impl Database {
    /// 暴露底层连接，供 folder 等模块复用
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// 打开（或创建）数据库文件并初始化表结构
    ///
    /// `db_path` 通常为 `app_data_dir/photos.db`
    pub fn open(db_path: &Path) -> Result<Self, DbError> {
        // 确保父目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DbError::Other(format!("无法创建数据库目录: {e}")))?;
        }
        let conn = Connection::open(db_path)?;
        // 启用外键约束（为后续 photos / tags 关联表预留）
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    /// 建表 —— 对应 schema.sql
    ///
    /// 使用 `IF NOT EXISTS`，应用每次启动安全调用
    fn init_schema(&self) -> Result<(), DbError> {
        // 用户表（多用户登录：账户名/邮箱/手机号三者唯一，密码只存 Argon2id 哈希）
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                username      TEXT    NOT NULL UNIQUE,
                email         TEXT    NOT NULL UNIQUE,
                phone         TEXT    NOT NULL UNIQUE,
                password_hash TEXT    NOT NULL,
                created_at    INTEGER NOT NULL
            );",
        )?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS albums (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT    NOT NULL,
                path        TEXT    NOT NULL UNIQUE,
                description TEXT,
                cover_path  TEXT,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL,
                location    TEXT,
                folder_id   INTEGER,
                sort_order  INTEGER DEFAULT 0
            );",
        )?;
        // 手动排序分组表（最多三级父子关系）
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS folders (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT    NOT NULL,
                parent_id   INTEGER,
                level       INTEGER NOT NULL DEFAULT 1,
                sort_order  INTEGER DEFAULT 0,
                description TEXT,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL
            );",
        )?;
        // 文件夹标签表（每个文件夹最多 5 个标签）
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS folder_tags (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                folder_id INTEGER NOT NULL,
                tag       TEXT    NOT NULL,
                UNIQUE(folder_id, tag)
            );",
        )?;
        // 相册标签表（每个最小相册最多 5 个标签）
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS album_tags (
                id       INTEGER PRIMARY KEY AUTOINCREMENT,
                album_id INTEGER NOT NULL,
                tag      TEXT    NOT NULL,
                UNIQUE(album_id, tag)
            );",
        )?;
        // 分组-子相册关联表（folders 记录其包含的子相册列表，保证分组归属可靠持久化）
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS folder_albums (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                folder_id INTEGER NOT NULL,
                album_id  INTEGER NOT NULL,
                sort_order INTEGER DEFAULT 0,
                UNIQUE(folder_id, album_id)
            );",
        )?;
        // 相册统计缓存表（photo_count/size_bytes/shoot_time/封面源图路径）
        // 文件系统统计结果缓存，避免每次列表加载都全目录遍历（详见 fill_album_stats）
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS album_stats (
                album_id     INTEGER PRIMARY KEY,
                photo_count  INTEGER NOT NULL DEFAULT 0,
                size_bytes   INTEGER NOT NULL DEFAULT 0,
                shoot_time   TEXT,
                cover_source TEXT,
                scanned_at   INTEGER NOT NULL,
                file_count   INTEGER NOT NULL DEFAULT -1
            );",
        )?;
        // 相册照片排除表（「记录删除」：从相册浏览中移除但保留本地文件）
        // 多用户隔离：user_id 校验；主键 (album_id, path) 防重复排除
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS album_photo_excluded (
                album_id    INTEGER NOT NULL,
                path        TEXT    NOT NULL,
                user_id     INTEGER NOT NULL,
                excluded_at INTEGER NOT NULL,
                PRIMARY KEY(album_id, path)
            );",
        )?;
        // 照片打分（星标）：按 (user_id, path) 唯一，独立于扫描记录，任何照片都可打分
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS photo_ratings (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id    INTEGER NOT NULL,
                path       TEXT    NOT NULL,
                rating     INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL,
                UNIQUE(user_id, path)
            );",
        )?;
        // 合并来源记录（FEAT-A）：
        //   记录「目标相册 id」被合并时合并掉的源相册（id / name / path）。
        //   用于相册卡片下显示"由 X 个相册合并而来"，每条路径可点击跳转。
        //   注意：源相册本身已被 delete_album 删除，但这里保留其历史信息以便展示。
        //   多用户隔离：user_id 跟随目标相册的归属。
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS album_merged_sources (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                album_id   INTEGER NOT NULL,
                source_id  INTEGER NOT NULL,
                source_name TEXT   NOT NULL,
                source_path TEXT   NOT NULL,
                user_id    INTEGER NOT NULL,
                merged_at  INTEGER NOT NULL,
                UNIQUE(album_id, source_id)
            );",
        )?;
        let _ = self.conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_merged_sources_album ON album_merged_sources(album_id);");
        let _ = self.conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_photo_ratings_user ON photo_ratings(user_id);");
        let _ = self.conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_photo_ratings_path ON photo_ratings(path);");
        // 迁移：为旧库 album_stats 补充 file_count 列（变更探测信号，旧行默认 -1 即强制首次重扫）
        let _ = self.conn.execute_batch("ALTER TABLE album_stats ADD COLUMN file_count INTEGER NOT NULL DEFAULT -1;");
        // 迁移：为旧库补充 albums 新列（若已存在则忽略）
        let _ = self.conn.execute_batch("ALTER TABLE albums ADD COLUMN location TEXT;");
        let _ = self.conn.execute_batch("ALTER TABLE albums ADD COLUMN folder_id INTEGER;");
        let _ = self.conn.execute_batch("ALTER TABLE albums ADD COLUMN sort_order INTEGER DEFAULT 0;");
        // 迁移：folder_albums 数据回填（12ccfb2 改进）
        // 自 folder_albums 成为分组归属唯一事实源后，读取不再依赖 albums.folder_id。
        // 但历史数据库可能存在「仅在 albums.folder_id 记录归属、folder_albums 无关联行」
        // 的相册（早期版本 create_album / 部分写入路径只更新 albums 表），若不同步回填，
        // 这些相册会在升级后静默丢失分组显示。
        // 只补充「albums.folder_id 有值且 folder_albums 尚无该相册任何关联」的相册，
        // 不触碰已有关联（含历史孤儿行），最保守地保证旧数据不丢失。
        self.conn.execute_batch(
            "INSERT INTO folder_albums (folder_id, album_id, sort_order)\n\
             SELECT a.folder_id, a.id, COALESCE(a.sort_order, 0)\n\
             FROM albums a\n\
             WHERE a.folder_id IS NOT NULL\n\
               AND NOT EXISTS (SELECT 1 FROM folder_albums fa WHERE fa.album_id = a.id);",
        )?;
        // 迁移：多用户隔离 —— 为旧库 albums / folders 补充 user_id 列（若已存在则忽略）
        let _ = self.conn.execute_batch("ALTER TABLE albums ADD COLUMN user_id INTEGER;");
        let _ = self.conn.execute_batch("ALTER TABLE folders ADD COLUMN user_id INTEGER;");
        // 旧数据归属：存在无主（user_id IS NULL）相册/分组时，自动创建内置管理员账户接管，
        // 保证多用户功能上线后旧数据仍可访问（凭据见 auth::DEFAULT_ADMIN_*，仅迁移时创建）。
        let orphan_count: i64 = self
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM albums WHERE user_id IS NULL)\n\
                 + (SELECT COUNT(*) FROM folders WHERE user_id IS NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if orphan_count > 0 {
            let admin_exists: bool = self
                .conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM users WHERE username = ?1",
                    params![crate::auth::DEFAULT_ADMIN_USERNAME],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !admin_exists {
                let hash = crate::auth::hash_password(crate::auth::DEFAULT_ADMIN_PASSWORD)
                    .map_err(|e| DbError::Other(e))?;
                // 落库前加密（邮箱/手机号/密码哈希不以原字段存在）
                let email_enc = crate::crypto::encrypt(crate::auth::DEFAULT_ADMIN_EMAIL)
                    .map_err(|e| DbError::Other(e))?;
                let phone_enc = crate::crypto::encrypt(crate::auth::DEFAULT_ADMIN_PHONE)
                    .map_err(|e| DbError::Other(e))?;
                let hash_enc = crate::crypto::encrypt(&hash).map_err(|e| DbError::Other(e))?;
                let now = Self::now_secs();
                self.conn.execute(
                    "INSERT INTO users (username, email, phone, password_hash, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        crate::auth::DEFAULT_ADMIN_USERNAME,
                        email_enc,
                        phone_enc,
                        hash_enc,
                        now
                    ],
                )?;
            }
            let admin_id: i64 = self
                .conn
                .query_row(
                    "SELECT id FROM users WHERE username = ?1",
                    params![crate::auth::DEFAULT_ADMIN_USERNAME],
                    |r| r.get(0),
                )
                .map_err(|e| DbError::Sqlite(e))?;
            self.conn.execute(
                "UPDATE albums SET user_id = ?1 WHERE user_id IS NULL",
                params![admin_id],
            )?;
            self.conn.execute(
                "UPDATE folders SET user_id = ?1 WHERE user_id IS NULL",
                params![admin_id],
            )?;
        }
        // 内容扫描表（FEAT-022：AI 内容扫描入库 + 照片智能搜索）
        self.init_content_schema()?;
        // 迁移：将历史以明文存储的用户邮箱/手机号/密码哈希重加密（无历史明文则为空操作）
        let _ = crate::auth::migrate_legacy_user_fields(self.conn());
        Ok(())
    }

    /// 当前 Unix 时间戳（秒）
    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// 从一行记录构造 Album
    ///
    /// 列顺序: id, name, path, description, cover_path, created_at, updated_at, location
    fn row_to_album(row: &rusqlite::Row) -> rusqlite::Result<Album> {
        Ok(Album {
            id: row.get(0)?,
            name: row.get(1)?,
            path: row.get(2)?,
            description: row.get(3)?,
            cover_path: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            location: row.get(7)?,
            photo_count: 0,
            shoot_time: None,
            size_bytes: 0,
            tags: Vec::new(),
            folder_id: None,
            folder_path: String::new(),
            merged_sources: Vec::new(),
        })
    }

    // ---------------- CRUD ----------------

    /// 创建相册
    ///
    /// 1. 校验路径是否存在（需求 §2.2 路径合法性校验）
    /// 2. 插入记录（归属当前登录用户 user_id），捕获 UNIQUE 冲突并返回占用该路径的
    ///    相册名（需求 §7.2）
    pub fn create_album(&self, input: CreateAlbumInput, user_id: i64) -> Result<Album, DbError> {
        if !Path::new(&input.path).is_dir() {
            return Err(DbError::PathNotExist(input.path));
        }
        let now = Self::now_secs();
        let result = self.conn.execute(
            "INSERT INTO albums (name, path, description, cover_path, created_at, updated_at, user_id)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
            params![input.name, input.path, input.description, now, now, user_id],
        );
        match result {
            Ok(_) => {
                let id = self.conn.last_insert_rowid();
                self.get_album(id, user_id)
            }
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                // UNIQUE 冲突：查出占用该路径的相册名，返回友好提示
                let conflict_name: String = self
                    .conn
                    .query_row(
                        "SELECT name FROM albums WHERE path = ?1",
                        params![input.path],
                        |row| row.get(0),
                    )
                    .unwrap_or_else(|_| "未知".to_string());
                Err(DbError::PathAlreadyUsed(conflict_name))
            }
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// 根据路径查找相册（不限用户）
    ///
    /// 用于批量导入时检查某路径是否已被任一用户的相册占用。
    /// 出现于多用户场景下：旧数据被 admin 接管后，新用户再导入同路径会撞全局 UNIQUE。
    /// 查得表示 path 已被其他用户占用，需提示为「已存在」友好跳过场景。
    pub fn find_any_album_by_path(&self, path: &str) -> Result<Option<Album>, DbError> {
        let result = self.conn.query_row(
            "SELECT id, name, path, description, cover_path, created_at, updated_at, location
             FROM albums
             WHERE path = ?1
             LIMIT 1",
            params![path],
            Self::row_to_album,
        );
        match result {
            Ok(album) => Ok(Some(album)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// 根据路径查找相册（不存在返回 None）
    ///
    /// 用于批量导入时检查某个文件夹是否已作为相册存在，避免重复创建。
    /// 限定在当前登录用户的相册空间内查询。
    pub fn find_album_by_path(&self, path: &str, user_id: i64) -> Result<Option<Album>, DbError> {
        let result = self.conn.query_row(
            "SELECT id, name, path, description, cover_path, created_at, updated_at, location
             FROM albums
             WHERE path = ?1 AND user_id = ?2",
            params![path, user_id],
            Self::row_to_album,
        );
        match result {
            Ok(album) => Ok(Some(album)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// 获取当前用户的全部相册，按 updated_at 降序（需求 §4.2 get_albums）
    ///
    /// 多用户隔离：仅返回归属 `user_id` 的相册。
    pub fn get_albums(&self, user_id: i64) -> Result<Vec<Album>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, path, description, cover_path, created_at, updated_at, location
             FROM albums
             WHERE user_id = ?1
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![user_id], Self::row_to_album)?;
        let mut albums: Vec<Album> = rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)?;
        self.load_album_tags(&mut albums)?;
        self.fill_album_folder(&mut albums, user_id)?;
        self.load_merged_sources(&mut albums)?;
        Ok(albums)
    }

    /// 填充相册的分组信息（folder_id + folder_path）
    ///
    /// **唯一事实源是 `folder_albums` 关联表**，`albums.folder_id` 仅是事务内同步的
    /// 冗余缓存列，读取不依赖它。一次查询批量填充，避免 N+1 与四层兜底。
    ///
    /// 多用户隔离：分组归属只读取当前用户的分组（albums 本身已按用户过滤）。
    fn fill_album_folder(&self, albums: &mut [Album], user_id: i64) -> Result<(), DbError> {
        // 构建当前用户的分组路径映射
        let folder_path_map = self.build_folder_paths_for_user(user_id)?;
        // 从 folder_albums 一次读取当前用户所有相册的分组归属
        let mut stmt = self.conn.prepare(
            "SELECT fa.album_id, fa.folder_id
             FROM folder_albums fa
             JOIN folders f ON f.id = fa.folder_id
             WHERE f.user_id = ?1",
        )?;
        let rows = stmt.query_map(params![user_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        let mut folder_map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
        for row in rows {
            let (aid, fid) = row?;
            folder_map.insert(aid, fid);
        }
        for album in albums.iter_mut() {
            match folder_map.get(&album.id) {
                Some(&fid) => {
                    album.folder_id = Some(fid);
                    album.folder_path = folder_path_map.get(&fid).cloned().unwrap_or_default();
                }
                None => {
                    album.folder_id = None;
                    album.folder_path = String::new();
                }
            }
        }
        Ok(())
    }

    /// 为一批相册加载标签
    fn load_album_tags(&self, albums: &mut [Album]) -> Result<(), DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT album_id, tag FROM album_tags ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut tag_map: std::collections::HashMap<i64, Vec<String>> =
            std::collections::HashMap::new();
        for row in rows {
            let (aid, tag) = row?;
            tag_map.entry(aid).or_default().push(tag);
        }
        for album in albums.iter_mut() {
            if let Some(t) = tag_map.get(&album.id) {
                album.tags = t.clone();
            }
        }
        Ok(())
    }

    /// 填充相册的合并来源列表（FEAT-A）
    ///
    /// 从 `album_merged_sources` 表加载每个相册的合并历史，
    /// 用于卡片上显示「由 X 个相册合并而来」，每条路径可点击跳转。
    /// 一次 SELECT 批量填充，避免 N+1。
    fn load_merged_sources(&self, albums: &mut [Album]) -> Result<(), DbError> {
        if albums.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = albums.iter().map(|a| a.id).collect();
        // 构造占位符 ?,?,?...
        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT album_id, source_id, source_name, source_path
             FROM album_merged_sources
             WHERE album_id IN ({})
             ORDER BY merged_at ASC",
            placeholders
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params_vec: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params_vec.as_slice(), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut map: std::collections::HashMap<i64, Vec<MergedSource>> =
            std::collections::HashMap::new();
        for row in rows {
            let (aid, sid, name, path) = row?;
            map.entry(aid).or_default().push(MergedSource { id: sid, name, path });
        }
        for album in albums.iter_mut() {
            if let Some(s) = map.get(&album.id) {
                album.merged_sources = s.clone();
            }
        }
        Ok(())
    }

    /// 按关键词搜索相册
    ///
    /// 搜索范围覆盖（需求：不只搜最小相册，也搜相册集/父相册）：
    /// - 相册名称
    /// - 相册标签（album_tags）
    /// - 相册所属分组（及其祖先分组）的名称
    /// - 相册所属分组（及其祖先分组）的标签（folder_tags）
    ///
    /// 多用户隔离：全部查询限定在当前登录用户的相册空间内。
    /// 返回匹配相册及其所属分组路径。
    pub fn search_albums(&self, keyword: &str, user_id: i64) -> Result<Vec<AlbumSearchResult>, DbError> {
        let kw = format!("%{}%", keyword.trim());

        // 收集所有能匹配关键词的相册 id 集合（去重）
        let mut matched_ids: Vec<i64> = Vec::new();
        // 1. 相册名匹配
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM albums WHERE name LIKE ?1 AND user_id = ?2")?;
            let rows = stmt.query_map(params![kw, user_id], |r| r.get::<_, i64>(0))?;
            for row in rows {
                matched_ids.push(row?);
            }
        }
        // 2. 相册标签匹配
        {
            let mut stmt = self.conn.prepare(
                "SELECT at.album_id
                 FROM album_tags at
                 JOIN albums a ON a.id = at.album_id
                 WHERE at.tag LIKE ?1 AND a.user_id = ?2",
            )?;
            let rows = stmt.query_map(params![kw, user_id], |r| r.get::<_, i64>(0))?;
            for row in rows {
                matched_ids.push(row?);
            }
        }
        // 3. 分组名/分组标签匹配 → 命中分组的相册
        //    分组归属以 folder_albums 关联表为唯一事实源
        {
            let mut stmt = self.conn.prepare(
                "SELECT fa.album_id
                 FROM folder_albums fa
                 JOIN folders f ON f.id = fa.folder_id
                 WHERE f.user_id = ?2
                   AND (f.name LIKE ?1
                    OR EXISTS (
                        SELECT 1 FROM folder_tags ft WHERE ft.folder_id = f.id AND ft.tag LIKE ?1
                    ))",
            )?;
            let rows = stmt.query_map(params![kw, user_id], |r| r.get::<_, i64>(0))?;
            for row in rows {
                matched_ids.push(row?);
            }
        }
        // 4. 祖先分组名/标签匹配（相册分组的祖先链）
        {
            // 获取当前用户的所有分组归属，逐一检查祖先链
            let mut stmt = self.conn.prepare(
                "SELECT fa.album_id, fa.folder_id
                 FROM folder_albums fa
                 JOIN folders f ON f.id = fa.folder_id
                 WHERE f.user_id = ?1",
            )?;
            let rows = stmt.query_map(params![user_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
            })?;
            // 构建当前用户的分组名/标签查找
            let mut folder_name_stmt =
                self.conn.prepare("SELECT id, name FROM folders WHERE user_id = ?1")?;
            let folder_name_map: std::collections::HashMap<i64, String> = folder_name_stmt
                .query_map(params![user_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<_, _>>()?;
            let mut folder_parent_stmt =
                self.conn.prepare("SELECT id, parent_id FROM folders WHERE user_id = ?1")?;
            let folder_parent_map: std::collections::HashMap<i64, Option<i64>> =
                folder_parent_stmt
                    .query_map(params![user_id], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
                    })?
                    .collect::<Result<_, _>>()?;
            let folder_tag_map: std::collections::HashMap<i64, Vec<String>> = {
                let mut map: std::collections::HashMap<i64, Vec<String>> =
                    std::collections::HashMap::new();
                let mut stmt2 = self.conn.prepare(
                    "SELECT ft.folder_id, ft.tag
                     FROM folder_tags ft
                     JOIN folders f ON f.id = ft.folder_id
                     WHERE f.user_id = ?1",
                )?;
                let tag_rows = stmt2.query_map(params![user_id], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?;
                for tr in tag_rows {
                    let (fid, tag) = tr?;
                    map.entry(fid).or_default().push(tag);
                }
                map
            };

            let kw_lower = keyword.trim().to_lowercase();
            // 检查某分组或其祖先是否匹配
            let mut folder_matches = std::collections::HashSet::new();
            for (fid, _) in &folder_name_map {
                // 遍历祖先链
                let mut cur = Some(*fid);
                while let Some(cid) = cur {
                    if let Some(name) = folder_name_map.get(&cid) {
                        if name.to_lowercase().contains(&kw_lower) {
                            folder_matches.insert(*fid);
                            break;
                        }
                    }
                    if let Some(tags) = folder_tag_map.get(&cid) {
                        if tags.iter().any(|t| t.to_lowercase().contains(&kw_lower)) {
                            folder_matches.insert(*fid);
                            break;
                        }
                    }
                    cur = folder_parent_map.get(&cid).copied().flatten();
                }
            }

            for row in rows {
                let (album_id, folder_id) = row?;
                if let Some(fid) = folder_id {
                    if folder_matches.contains(&fid) {
                        matched_ids.push(album_id);
                    }
                }
            }
        }

        // 去重并查询完整相册
        matched_ids.sort_unstable();
        matched_ids.dedup();
        let mut results = Vec::new();
        if !matched_ids.is_empty() {
            // 构建当前用户的文件夹路径映射
            let folder_path_map = self.build_folder_paths_for_user(user_id)?;
            for id in matched_ids {
                if let Ok(album) = self.get_album(id, user_id) {
                    // 获取 folder_id（唯一事实源：folder_albums，限定当前用户）
                    let folder_id: Option<i64> = self
                        .conn
                        .query_row(
                            "SELECT fa.folder_id
                             FROM folder_albums fa
                             JOIN folders f ON f.id = fa.folder_id
                             WHERE fa.album_id = ?1 AND f.user_id = ?2
                             LIMIT 1",
                            params![id, user_id],
                            |r| r.get(0),
                        )
                        .unwrap_or(None);
                    let folder_path = folder_id
                        .as_ref()
                        .and_then(|fid| folder_path_map.get(fid).cloned())
                        .unwrap_or_default();
                    results.push(AlbumSearchResult {
                        album,
                        folder_id,
                        folder_path,
                    });
                }
            }
        }

        Ok(results)
    }

    /// 构建当前用户分组的完整路径映射（folder_id → "父/子/孙"）
    ///
    /// 多用户隔离：仅读取归属 `user_id` 的分组。
    fn build_folder_paths_for_user(
        &self,
        user_id: i64,
    ) -> Result<std::collections::HashMap<i64, String>, DbError> {
        // 读取当前用户所有分组
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, parent_id FROM folders WHERE user_id = ?1")?;
        let rows = stmt.query_map(params![user_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?;
        let folders: Vec<(i64, String, Option<i64>)> =
            rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)?;

        // 递归构建路径
        fn build(
            id: i64,
            name: &str,
            parent: Option<i64>,
            folders: &[(i64, String, Option<i64>)],
            memo: &mut std::collections::HashMap<i64, String>,
        ) -> String {
            if let Some(p) = memo.get(&id) {
                return p.clone();
            }
            let parent_path = match parent {
                Some(pid) => folders
                    .iter()
                    .find(|(fid, _, _)| *fid == pid)
                    .map(|(_, pname, pparent)| build(pid, pname, *pparent, folders, memo))
                    .unwrap_or_default(),
                None => String::new(),
            };
            let path = if parent_path.is_empty() {
                name.to_string()
            } else {
                format!("{parent_path}/{name}")
            };
            memo.insert(id, path.clone());
            path
        }

        let mut memo = std::collections::HashMap::new();
        for (id, name, parent) in &folders {
            build(*id, name, *parent, &folders, &mut memo);
        }
        Ok(memo)
    }

    /// 根据 ID 获取单个相册（需求 §4.2 get_album）
    ///
    /// 多用户隔离：仅能获取归属 `user_id` 的相册，他人相册等同不存在。
    pub fn get_album(&self, id: i64, user_id: i64) -> Result<Album, DbError> {
        let mut album = self
            .conn
            .query_row(
                "SELECT id, name, path, description, cover_path, created_at, updated_at, location
                 FROM albums
                 WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
                Self::row_to_album,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => DbError::NotFound(id),
                other => DbError::Sqlite(other),
            })?;
        let mut arr = [album.clone()];
        self.load_album_tags(&mut arr)?;
        self.fill_album_folder(&mut arr, user_id)?;
        self.load_merged_sources(&mut arr)?;
        album = arr[0].clone();
        Ok(album)
    }

    /// 更新相册信息（需求 §4.2 update_album）
    ///
    /// 使用 COALESCE 实现"提供则更新，不提供则保留原值"，
    /// 并自动刷新 updated_at。返回受影响行数为 0 时表示 ID 不存在。
    /// 多用户隔离：WHERE 限定归属 `user_id`，他人相册更新等同不存在。
    pub fn update_album(&self, input: UpdateAlbumInput, user_id: i64) -> Result<(), DbError> {
        let now = Self::now_secs();
        // location 支持三种情况：None=保留原值，Some("")=清除，Some("x")=设置
        // 占位符随 location_sql 动态决定，参数列表同步构建，避免参数个数不匹配
        let mut p: Vec<rusqlite::types::Value> = Vec::new();
        p.push(input.id.into());
        p.push(input.name.into());
        p.push(input.description.into());
        p.push(input.cover_path.into());
        p.push(now.into());
        // location 占用 ?6 时 user_id 为 ?7，否则 user_id 为 ?6
        let (location_sql, user_id_ph): (&str, &str) = match &input.location {
            None => ("location", "?6"),                      // 保留原值
            Some(s) if s.trim().is_empty() => ("NULL", "?6"), // 清除
            Some(s) => {
                p.push(rusqlite::types::Value::Text(s.trim().to_string()));
                ("?6", "?7")                                 // 设置新值
            }
        };
        p.push(user_id.into());
        let affected = self.conn.execute(
            &format!(
                "UPDATE albums
                 SET name        = COALESCE(?2, name),
                     description = COALESCE(?3, description),
                     cover_path  = COALESCE(?4, cover_path),
                     location    = {location_sql},
                     updated_at  = ?5
                 WHERE id = ?1 AND user_id = {user_id_ph}"
            ),
            rusqlite::params_from_iter(p),
        )?;
        if affected == 0 {
            return Err(DbError::NotFound(input.id));
        }
        Ok(())
    }

    /// 更新相册地点（自动识别用）：仅写 location，**不动 updated_at**
    ///
    /// 自动地点检测不应打乱列表排序（updated_at 降序），
    /// 手动编辑地点仍走 update_album（会刷新 updated_at）。
    /// 多用户隔离：WHERE 限定归属 `user_id`。
    pub fn update_album_location(&self, id: i64, user_id: i64, location: &str) -> Result<(), DbError> {
        let affected = self.conn.execute(
            "UPDATE albums SET location = ?2 WHERE id = ?1 AND user_id = ?3",
            params![id, location.trim(), user_id],
        )?;
        if affected == 0 {
            return Err(DbError::NotFound(id));
        }
        Ok(())
    }

    /// 删除相册（需求 §4.2 delete_album）
    ///
    /// 仅删除数据库记录，不触碰本地文件（需求 §2.3 核心原则）。
    /// 事务内级联清理关联表（folder_albums / album_tags / album_stats），
    /// 避免孤儿数据残留导致手动树归属判断出错。
    /// 多用户隔离：仅能删除归属 `user_id` 的相册，他人相册等同不存在。
    /// 批量排除相册照片（「记录删除」：网格不再显示，本地文件保留）
    ///
    /// 返回新排除的数量（已排除过的忽略，不算错误）。
    /// 多用户隔离：仅能操作归属当前用户的相册。
    pub fn exclude_album_photos(
        &self,
        album_id: i64,
        user_id: i64,
        paths: &[String],
    ) -> Result<usize, DbError> {
        let tx = self.conn.unchecked_transaction()?;
        // 先校验相册归属（0 行 → NotFound）
        let owned: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM albums WHERE id = ?1 AND user_id = ?2",
                params![album_id, user_id],
                |r| r.get(0),
            )
            .map_err(DbError::Sqlite)?;
        if owned == 0 {
            return Err(DbError::NotFound(album_id));
        }
        let mut n = 0usize;
        for p in paths {
            let affected = tx.execute(
                "INSERT OR IGNORE INTO album_photo_excluded(album_id, path, user_id, excluded_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![album_id, p, user_id, Self::now_secs()],
            )?;
            n += affected;
        }
        tx.commit()?;
        Ok(n)
    }

    /// 列出某相册已被排除的照片路径（过滤 list_album_photos 用）
    pub fn list_excluded_photos(&self, album_id: i64) -> Result<Vec<String>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM album_photo_excluded WHERE album_id = ?1")
            .map_err(DbError::Sqlite)?;
        let rows = stmt
            .query_map(params![album_id], |r| r.get::<_, String>(0))
            .map_err(DbError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
    }

    pub fn delete_album(&self, id: i64, user_id: i64) -> Result<(), DbError> {
        let tx = self.conn.unchecked_transaction()?;
        let affected =
            tx.execute("DELETE FROM albums WHERE id = ?1 AND user_id = ?2", params![id, user_id])?;
        if affected == 0 {
            return Err(DbError::NotFound(id));
        }
        self.delete_album_refs(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// 批量删除相册（勾选删除）
    ///
    /// 在一个事务中删除多个相册记录，保证原子性（全部成功或全部失败）。
    /// 仅删除数据库记录，**不删除本地照片文件**。
    /// 返回实际删除的数量（不存在的 ID 会被忽略，不算错误）。
    /// 多用户隔离：仅能删除归属 `user_id` 的相册。
    pub fn delete_albums(&self, ids: &[i64], user_id: i64) -> Result<usize, DbError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut deleted = 0usize;
        for id in ids {
            let affected =
                tx.execute("DELETE FROM albums WHERE id = ?1 AND user_id = ?2", params![id, user_id])?;
            if affected > 0 {
                deleted += 1;
                self.delete_album_refs(&tx, *id)?;
            }
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// 级联清理某个相册在所有关联表中的记录（须在事务内调用）
    fn delete_album_refs(&self, tx: &rusqlite::Transaction, album_id: i64) -> Result<(), DbError> {
        tx.execute("DELETE FROM folder_albums WHERE album_id = ?1", params![album_id])?;
        tx.execute("DELETE FROM album_tags WHERE album_id = ?1", params![album_id])?;
        tx.execute("DELETE FROM album_stats WHERE album_id = ?1", params![album_id])?;
        tx.execute("DELETE FROM photo_content_scan WHERE album_id = ?1", params![album_id])?;
        tx.execute("DELETE FROM album_photo_excluded WHERE album_id = ?1", params![album_id])?;
        // 合并来源：清理「作为目标相册」的来源记录 + 「作为源被合并」的来源记录
        tx.execute("DELETE FROM album_merged_sources WHERE album_id = ?1 OR source_id = ?1", params![album_id])?;
        Ok(())
    }

    /// 读取相册文件系统统计缓存（photo_count/size_bytes/shoot_time/封面源图）
    pub fn get_album_stats(&self, album_id: i64) -> Result<Option<AlbumStats>, DbError> {
        let result = self.conn.query_row(
            "SELECT photo_count, size_bytes, shoot_time, cover_source, file_count
             FROM album_stats WHERE album_id = ?1",
            params![album_id],
            |r| {
                Ok(AlbumStats {
                    photo_count: r.get::<_, i64>(0)?,
                    size_bytes: r.get::<_, u64>(1)?,
                    shoot_time: r.get(2)?,
                    cover_source: r.get(3)?,
                    file_count: r.get(4)?,
                })
            },
        );
        match result {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// 写入/更新相册文件系统统计缓存
    ///
    /// `file_count` 为扫描时的目录递归文件总数，作为下次加载的变更探测信号。
    pub fn upsert_album_stats(
        &self,
        album_id: i64,
        photo_count: i64,
        size_bytes: u64,
        shoot_time: Option<String>,
        cover_source: Option<String>,
        file_count: i64,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO album_stats
             (album_id, photo_count, size_bytes, shoot_time, cover_source, scanned_at, file_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                album_id,
                photo_count,
                size_bytes,
                shoot_time,
                cover_source,
                Self::now_secs(),
                file_count
            ],
        )?;
        Ok(())
    }

    /// 删除某个相册的统计缓存（封面源缺失降级重扫时调用）
    pub fn delete_album_stats(&self, album_id: i64) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM album_stats WHERE album_id = ?1", params![album_id])?;
        Ok(())
    }

    /// 设置一批照片的打分（rating 0-5，0 清除）。按 (user_id, path) upsert，任意照片可打分。
    pub fn set_photo_rating(&self, user_id: i64, paths: &[String], rating: i64) -> Result<(), DbError> {
        let now = Self::now_secs();
        for p in paths {
            self.conn.execute(
                "INSERT INTO photo_ratings (user_id, path, rating, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(user_id, path) DO UPDATE SET
                   rating = excluded.rating, updated_at = excluded.updated_at",
                params![user_id, p, rating, now],
            )?;
        }
        Ok(())
    }

    /// 查询一批照片的打分，返回 [(path, rating)]（未打分的不出现在结果中）。
    pub fn get_photo_ratings(&self, user_id: i64, paths: &[String]) -> Result<Vec<(String, i64)>, DbError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (1..=paths.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT path, rating FROM photo_ratings WHERE user_id = ?1 AND path IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&user_id];
        for p in paths {
            params_vec.push(p);
        }
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
    }

    /// 移动照片后同步打分表的路径归属（照片合并/移动时调用）。
    pub fn move_photo_rating_path(&self, user_id: i64, old_path: &str, new_path: &str) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE photo_ratings SET path = ?1, updated_at = ?2 WHERE user_id = ?3 AND path = ?4",
            params![new_path, Self::now_secs(), user_id, old_path],
        )?;
        Ok(())
    }

    /// 移动照片后同步内容扫描表的路径与相册归属（用户 + 旧路径定位）。
    pub fn move_photo_content_path(&self, user_id: i64, old_path: &str, new_path: &str, album_id: i64) -> Result<(), DbError> {
        let parent = std::path::Path::new(new_path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        self.conn.execute(
            "UPDATE photo_content_scan SET path = ?1, parent_dir = ?2, album_id = ?3
             WHERE user_id = ?4 AND path = ?5",
            params![new_path, parent, album_id, user_id, old_path],
        )?;
        Ok(())
    }

    /// 更新相册封面路径（仅改 cover_path，不动 updated_at，避免统计刷新影响列表排序）
    ///
    /// 自动封面在扫描/换封面时持久化到 albums.cover_path，加载时直接读取，
    /// 不依赖缩略图生成链（修复：cover_source 为 NULL 时命中路径封面丢失）。
    pub fn update_album_cover(&self, id: i64, cover_path: Option<String>) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE albums SET cover_path = ?1 WHERE id = ?2",
            params![cover_path, id],
        )?;
        Ok(())
    }

    /// 更新相册名称与绑定路径（rename_album 命令专用，文件夹已重命名成功后调用）
    ///
    /// 多用户隔离：WHERE 限定归属 `user_id`。
    pub fn update_album_name_path(
        &self,
        id: i64,
        user_id: i64,
        name: &str,
        path: &str,
    ) -> Result<(), DbError> {
        let now = Self::now_secs();
        let affected = self.conn.execute(
            "UPDATE albums SET name = ?1, path = ?2, updated_at = ?3 WHERE id = ?4 AND user_id = ?5",
            params![name, path, now, id, user_id],
        )?;
        if affected == 0 {
            return Err(DbError::NotFound(id));
        }
        Ok(())
    }

    /// 设置相册标签（覆盖式，最多 5 个）
    ///
    /// 相册不存在返回 NotFound。标签数量超过 5 返回错误。
    /// 多用户隔离：仅能操作归属 `user_id` 的相册。
    pub fn update_album_tags(
        &self,
        album_id: i64,
        user_id: i64,
        tags: Vec<String>,
    ) -> Result<(), DbError> {
        // 检查相册存在（归属当前用户）
        let exists: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM albums WHERE id = ?1 AND user_id = ?2",
                params![album_id, user_id],
                |r| r.get(0),
            )?;
        if !exists {
            return Err(DbError::NotFound(album_id));
        }

        let clean: Vec<String> = tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if clean.len() > 5 {
            return Err(DbError::Other("最多只能添加 5 个标签".into()));
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM album_tags WHERE album_id = ?1", params![album_id])?;
        for tag in clean {
            tx.execute(
                "INSERT INTO album_tags (album_id, tag) VALUES (?1, ?2)",
                params![album_id, tag],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 读取相册标签列表（批量整理：加/删标签前需先取现有）
    pub fn get_album_tag_list(&self, album_id: i64, user_id: i64) -> Result<Vec<String>, DbError> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM albums WHERE id = ?1 AND user_id = ?2",
                params![album_id, user_id],
                |r| r.get(0),
            )?;
        if !exists {
            return Err(DbError::NotFound(album_id));
        }
        let mut stmt = self
            .conn
            .prepare("SELECT tag FROM album_tags WHERE album_id = ?1 ORDER BY id")?;
        let rows = stmt.query_map(params![album_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// album_stats 变更探测：file_count 读写一致（新方案核心）
    #[test]
    fn album_stats_file_count_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        let db = Database { conn };
        db.init_schema().unwrap();

        // 初始无记录
        assert!(db.get_album_stats(1).unwrap().is_none());

        // 写入含 file_count
        db.upsert_album_stats(1, 10, 2048, Some("2026-01-01".into()), Some("/x/a.jpg".into()), 42)
            .unwrap();
        let s = db.get_album_stats(1).unwrap().unwrap();
        assert_eq!(s.photo_count, 10);
        assert_eq!(s.size_bytes, 2048);
        assert_eq!(s.file_count, 42);
        assert_eq!(s.cover_source.as_deref(), Some("/x/a.jpg"));

        // 更新 file_count（目录变化后重扫写回）
        db.upsert_album_stats(1, 12, 4096, Some("2026-02-02".into()), Some("/x/b.jpg".into()), 44)
            .unwrap();
        let s = db.get_album_stats(1).unwrap().unwrap();
        assert_eq!(s.photo_count, 12);
        assert_eq!(s.file_count, 44, "file_count 应反映目录文件数变化");

        // 删除（降级重扫路径）
        db.delete_album_stats(1).unwrap();
        assert!(db.get_album_stats(1).unwrap().is_none());
    }

    /// 内存数据库单元测试，验证建表与完整 CRUD 流程
    ///
    /// 多用户隔离：全部操作归属 user_id=1（无 users 行也可，albums.user_id 无外键约束）。
    #[test]
    fn album_crud_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        let db = Database { conn };
        db.init_schema().unwrap();

        // 创建一个临时目录作为相册路径
        let tmp = std::env::temp_dir().join("pm_test_album");
        std::fs::create_dir_all(&tmp).unwrap();

        // create
        let created = db
            .create_album(
                CreateAlbumInput {
                    name: "测试相册".into(),
                    path: tmp.to_string_lossy().into_owned(),
                    description: Some("desc".into()),
                },
                1,
            )
            .unwrap();
        assert_eq!(created.name, "测试相册");

        // 重复路径应报冲突
        let dup = db.create_album(
            CreateAlbumInput {
                name: "另一个".into(),
                path: tmp.to_string_lossy().into_owned(),
                description: None,
            },
            1,
        );
        assert!(matches!(dup, Err(DbError::PathAlreadyUsed(_))));

        // get_albums（仅返回当前用户的相册）
        let list = db.get_albums(1).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(db.get_albums(2).unwrap().len(), 0, "其他用户看不到该相册");

        // update
        db.update_album(
            UpdateAlbumInput {
                id: created.id,
                name: Some("改名".into()),
                description: None,
                cover_path: None,
                location: None,
            },
            1,
        )
        .unwrap();
        assert_eq!(db.get_album(created.id, 1).unwrap().name, "改名");
        // 其他用户更新/读取 → 等同不存在
        assert!(matches!(
            db.update_album(
                UpdateAlbumInput {
                    id: created.id,
                    name: Some("越权改名".into()),
                    description: None,
                    cover_path: None,
                    location: None,
                },
                2,
            ),
            Err(DbError::NotFound(_))
        ));
        assert!(matches!(
            db.get_album(created.id, 2),
            Err(DbError::NotFound(_))
        ));

        // delete（其他用户删除 → 等同不存在，数据不受影响）
        assert!(matches!(
            db.delete_album(created.id, 2),
            Err(DbError::NotFound(_))
        ));
        assert!(db.get_album(created.id, 1).is_ok());
        db.delete_album(created.id, 1).unwrap();
        assert!(matches!(
            db.get_album(created.id, 1),
            Err(DbError::NotFound(_))
        ));
    }

    /// 多用户相册空间隔离：两个用户各建相册互不可见
    #[test]
    fn album_user_isolation() {
        let conn = Connection::open_in_memory().unwrap();
        let db = Database { conn };
        db.init_schema().unwrap();

        let dir_a = std::env::temp_dir().join("pm_test_user_a");
        let dir_b = std::env::temp_dir().join("pm_test_user_b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        let a = db
            .create_album(
                CreateAlbumInput {
                    name: "用户A的相册".into(),
                    path: dir_a.to_string_lossy().into_owned(),
                    description: None,
                },
                1,
            )
            .unwrap();
        let b = db
            .create_album(
                CreateAlbumInput {
                    name: "用户B的相册".into(),
                    path: dir_b.to_string_lossy().into_owned(),
                    description: None,
                },
                2,
            )
            .unwrap();

        assert_eq!(db.get_albums(1).unwrap().len(), 1);
        assert_eq!(db.get_albums(1).unwrap()[0].id, a.id);
        assert_eq!(db.get_albums(2).unwrap().len(), 1);
        assert_eq!(db.get_albums(2).unwrap()[0].id, b.id);

        // 搜索同样隔离
        assert_eq!(db.search_albums("相册", 1).unwrap().len(), 1);
        assert_eq!(db.search_albums("相册", 1).unwrap()[0].album.id, a.id);
        assert_eq!(db.search_albums("相册", 2).unwrap()[0].album.id, b.id);
    }

    /// FEAT-A：合并来源表 album_merged_sources 读写 / 多用户隔离 / 级联删除
    ///
    /// 验证：
    /// 1. 插入 → get_albums / get_album 能看到 merged_sources
    /// 2. 重复插入同 (album_id, source_id) 被 IGNORE 幂等
    /// 3. 删除源相册时级联清空（album_merged_sources 不会产生孤儿行）
    #[test]
    fn merged_sources_persist_and_cascade() {
        let conn = Connection::open_in_memory().unwrap();
        let db = Database { conn };
        db.init_schema().unwrap();

        let dir_target = std::env::temp_dir().join("pm_test_merge_target");
        let dir_src1 = std::env::temp_dir().join("pm_test_merge_src1");
        let dir_src2 = std::env::temp_dir().join("pm_test_merge_src2");
        std::fs::create_dir_all(&dir_target).unwrap();
        std::fs::create_dir_all(&dir_src1).unwrap();
        std::fs::create_dir_all(&dir_src2).unwrap();

        let target = db
            .create_album(
                CreateAlbumInput {
                    name: "目标".into(),
                    path: dir_target.to_string_lossy().into_owned(),
                    description: None,
                },
                1,
            )
            .unwrap();
        let src1 = db
            .create_album(
                CreateAlbumInput {
                    name: "源1".into(),
                    path: dir_src1.to_string_lossy().into_owned(),
                    description: None,
                },
                1,
            )
            .unwrap();
        let src2 = db
            .create_album(
                CreateAlbumInput {
                    name: "源2".into(),
                    path: dir_src2.to_string_lossy().into_owned(),
                    description: None,
                },
                1,
            )
            .unwrap();

        let now = Database::now_secs();
        // 直接写 source 记录（避开 UI、模拟 merge_albums 写入的路径）
        db.conn.execute(
            "INSERT INTO album_merged_sources (album_id, source_id, source_name, source_path, user_id, merged_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![target.id, src1.id, "源1", dir_src1.to_string_lossy(), 1, now],
        ).unwrap();
        db.conn.execute(
            "INSERT INTO album_merged_sources (album_id, source_id, source_name, source_path, user_id, merged_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![target.id, src2.id, "源2", dir_src2.to_string_lossy(), 1, now],
        ).unwrap();

        // 重复插入同 (album_id, source_id) 应被 UNIQUE IGNORE
        db.conn.execute(
            "INSERT OR IGNORE INTO album_merged_sources (album_id, source_id, source_name, source_path, user_id, merged_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![target.id, src1.id, "源1-重复", "/x/y", 1, now],
        ).unwrap();
        let count: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM album_merged_sources WHERE album_id = ?1",
            rusqlite::params![target.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 2, "UNIQUE 约束应阻止重复");

        // get_albums 应能读到 merged_sources
        let list = db.get_albums(1).unwrap();
        let target_filled = list.iter().find(|a| a.id == target.id).unwrap();
        assert_eq!(target_filled.merged_sources.len(), 2);
        let names: Vec<&str> = target_filled.merged_sources.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"源1") && names.contains(&"源2"));

        // get_album 也应填充
        let one = db.get_album(target.id, 1).unwrap();
        assert_eq!(one.merged_sources.len(), 2);

        // 删除源1 → 级联清空 (album_id, source_id=src1) 与 (album_id=src1, *) 两种行
        db.delete_album(src1.id, 1).unwrap();
        let count_after: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM album_merged_sources WHERE album_id = ?1",
            rusqlite::params![target.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count_after, 1, "删除源1后应只剩 src2");
        let remaining = &db.get_album(target.id, 1).unwrap().merged_sources;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, src2.id);
    }

    /// FEAT-034-C：find_any_album_by_path 不限用户匹配，用于批量导入时判断
    /// 路径是否已被任一用户占用（配合全局 UNIQUE 避免误报“已被相册使用”）。
    #[test]
    fn find_any_album_by_path_cross_user() {
        let conn = Connection::open_in_memory().unwrap();
        let db = Database { conn };
        db.init_schema().unwrap();

        let dir = std::env::temp_dir().join("pm_find_any_path");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.to_string_lossy().into_owned();

        // 用户 1 创建该路径相册
        db.create_album(
            CreateAlbumInput { name: "用户A相册".into(), path: p.clone(), description: None },
            1,
        )
        .unwrap();

        // 同用户查询命中
        assert!(db.find_album_by_path(&p, 1).unwrap().is_some());
        // 不同用户 find_album_by_path 查不到（多用户隔离）
        assert!(db.find_album_by_path(&p, 2).unwrap().is_none());
        // 不限用户查询命中（FEAT-034-C 关键）
        let any = db.find_any_album_by_path(&p).unwrap().unwrap();
        assert_eq!(any.name, "用户A相册");

        // 未存在路径返回 None
        assert!(db.find_any_album_by_path("/no/such/path").unwrap().is_none());
    }

}
