//! 手动排序分组模块（模仿文件夹）
//!
//! 提供手动自主排序的数据结构：
//! - `folders` 表：手动分组，最多三级父子关系（level 1/2/3）
//! - `folder_tags` 表：每个分组的标签（最多 5 个）
//! - `albums.folder_id` / `albums.sort_order`：相册所属分组及组内顺序
//!
//! 对应需求：手动自主排序，模仿文件夹建立方式，父文件夹支持最多 5 个标签和 1 个说明。

use rusqlite::{params, Connection};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// 最大分组层级
pub const MAX_LEVEL: i64 = 3;
/// 最大标签数量
pub const MAX_TAGS: usize = 5;

/// 分组（文件夹）实体
#[derive(Debug, Clone, Serialize)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    /// 父分组 ID，NULL 为顶级
    pub parent_id: Option<i64>,
    /// 层级 1/2/3
    pub level: i64,
    pub sort_order: i64,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

/// 手动排序整体结构（返回给前端）
#[derive(Debug, Clone, Serialize)]
pub struct ManualTree {
    /// 顶级文件夹（含子级嵌套信息由前端从列表构建）
    pub folders: Vec<Folder>,
    /// 属于各文件夹的相册 ID 映射
    pub folder_albums: Vec<FolderAlbumEntry>,
    /// 不属于任何文件夹的相册（顶级游离相册），含顺序
    pub root_albums: Vec<RootAlbumEntry>,
}

/// 文件夹 → 相册关联
#[derive(Debug, Clone, Serialize)]
pub struct FolderAlbumEntry {
    pub folder_id: i64,
    pub album_ids: Vec<i64>,
}

/// 顶级相册（不属于任何文件夹）
#[derive(Debug, Clone, Serialize)]
pub struct RootAlbumEntry {
    pub album_id: i64,
    pub sort_order: i64,
}

/// 手动排序错误
#[derive(Debug)]
pub enum FolderError {
    /// 层级超过 3 级
    LevelExceeded,
    /// 分组名称为空
    EmptyName,
    /// 父分组不存在
    ParentNotFound,
    /// 标签数量超过 5
    TooManyTags,
    /// 分组不存在
    NotFound,
    /// 数据库错误
    Sqlite(rusqlite::Error),
}

impl std::fmt::Display for FolderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FolderError::LevelExceeded => write!(f, "分组层级最多支持 3 级"),
            FolderError::EmptyName => write!(f, "分组名称不能为空"),
            FolderError::ParentNotFound => write!(f, "父分组不存在"),
            FolderError::TooManyTags => write!(f, "最多只能添加 5 个标签"),
            FolderError::NotFound => write!(f, "分组不存在"),
            FolderError::Sqlite(e) => write!(f, "数据库错误: {e}"),
        }
    }
}

impl From<rusqlite::Error> for FolderError {
    fn from(e: rusqlite::Error) -> Self {
        FolderError::Sqlite(e)
    }
}

/// 当前 Unix 时间戳（秒）
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 创建分组
///
/// - `parent_id` 为 None 时创建顶级（level=1）
/// - 有父分组时，level = 父.level + 1，最多 3 级
/// - `user_id` 为归属用户（多用户隔离，分组属于当前登录用户）
pub fn create_folder(
    conn: &Connection,
    user_id: i64,
    name: &str,
    parent_id: Option<i64>,
) -> Result<Folder, FolderError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(FolderError::EmptyName);
    }

    let level = match parent_id {
        Some(pid) => {
            let parent_level = conn.query_row(
                "SELECT level FROM folders WHERE id = ?1 AND user_id = ?2",
                params![pid, user_id],
                |r| r.get::<_, i64>(0),
            );
            match parent_level {
                Ok(pl) => {
                    if pl >= MAX_LEVEL {
                        return Err(FolderError::LevelExceeded);
                    }
                    pl + 1
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    return Err(FolderError::ParentNotFound);
                }
                Err(e) => return Err(FolderError::Sqlite(e)),
            }
        }
        None => 1,
    };

    // 同级中排在末尾
    let sort_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM folders WHERE parent_id IS ?1 AND user_id = ?2",
        params![parent_id, user_id],
        |r| r.get(0),
    )?;

    let now = now_secs();
    conn.execute(
        "INSERT INTO folders (name, parent_id, level, sort_order, description, created_at, updated_at, user_id)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7)",
        params![name, parent_id, level, sort_order, now, now, user_id],
    )?;
    let id = conn.last_insert_rowid();

    Ok(Folder {
        id,
        name: name.to_string(),
        parent_id,
        level,
        sort_order,
        description: None,
        tags: Vec::new(),
    })
}

