//! 相册数据持久化层
//!
//! 对应 SpringBoot 中的：
//! - `Album` 结构体  →  `@Entity` 实体类
//! - `CreateAlbumInput` / `UpdateAlbumInput`  →  `@RequestBody DTO`
//! - `Database` 结构体  →  `Repository` + `DataSource`
//! - `init_schema`  →  `schema.sql` 建表脚本
//! - `DbError`  →  自定义业务异常（配合全局异常处理）

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
        })
    }

    // ---------------- CRUD ----------------

    /// 创建相册
    ///
    /// 1. 校验路径是否存在（需求 §2.2 路径合法性校验）
    /// 2. 插入记录，捕获 UNIQUE 冲突并返回占用该路径的相册名（需求 §7.2）
    pub fn create_album(&self, input: CreateAlbumInput) -> Result<Album, DbError> {
        if !Path::new(&input.path).is_dir() {
            return Err(DbError::PathNotExist(input.path));
        }
        let now = Self::now_secs();
        let result = self.conn.execute(
            "INSERT INTO albums (name, path, description, cover_path, created_at, updated_at)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            params![input.name, input.path, input.description, now, now],
        );
        match result {
            Ok(_) => {
                let id = self.conn.last_insert_rowid();
                self.get_album(id)
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

    /// 根据路径查找相册（不存在返回 None）
    ///
    /// 用于批量导入时检查某个文件夹是否已作为相册存在，避免重复创建。
    pub fn find_album_by_path(&self, path: &str) -> Result<Option<Album>, DbError> {
        let result = self.conn.query_row(
            "SELECT id, name, path, description, cover_path, created_at, updated_at, location
             FROM albums
             WHERE path = ?1",
            params![path],
            Self::row_to_album,
        );
        match result {
            Ok(album) => Ok(Some(album)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DbError::Sqlite(e)),
        }
    }

    /// 获取所有相册，按 updated_at 降序（需求 §4.2 get_albums）
    pub fn get_albums(&self) -> Result<Vec<Album>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, path, description, cover_path, created_at, updated_at, location
             FROM albums
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], Self::row_to_album)?;
        let mut albums: Vec<Album> = rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)?;
        self.load_album_tags(&mut albums)?;
        self.fill_album_folder(&mut albums)?;
        Ok(albums)
    }

    /// 填充相册的分组信息（folder_id + folder_path）
    ///
    /// **唯一事实源是 `folder_albums` 关联表**，`albums.folder_id` 仅是事务内同步的
    /// 冗余缓存列，读取不依赖它。一次查询批量填充，避免 N+1 与四层兜底。
    fn fill_album_folder(&self, albums: &mut [Album]) -> Result<(), DbError> {
        // 构建分组路径映射
        let folder_path_map = self.build_folder_paths()?;
        // 从 folder_albums 一次读取所有相册的分组归属
        let mut stmt = self.conn.prepare("SELECT album_id, folder_id FROM folder_albums")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
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

    /// 按关键词搜索相册
    ///
    /// 搜索范围覆盖（需求：不只搜最小相册，也搜相册集/父相册）：
    /// - 相册名称
    /// - 相册标签（album_tags）
    /// - 相册所属分组（及其祖先分组）的名称
    /// - 相册所属分组（及其祖先分组）的标签（folder_tags）
    ///
    /// 返回匹配相册及其所属分组路径。
    pub fn search_albums(&self, keyword: &str) -> Result<Vec<AlbumSearchResult>, DbError> {
        let kw = format!("%{}%", keyword.trim());

        // 收集所有能匹配关键词的相册 id 集合（去重）
        let mut matched_ids: Vec<i64> = Vec::new();
        // 1. 相册名匹配
        {
            let mut stmt = self.conn.prepare("SELECT id FROM albums WHERE name LIKE ?1")?;
            let rows = stmt.query_map(params![kw], |r| r.get::<_, i64>(0))?;
            for row in rows {
                matched_ids.push(row?);
            }
        }
        // 2. 相册标签匹配
        {
            let mut stmt = self.conn.prepare(
                "SELECT at.album_id FROM album_tags at WHERE at.tag LIKE ?1",
            )?;
            let rows = stmt.query_map(params![kw], |r| r.get::<_, i64>(0))?;
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
                 WHERE f.name LIKE ?1
                    OR EXISTS (
                        SELECT 1 FROM folder_tags ft WHERE ft.folder_id = f.id AND ft.tag LIKE ?1
                    )",
            )?;
            let rows = stmt.query_map(params![kw], |r| r.get::<_, i64>(0))?;
            for row in rows {
                matched_ids.push(row?);
            }
        }
        // 4. 祖先分组名/标签匹配（相册分组的祖先链）
        {
            // 获取所有分组归属，逐一检查祖先链
            let mut stmt = self.conn.prepare(
                "SELECT album_id, folder_id FROM folder_albums",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
            })?;
            // 构建分组名/标签查找
            let mut folder_name_stmt =
                self.conn.prepare("SELECT id, name FROM folders")?;
            let folder_name_map: std::collections::HashMap<i64, String> = folder_name_stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<_, _>>()?;
            let mut folder_parent_stmt =
                self.conn.prepare("SELECT id, parent_id FROM folders")?;
            let folder_parent_map: std::collections::HashMap<i64, Option<i64>> =
                folder_parent_stmt
                    .query_map([], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
                    })?
                    .collect::<Result<_, _>>()?;
            let folder_tag_map: std::collections::HashMap<i64, Vec<String>> = {
                let mut map: std::collections::HashMap<i64, Vec<String>> =
                    std::collections::HashMap::new();
                let mut stmt2 = self.conn.prepare("SELECT folder_id, tag FROM folder_tags")?;
                let tag_rows = stmt2.query_map([], |r| {
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
            // 构建文件夹路径映射
            let folder_path_map = self.build_folder_paths()?;
            for id in matched_ids {
                if let Ok(album) = self.get_album(id) {
                    // 获取 folder_id（唯一事实源：folder_albums）
                    let folder_id: Option<i64> = self
                        .conn
                        .query_row(
                            "SELECT folder_id FROM folder_albums WHERE album_id = ?1 LIMIT 1",
                            params![id],
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

    /// 构建所有分组的完整路径映射（folder_id → "父/子/孙"）
    fn build_folder_paths(&self) -> Result<std::collections::HashMap<i64, String>, DbError> {
        // 读取所有分组
        let mut stmt = self.conn.prepare("SELECT id, name, parent_id FROM folders")?;
        let rows = stmt.query_map([], |r| {
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
    pub fn get_album(&self, id: i64) -> Result<Album, DbError> {
        let mut album = self
            .conn
            .query_row(
                "SELECT id, name, path, description, cover_path, created_at, updated_at, location
                 FROM albums
                 WHERE id = ?1",
                params![id],
                Self::row_to_album,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => DbError::NotFound(id),
                other => DbError::Sqlite(other),
            })?;
        let mut arr = [album.clone()];
        self.load_album_tags(&mut arr)?;
        self.fill_album_folder(&mut arr)?;
        album = arr[0].clone();
        Ok(album)
    }

    /// 更新相册信息（需求 §4.2 update_album）
    ///
    /// 使用 COALESCE 实现"提供则更新，不提供则保留原值"，
    /// 并自动刷新 updated_at。返回受影响行数为 0 时表示 ID 不存在。
    pub fn update_album(&self, input: UpdateAlbumInput) -> Result<(), DbError> {
        let now = Self::now_secs();
        // location 支持三种情况：None=保留原值，Some("")=清除，Some("x")=设置
        // 占位符随 location_sql 动态决定，参数列表同步构建，避免参数个数不匹配
        let location_sql: &str;
        let mut p: Vec<rusqlite::types::Value> = Vec::new();
        p.push(input.id.into());
        p.push(input.name.into());
        p.push(input.description.into());
        p.push(input.cover_path.into());
        p.push(now.into());
        match &input.location {
            None => location_sql = "location",            // 保留原值
            Some(s) if s.trim().is_empty() => location_sql = "NULL", // 清除
            Some(s) => {
                location_sql = "?6";                      // 设置新值
                p.push(rusqlite::types::Value::Text(s.trim().to_string()));
            }
        }
        let affected = self.conn.execute(
            &format!(
                "UPDATE albums
                 SET name        = COALESCE(?2, name),
                     description = COALESCE(?3, description),
                     cover_path  = COALESCE(?4, cover_path),
                     location    = {location_sql},
                     updated_at  = ?5
                 WHERE id = ?1"
            ),
            rusqlite::params_from_iter(p),
        )?;
        if affected == 0 {
            return Err(DbError::NotFound(input.id));
        }
        Ok(())
    }

    /// 删除相册（需求 §4.2 delete_album）
    ///
    /// 仅删除数据库记录，不触碰本地文件（需求 §2.3 核心原则）。
    /// 事务内级联清理关联表（folder_albums / album_tags / album_stats），
    /// 避免孤儿数据残留导致手动树归属判断出错。
    pub fn delete_album(&self, id: i64) -> Result<(), DbError> {
        let tx = self.conn.unchecked_transaction()?;
        let affected = tx.execute("DELETE FROM albums WHERE id = ?1", params![id])?;
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
    pub fn delete_albums(&self, ids: &[i64]) -> Result<usize, DbError> {
        let tx = self.conn.unchecked_transaction()?;
        let mut deleted = 0usize;
        for id in ids {
            let affected = tx.execute("DELETE FROM albums WHERE id = ?1", params![id])?;
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

    /// 设置相册标签（覆盖式，最多 5 个）
    ///
    /// 相册不存在返回 NotFound。标签数量超过 5 返回错误。
    pub fn update_album_tags(&self, album_id: i64, tags: Vec<String>) -> Result<(), DbError> {
        // 检查相册存在
        let exists: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM albums WHERE id = ?1",
                params![album_id],
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
            .create_album(CreateAlbumInput {
                name: "测试相册".into(),
                path: tmp.to_string_lossy().into_owned(),
                description: Some("desc".into()),
            })
            .unwrap();
        assert_eq!(created.name, "测试相册");

        // 重复路径应报冲突
        let dup = db.create_album(CreateAlbumInput {
            name: "另一个".into(),
            path: tmp.to_string_lossy().into_owned(),
            description: None,
        });
        assert!(matches!(dup, Err(DbError::PathAlreadyUsed(_))));

        // get_albums
        let list = db.get_albums().unwrap();
        assert_eq!(list.len(), 1);

        // update
        db.update_album(UpdateAlbumInput {
            id: created.id,
            name: Some("改名".into()),
            description: None,
            cover_path: None,
            location: None,
        })
        .unwrap();
        assert_eq!(db.get_album(created.id).unwrap().name, "改名");

        // delete
        db.delete_album(created.id).unwrap();
        assert!(matches!(
            db.get_album(created.id),
            Err(DbError::NotFound(_))
        ));
    }
}
