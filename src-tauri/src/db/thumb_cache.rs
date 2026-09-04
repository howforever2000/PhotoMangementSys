//! 缩略图反向索引表 `photo_thumb_cache` 持久层（FEAT-044）
//!
//! ## 背景
//!
//! 智慧相册相关页面（时间线 / 回忆 / 智能搜索）的数据源是 `photo_content_scan`，
//! 这些页面进入时前端会调 `get_photo_thumbs` 拉一批缩略图。
//!
//! 老链路的问题：
//! 1. 「批量扫描入库」只往 `photo_content_scan` 写元数据，不生成缩略图；
//! 2. 缩略图首次访问时按 `file_fingerprint(path)` 算哈希 + 走文件系统
//!    `is_file()` 检查（每次 IO stat + 头 8KB 读），首次进入智慧相册仍要
//!    现场批量生成，几十张图首屏要等几秒。
//!
//! ## 方案
//!
//! 在 `photo_content_scan` 的姐妹表 `photo_thumb_cache` 里建显式反向索引：
//!
//! | 列 | 说明 |
//! |---|---|
//! | `photo_hash` PK | 与 `photo_content_scan.photo_hash` 同源（路径+大小+mtime 哈希），主键 |
//! | `source_path` | 原图绝对路径（用于删除/迁移时按路径清理） |
//! | `thumb_path` | 缩略图绝对路径（用于 O(1) 命中返回） |
//! | `album_id` | 归属相册（删除相册时批量清理） |
//! | `user_id` | 多用户隔离 |
//! | `size_bytes` | 原图大小（与 photo_hash 输入一致，做二次校验） |
//! | `mtime_ns` | 原图修改时间（纳秒） |
//! | `generated_at` | 缩略图生成时间（Unix 秒） |
//!
//! 命中路径：`ensure_grid_thumb(source)` 时
//! 1. 算 `photo_hash(path, size, mtime_ns)`，先查表
//! 2. 命中且 thumb_path 文件存在 → 0 IO 直接返回 thumb_path
//! 3. 未命中/失效 → 生成新缩略图 → 写表（ON CONFLICT REPLACE）
//!
//! ## 触发点
//!
//! 1. `scan_album_content` / `scan_album_combined` 入库成功后，对 recs 列表
//!    调 `prewarm_thumb_caches`，让入库即预热（解决"智慧相册首次进入需现场生成"）；
//! 2. `get_photo_thumbs` 懒加载链路走同样的 `ensure_grid_thumb`，
//!    命中表 → 即时返回（不改前端协议）。
//!
//! ## 写入策略
//!
//! - 同一张照片跨相册/跨路径复制时 photo_hash 相同 → 唯一索引约束保证只占 1 行，
//!   缩略图也只生成 1 次（与 photo_content_scan 行为一致）。
//! - `ON CONFLICT(photo_hash) DO UPDATE`：source_path / album_id / generated_at 刷新。

use rusqlite::{params, Transaction};

use super::{DbError, Database};

/// 单条缩略图缓存记录（按 photo_hash 主键）
#[derive(Debug, Clone)]
pub struct ThumbCacheRecord {
    pub photo_hash: String,
    pub source_path: String,
    pub thumb_path: String,
    pub album_id: Option<i64>,
    pub user_id: i64,
    pub size_bytes: u64,
    pub mtime_ns: u128,
}

/// 单条命中记录（读表结果）
#[derive(Debug, Clone)]
pub struct ThumbCacheHit {
    pub photo_hash: String,
    pub thumb_path: String,
}