/// 更新分组（名称、说明、标签）
///
/// - `name`/`description` 为 None 时保留原值
/// - `tags` 覆盖式设置，最多 5 个
/// - `user_id` 校验分组归属（多用户隔离，他人分组等同不存在）
pub fn update_folder(
    conn: &Connection,
    user_id: i64,
    id: i64,
    name: Option<&str>,
    description: Option<&str>,
    tags: Option<Vec<String>>,
) -> Result<Folder, FolderError> {
    // 检查存在（归属当前用户）
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM folders WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(FolderError::NotFound);
    }

    if let Some(n) = name {
        let n = n.trim();
        if !n.is_empty() {
            conn.execute(
                "UPDATE folders SET name = ?1, updated_at = ?2 WHERE id = ?3",
                params![n, now_secs(), id],
            )?;
        }
    }
    if let Some(d) = description {
        conn.execute(
            "UPDATE folders SET description = ?1, updated_at = ?2 WHERE id = ?3",
            params![d.trim(), now_secs(), id],
        )?;
    }

    // 覆盖式设置标签（最多 5 个）
    if let Some(tags) = tags {
        let clean: Vec<String> = tags
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if clean.len() > MAX_TAGS {
            return Err(FolderError::TooManyTags);
        }
        conn.execute("DELETE FROM folder_tags WHERE folder_id = ?1", params![id])?;
        for tag in clean {
            conn.execute(
                "INSERT INTO folder_tags (folder_id, tag) VALUES (?1, ?2)",
                params![id, tag],
            )?;
        }
    }

    get_folder(conn, user_id, id)
}

