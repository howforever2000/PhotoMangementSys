//! 照片语义向量持久层（FEAT-SEM：语义搜索 embedding 入库 + 阈值检索）
//!
//! 对应表 `photo_embeddings`：
//! - `photo_hash`：与 `photo_content_scan.photo_hash` 同一口径
//!   （路径+大小+修改时间组合哈希，见 `content.rs::photo_hash`），主键保证幂等 upsert。
//! - `embedding`：Chinese-CLIP fp16 编码的 512 维向量，f32 小端 BLOB（2KB/张）。
//!   服务端已 L2 归一化，检索端直接点积 = 余弦相似度。
//! - `path` / `album_id` / `user_id`：冗余列，使删除/移动的级联清理与
//!   `photo_content_scan` 完全同构（`delete_content_by_paths` /
//!   `delete_album_refs` / `move_photo_content_path` 三处同步维护）。
//!
//! 本模块只做持久化（建表/批量 upsert/查询/级联删除），扫描编排与服务调用在
//! `content.rs` / `vision.rs` 完成，保持分层解耦。

use std::collections::HashSet;

use rusqlite::{params, Transaction};
use serde::Serialize;

use super::{DbError, Database};

/// 待写入的向量记录（一批一事务，按 photo_hash upsert）
#[derive(Debug, Clone)]
pub struct EmbeddingRecord {
    pub photo_hash: String,
    pub user_id: i64,
    pub album_id: Option<i64>,
    pub path: String,
    pub dim: i64,
    pub model: String,
    /// 512 个 f32（服务端已归一化）
    pub embedding: Vec<f32>,
    /// ISO8601 字符串（写入时间）
    pub generated_at: String,
}

/// 向量缓存版本信号：count 与 max(rowid) 任一变化即需重载内存缓存；
/// v5 追加 model —— 换语义档位后向量空间不同，必须整体失效重建
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EmbeddingVersion {
    pub count: i64,
    pub max_rowid: i64,
    pub model: String,
}

impl Database {
    /// 建表（FEAT-SEM）：`IF NOT EXISTS` 启动安全调用
    pub fn init_embedding_schema(&self) -> Result<(), DbError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS photo_embeddings (
                photo_hash    TEXT    PRIMARY KEY,
                user_id       INTEGER NOT NULL,
                album_id      INTEGER,
                path          TEXT    NOT NULL,
                dim           INTEGER NOT NULL,
                model         TEXT    NOT NULL,
                embedding     BLOB    NOT NULL,
                generated_at  TEXT    NOT NULL
            );",
        )?;
        let _ = self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_pe_album ON photo_embeddings(album_id);
             CREATE INDEX IF NOT EXISTS idx_pe_user ON photo_embeddings(user_id);
             CREATE INDEX IF NOT EXISTS idx_pe_path ON photo_embeddings(path);",
        );
        Ok(())
    }

    /// 批量 upsert（按 photo_hash 主键，500/批事务由调用方控制）
    pub fn upsert_embeddings(&self, recs: &[EmbeddingRecord]) -> Result<(), DbError> {
        if recs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for rec in recs {
            upsert_embedding_in_tx(&tx, rec)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 查询已存在向量的 hash 集合（增量索引跳过用）；按 model 隔离（换档需重算）
    pub fn lookup_embedding_hashes(
        &self,
        hashes: &[String],
        model: &str,
    ) -> Result<HashSet<String>, DbError> {
        let mut out = HashSet::new();
        if hashes.is_empty() {
            return Ok(out);
        }
        let placeholders = std::iter::repeat("?").take(hashes.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT photo_hash FROM photo_embeddings WHERE model = ? AND photo_hash IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut pv: Vec<&dyn rusqlite::ToSql> = vec![&model];
        pv.extend(hashes.iter().map(|h| h as &dyn rusqlite::ToSql));
        let rows = stmt.query_map(pv.as_slice(), |r| r.get::<_, String>(0))?;
        for r in rows {
            out.insert(r?);
        }
        Ok(out)
    }

    /// 该用户全量向量（内存缓存构建用，含 album_id 与 path 供相册过滤 / 分类命中级联）
    ///
    /// v5：按 model 过滤 —— 换语义档位后旧向量维度/空间不同，绝不能混用。
    pub fn load_all_embeddings(
        &self,
        user_id: i64,
        model: &str,
    ) -> Result<Vec<(String, Option<i64>, String, Vec<f32>)>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT photo_hash, album_id, path, embedding FROM photo_embeddings
             WHERE user_id = ?1 AND model = ?2",
        )?;
        let rows = stmt.query_map(params![user_id, model], |r| {
            let hash: String = r.get(0)?;
            let album: Option<i64> = r.get(1)?;
            let path: String = r.get(2)?;
            let blob: Vec<u8> = r.get(3)?;
            Ok((hash, album, path, f32_vec_from_blob(&blob)))
        })?;
        rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
    }

    /// 缓存版本信号（轻查询，/search 高频调用）；model 变化即视为失效
    pub fn embedding_version(&self, user_id: i64, model: &str) -> Result<EmbeddingVersion, DbError> {
        self.conn.query_row(
            "SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM photo_embeddings
             WHERE user_id = ?1 AND model = ?2",
            params![user_id, model],
            |r| {
                Ok(EmbeddingVersion {
                    count: r.get(0)?,
                    max_rowid: r.get(1)?,
                    model: model.to_string(),
                })
            },
        )
        .map_err(DbError::Sqlite)
    }

    /// 按绝对路径批量删除（照片删除级联，对齐 delete_content_by_paths）
    pub fn delete_embeddings_by_paths(&self, paths: &[String]) -> Result<usize, DbError> {
        let mut n = 0usize;
        for p in paths {
            n += self
                .conn
                .execute("DELETE FROM photo_embeddings WHERE path = ?1", params![p])
                .map_err(DbError::Sqlite)?;
        }
        Ok(n)
    }

    /// 移动照片时同步 path（对齐 move_photo_content_path）
    pub fn move_photo_embedding_path(
        &self,
        user_id: i64,
        old_path: &str,
        new_path: &str,
        album_id: i64,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE photo_embeddings SET path = ?1, album_id = ?2
             WHERE user_id = ?3 AND path = ?4",
            params![new_path, album_id, user_id, old_path],
        )?;
        Ok(())
    }
}

/// 相册删除级联（事务内调用，对齐 delete_album_refs）
pub fn delete_embeddings_by_album_in_tx(tx: &Transaction, album_id: i64) -> Result<(), DbError> {
    tx.execute("DELETE FROM photo_embeddings WHERE album_id = ?1", params![album_id])?;
    Ok(())
}

/// 事务内写一条向量
fn upsert_embedding_in_tx(tx: &Transaction, rec: &EmbeddingRecord) -> Result<(), DbError> {
    tx.execute(
        "INSERT INTO photo_embeddings
            (photo_hash, user_id, album_id, path, dim, model, embedding, generated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(photo_hash) DO UPDATE SET
             user_id=excluded.user_id, album_id=excluded.album_id, path=excluded.path,
             dim=excluded.dim, model=excluded.model, embedding=excluded.embedding,
             generated_at=excluded.generated_at",
        params![
            rec.photo_hash,
            rec.user_id,
            rec.album_id,
            rec.path,
            rec.dim,
            rec.model,
            f32_blob(&rec.embedding),
            rec.generated_at,
        ],
    )?;
    Ok(())
}

/// f32 小端 BLOB ↔ Vec<f32>（与 persons.rs 人脸质心同一编码约定）
pub fn f32_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn f32_vec_from_blob(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}