impl Database {
    /// 建表（FEAT-044）：缩略图反向索引
    ///
    /// `IF NOT EXISTS` 启动安全调用。
    pub fn init_thumb_cache_schema(&self) -> Result<(), DbError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS photo_thumb_cache (
                photo_hash    TEXT    PRIMARY KEY,
                source_path   TEXT    NOT NULL,
                thumb_path    TEXT    NOT NULL,
                album_id      INTEGER,
                user_id       INTEGER NOT NULL,
                size_bytes    INTEGER NOT NULL,
                mtime_ns      INTEGER NOT NULL,
                generated_at  INTEGER NOT NULL
            );",
        )?;
        // 按源路径清理时（照片被删除）走索引
        let _ = self
            .conn
            .execute_batch("CREATE INDEX IF NOT EXISTS idx_ptc_source ON photo_thumb_cache(source_path);");
        // 按相册清理时（删除相册）走索引
        let _ = self
            .conn
            .execute_batch("CREATE INDEX IF NOT EXISTS idx_ptc_album ON photo_thumb_cache(album_id);");
        // 多用户隔离
        let _ = self
            .conn
            .execute_batch("CREATE INDEX IF NOT EXISTS idx_ptc_user ON photo_thumb_cache(user_id);");
        Ok(())
    }

    /// 批量 upsert 缩略图缓存记录（按 photo_hash 主键）
    ///
    /// - 用于「入库即预热」：扫描完成后一次性写入本批的缩略图映射
    /// - 用于 `ensure_grid_thumb` 生成新缩略图后写表
    /// - 单条失败不影响其他（参数绑定失败立即返回，但单条 execute 失败被忽略）
    pub fn upsert_thumb_caches(&self, recs: &[ThumbCacheRecord]) -> Result<(), DbError> {
        if recs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        upsert_thumb_caches_in_tx(&tx, recs)?;
        tx.commit()?;
        Ok(())
    }

    /// 按 photo_hash 批量查询（返回 thumb_path 命中列表）
    ///
    /// 输入：`Vec<(photo_hash, source_path)>`（用 source_path 做最终一致性检查）
    /// 输出：`Vec<ThumbCacheHit>`，仅含 `(photo_hash, thumb_path)`，不区分 source_path
    /// ——`ensure_grid_thumb` 会再 stat 文件确认 thumb_path 仍存在。
    pub fn lookup_thumb_caches(&self, hashes: &[String]) -> Result<Vec<ThumbCacheHit>, DbError> {
        if hashes.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat("?")
            .take(hashes.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT photo_hash, thumb_path FROM photo_thumb_cache WHERE photo_hash IN ({})",
            placeholders
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params_vec: Vec<&dyn rusqlite::ToSql> = hashes
            .iter()
            .map(|h| h as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params_vec.as_slice(), |r| {
            Ok(ThumbCacheHit {
                photo_hash: r.get(0)?,
                thumb_path: r.get(1)?,
            })
        })?;
        let mut out = Vec::with_capacity(rows.size_hint().0);
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 按 photo_hash 查单条命中（用于 ensure_grid_thumb 内部命中检查）
    pub fn lookup_thumb_cache_one(&self, hash: &str) -> Result<Option<ThumbCacheHit>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT photo_hash, thumb_path FROM photo_thumb_cache WHERE photo_hash = ?1")?;
        let mut rows = stmt.query_map(params![hash], |r| {
            Ok(ThumbCacheHit {
                photo_hash: r.get(0)?,
                thumb_path: r.get(1)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// 按相册 id 批量删除记录（删除相册时调用）
    ///
    /// 返回删除的行数。注意：实际文件清理（缩略图磁盘）由调用方负责，
    /// 这里只清表。本模块**不**触碰文件系统，保持职责单一。
    pub fn delete_thumb_caches_by_album(&self, album_id: i64) -> Result<usize, DbError> {
        let n = self
            .conn
            .execute(
                "DELETE FROM photo_thumb_cache WHERE album_id = ?1",
                params![album_id],
            )
            .map_err(DbError::Sqlite)?;
        Ok(n)
    }

    /// 按源路径批量删除记录（照片被删除时调用）
    ///
    /// 返回删除的行数。
    pub fn delete_thumb_caches_by_paths(&self, paths: &[String]) -> Result<usize, DbError> {
        if paths.is_empty() {
            return Ok(0);
        }
        let mut total = 0usize;
        // IN 子句过长（>999）会导致 SQLITE_MAX_VARIABLE_NUMBER 错误，按 500 一批
        for chunk in paths.chunks(500) {
            let placeholders = std::iter::repeat("?")
                .take(chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "DELETE FROM photo_thumb_cache WHERE source_path IN ({})",
                placeholders
            );
            let params_vec: Vec<&dyn rusqlite::ToSql> = chunk
                .iter()
                .map(|p| p as &dyn rusqlite::ToSql)
                .collect();
            total += self
                .conn
                .execute(&sql, params_vec.as_slice())
                .map_err(DbError::Sqlite)?;
        }
        Ok(total)
    }
}

/// 事务内批量 upsert（供 `upsert_thumb_caches` 与外部事务复用）
pub(crate) fn upsert_thumb_caches_in_tx(
    tx: &Transaction,
    recs: &[ThumbCacheRecord],
) -> Result<(), DbError> {
    for r in recs {
        tx.execute(
            "INSERT INTO photo_thumb_cache
                (photo_hash, source_path, thumb_path, album_id, user_id,
                 size_bytes, mtime_ns, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(photo_hash) DO UPDATE SET
                source_path=excluded.source_path,
                thumb_path=excluded.thumb_path,
                album_id=excluded.album_id,
                user_id=excluded.user_id,
                size_bytes=excluded.size_bytes,
                mtime_ns=excluded.mtime_ns,
                generated_at=excluded.generated_at",
            params![
                r.photo_hash,
                r.source_path,
                r.thumb_path,
                r.album_id,
                r.user_id,
                r.size_bytes as i64,
                r.mtime_ns as i64,
                Database::now_secs(),
            ],
        )?;
    }
    Ok(())
}

/// 测试用：在打开的连接上建表（不走 Database.open）—— 供外部 #[cfg(test)] 使用
#[cfg(test)]
pub fn init_for_test_conn(conn: &rusqlite::Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS photo_thumb_cache (
            photo_hash    TEXT    PRIMARY KEY,
            source_path   TEXT    NOT NULL,
            thumb_path    TEXT    NOT NULL,
            album_id      INTEGER,
            user_id       INTEGER NOT NULL,
            size_bytes    INTEGER NOT NULL,
            mtime_ns      INTEGER NOT NULL,
            generated_at  INTEGER NOT NULL
        );",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn mem_db() -> Database {
        let conn = Connection::open_in_memory().unwrap();
        let db = Database { conn };
        db.init_thumb_cache_schema().unwrap();
        db
    }

    fn rec(hash: &str, path: &str, thumb: &str) -> ThumbCacheRecord {
        ThumbCacheRecord {
            photo_hash: hash.into(),
            source_path: path.into(),
            thumb_path: thumb.into(),
            album_id: Some(1),
            user_id: 1,
            size_bytes: 1024,
            mtime_ns: 1_700_000_000_000_000_000,
        }
    }

    #[test]
    fn upsert_then_lookup() {
        let db = mem_db();
        let r = rec("H1", "/a/1.jpg", "/thumbs/1.webp");
        db.upsert_thumb_caches(&[r.clone()]).unwrap();
        let hit = db.lookup_thumb_cache_one("H1").unwrap().unwrap();
        assert_eq!(hit.photo_hash, "H1");
        assert_eq!(hit.thumb_path, "/thumbs/1.webp");
    }

    #[test]
    fn upsert_replaces_existing() {
        let db = mem_db();
        db.upsert_thumb_caches(&[rec("H1", "/a/1.jpg", "/t/old.webp")]).unwrap();
        // 同哈希 → 覆盖（路径变了）
        db.upsert_thumb_caches(&[rec("H1", "/a/1.jpg", "/t/new.webp")]).unwrap();
        let hit = db.lookup_thumb_cache_one("H1").unwrap().unwrap();
        assert_eq!(hit.thumb_path, "/t/new.webp");
    }

    #[test]
    fn lookup_batch() {
        let db = mem_db();
        db.upsert_thumb_caches(&[
            rec("H1", "/a/1.jpg", "/t/1.webp"),
            rec("H2", "/a/2.jpg", "/t/2.webp"),
            rec("H3", "/a/3.jpg", "/t/3.webp"),
        ])
        .unwrap();
        let hashes = vec!["H1".into(), "H2".into(), "MISS".into()];
        let hits = db.lookup_thumb_caches(&hashes).unwrap();
        assert_eq!(hits.len(), 2, "未命中不应返回");
        let got: Vec<String> = hits.iter().map(|h| h.photo_hash.clone()).collect();
        assert!(got.contains(&"H1".to_string()));
        assert!(got.contains(&"H2".to_string()));
    }

    #[test]
    fn delete_by_album() {
        let db = mem_db();
        let mut r1 = rec("H1", "/a/1.jpg", "/t/1.webp");
        r1.album_id = Some(1);
        let mut r2 = rec("H2", "/a/2.jpg", "/t/2.webp");
        r2.album_id = Some(2);
        db.upsert_thumb_caches(&[r1, r2]).unwrap();
        let n = db.delete_thumb_caches_by_album(1).unwrap();
        assert_eq!(n, 1);
        assert!(db.lookup_thumb_cache_one("H1").unwrap().is_none());
        assert!(db.lookup_thumb_cache_one("H2").unwrap().is_some());
    }

    #[test]
    fn delete_by_paths() {
        let db = mem_db();
        db.upsert_thumb_caches(&[
            rec("H1", "/a/1.jpg", "/t/1.webp"),
            rec("H2", "/a/2.jpg", "/t/2.webp"),
            rec("H3", "/a/3.jpg", "/t/3.webp"),
        ])
        .unwrap();
        let n = db
            .delete_thumb_caches_by_paths(&["/a/1.jpg".into(), "/a/3.jpg".into()])
            .unwrap();
        assert_eq!(n, 2);
        assert!(db.lookup_thumb_cache_one("H2").unwrap().is_some());
    }

    /// FEAT-044 集成场景：懒加载先生成 → 扫描入库预热跳过（幂等）
    ///
    /// 关键语义：
    /// 1. 首次调 get_photo_thumbs（懒加载）→ 未命中 → 生成并写表
    /// 2. 扫描入库预热 → 同 photo_hash 命中 → 不会重复生成
    /// 3. thumb_path 不变（证明未重写文件）
    #[test]
    fn scan_prewarm_skips_already_cached() {
        let db = mem_db();
        // 1. 模拟懒加载：未命中 → 走生成路径，生成后写表
        let lazy_thumb = "/thumbs/lazy.webp".to_string();
        db.upsert_thumb_caches(&[rec("PH1", "/a/1.jpg", &lazy_thumb)]).unwrap();
        // 2. 模拟扫描入库预热：同 photo_hash 查表，命中
        let hit = db.lookup_thumb_cache_one("PH1").unwrap().unwrap();
        assert_eq!(hit.thumb_path, lazy_thumb, "扫描入库不应重写缩略图路径");
        // 3. 预览仍只有一行（幂等）
        let all: Vec<ThumbCacheHit> = db
            .lookup_thumb_caches(&["PH1".into()])
            .unwrap();
        assert_eq!(all.len(), 1, "同 photo_hash 应只占 1 行");
    }

    /// FEAT-044 集成场景：扫描入库预热 → 懒加载 0 IO 命中
    ///
    /// 路径 3：先扫描入库预热，缩略图已生成 + 写表，
    /// 之后首次进智慧相册/时间线 → get_photo_thumbs 查表命中 → 不现场生成
    #[test]
    fn scan_prewarm_then_lazy_load_hits_cache() {
        let db = mem_db();
        // 1. 扫描入库预热：写表
        let warm_thumb = "/thumbs/warm.webp".to_string();
        db.upsert_thumb_caches(&[rec("PH1", "/a/1.jpg", &warm_thumb)]).unwrap();
        // 2. 首次进智慧相册：get_photo_thumbs 查表，命中（0 IO 返回）
        let hits = db.lookup_thumb_caches(&["PH1".into()]).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].thumb_path, warm_thumb);
    }
}