/// 删除分组
///
/// - 删除分组本身
/// - 其下直接相册移到顶级（folder_id=NULL）
/// - 子分组升级为顶级（parent_id=NULL, level=1）
/// - `user_id` 校验分组归属（多用户隔离）
pub fn delete_folder(conn: &Connection, user_id: i64, id: i64) -> Result<(), FolderError> {
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM folders WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(FolderError::NotFound);
    }

    // 该分组的相册移到顶级
    conn.execute(
        "UPDATE albums SET folder_id = NULL, sort_order = 0 WHERE folder_id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;
    // 清理该分组的子相册关联
    conn.execute("DELETE FROM folder_albums WHERE folder_id = ?1", params![id])?;
    // 子分组升级为顶级
    conn.execute(
        "UPDATE folders SET parent_id = NULL, level = 1 WHERE parent_id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;
    // 删除标签
    conn.execute("DELETE FROM folder_tags WHERE folder_id = ?1", params![id])?;
    // 删除分组
    conn.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
    Ok(())
}

/// 查询单个分组（多用户隔离：仅能查询归属 `user_id` 的分组）
pub fn get_folder(conn: &Connection, user_id: i64, id: i64) -> Result<Folder, FolderError> {
    let folder = conn.query_row(
        "SELECT id, name, parent_id, level, sort_order, description FROM folders WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
        |r| {
            Ok(Folder {
                id: r.get(0)?,
                name: r.get(1)?,
                parent_id: r.get(2)?,
                level: r.get(3)?,
                sort_order: r.get(4)?,
                description: r.get(5)?,
                tags: Vec::new(),
            })
        },
    )?;
    // 加载标签
    let mut stmt = conn.prepare("SELECT tag FROM folder_tags WHERE folder_id = ?1 ORDER BY id")?;
    let tags = stmt
        .query_map(params![id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut folder = folder;
    folder.tags = tags;
    Ok(folder)
}

/// 同步 albums.folder_id 到 folder_albums 关联表
///
/// 兼容旧数据：之前只有 albums.folder_id，现在双写。此函数把已有的
/// albums.folder_id 迁移到 folder_albums，避免旧数据在手动树中丢失。
/// 多用户隔离：仅同步归属 `user_id` 的相册。
fn sync_folder_albums(conn: &Connection, user_id: i64) -> Result<(), FolderError> {
    // 从 albums 读取 folder_id 有值的相册，INSERT OR IGNORE 到 folder_albums
    conn.execute(
        "INSERT OR IGNORE INTO folder_albums (folder_id, album_id, sort_order)
         SELECT folder_id, id, sort_order FROM albums WHERE folder_id IS NOT NULL AND user_id = ?1",
        params![user_id],
    )?;
    Ok(())
}

/// 获取手动排序整体结构（多用户隔离：仅返回归属 `user_id` 的分组与相册）
pub fn get_manual_tree(conn: &Connection, user_id: i64) -> Result<ManualTree, FolderError> {
    // 兼容旧数据：把 albums.folder_id 同步到 folder_albums
    sync_folder_albums(conn, user_id)?;

    // 当前用户所有分组
    let mut folders = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, name, parent_id, level, sort_order, description FROM folders
             WHERE user_id = ?1 ORDER BY level, parent_id, sort_order",
        )?;
        let rows = stmt.query_map(params![user_id], |r| {
            Ok(Folder {
                id: r.get(0)?,
                name: r.get(1)?,
                parent_id: r.get(2)?,
                level: r.get(3)?,
                sort_order: r.get(4)?,
                description: r.get(5)?,
                tags: Vec::new(),
            })
        })?;
        for f in rows {
            folders.push(f?);
        }
    }
    // 加载当前用户所有分组的标签
    {
        let mut stmt = conn.prepare(
            "SELECT ft.folder_id, ft.tag
             FROM folder_tags ft
             JOIN folders f ON f.id = ft.folder_id
             WHERE f.user_id = ?1 ORDER BY ft.id",
        )?;
        let rows = stmt
            .query_map(params![user_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut tag_map: std::collections::HashMap<i64, Vec<String>> =
            std::collections::HashMap::new();
        for row in rows {
            let (fid, tag) = row?;
            tag_map.entry(fid).or_default().push(tag);
        }
        for f in folders.iter_mut() {
            if let Some(t) = tag_map.get(&f.id) {
                f.tags = t.clone();
            }
        }
    }

    // 文件夹内相册（从 folder_albums 关联表读取，保证分组归属可靠；限定当前用户）
    let mut folder_albums = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT fa.folder_id, fa.album_id
             FROM folder_albums fa
             JOIN folders f ON f.id = fa.folder_id
             JOIN albums a ON a.id = fa.album_id
             WHERE f.user_id = ?1 AND a.user_id = ?1
             ORDER BY fa.folder_id, fa.sort_order, fa.id",
        )?;
        let rows = stmt
            .query_map(params![user_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        let mut map: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
        for row in rows {
            let (fid, aid) = row?;
            map.entry(fid).or_default().push(aid);
        }
        for (fid, aids) in map {
            folder_albums.push(FolderAlbumEntry { folder_id: fid, album_ids: aids });
        }
    }

    // 顶级相册（不在任何 folder_albums 关联里的相册，限定当前用户）
    let mut root_albums = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.sort_order
             FROM albums a
             WHERE a.user_id = ?1
               AND NOT EXISTS (SELECT 1 FROM folder_albums fa WHERE fa.album_id = a.id)
             ORDER BY a.sort_order, a.id",
        )?;
        let rows = stmt.query_map(params![user_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (aid, sort) = row?;
            root_albums.push(RootAlbumEntry { album_id: aid, sort_order: sort });
        }
    }

    Ok(ManualTree { folders, folder_albums, root_albums })
}
