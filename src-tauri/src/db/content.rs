//! 内容扫描表持久层（FEAT-022：AI 内容扫描入库 + 照片智能搜索）
//!
//! 对应 schema.sql 的 `photo_content_scan` 表：
//! - `photo_hash`：照片唯一标定（路径+大小+修改时间 组合哈希，见 content.rs `photo_hash`）。
//!   唯一键保证多次扫描不产生重复行；二次扫描以 `INSERT ... ON CONFLICT` 按哈希覆盖更新。
//! - 预留 EXIF 字段：时间 / 地点 / 快门速度 / ISO / 光圈 / 焦段（扫描时一并填充，二次扫描覆盖）。
//! - 索引：以「父目录 + 绝对地址」优化（另有 user_id / album_id 隔离索引）。
//! - 多用户隔离：所有查询限定 `user_id`，可再按 `album_id` 限定单相册范围。
//!
//! 本模块只做持久化（建表/写入/查询），扫描编排与哈希计算在 `content.rs` 服务层完成，
//! 保持分层解耦、单文件轻量。

use rusqlite::{params, Transaction};
use serde::Serialize;

use super::{DbError, Database};

/// 待写入的内容扫描记录（一次扫描一行，按 photo_hash upsert）
///
/// `person_ids` / `top3_json` 以 JSON 文本存库（其余为标量），读取时反序列化。
pub struct PhotoContentRecord {
    pub photo_hash: String,
    pub path: String,
    pub parent_dir: String,
    pub album_id: Option<i64>,
    pub user_id: i64,
    /// 聚合的可搜索文本（大类+细类+label+人物标号，搜索主字段）
    pub content: String,
    pub category: Option<String>,
    pub sub_category: Option<String>,
    pub label: Option<String>,
    pub confidence: Option<f64>,
    pub top3_json: Option<String>,
    pub person_ids: Option<String>,
    pub person_count: i64,
    // ---- 预留 EXIF 字段 ----
    pub shoot_time: Option<String>,
    pub location: Option<String>,
    pub shutter_speed: Option<String>,
    pub iso: Option<String>,
    pub aperture: Option<String>,
    pub focal_length: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

/// 内容搜索结果（命中照片 + 所属相册信息，供前端跳转）
#[derive(Debug, Clone, Serialize)]
pub struct ContentSearchHit {
    pub id: i64,
    /// 照片绝对路径
    pub path: String,
    /// 父目录绝对路径
    pub parent_dir: String,
    /// 归属相册 ID（可能为空）
    pub album_id: Option<i64>,
    /// 归属相册名称（LEFT JOIN albums，可能为空）
    pub album_name: Option<String>,
    /// 归属相册路径（供前端定位/打开）
    pub album_path: Option<String>,
    /// 聚合可搜索文本
    pub content: String,
    pub category: Option<String>,
    pub sub_category: Option<String>,
    pub label: Option<String>,
    pub confidence: Option<f64>,
    /// 人物标号（JSON 反序列化后的列表）
    pub person_ids: Vec<String>,
    pub shoot_time: Option<String>,
    pub location: Option<String>,
    pub iso: Option<String>,
    pub aperture: Option<String>,
    pub shutter_speed: Option<String>,
    pub focal_length: Option<String>,
}

impl Database {
    /// 建表/迁移内容扫描表（`IF NOT EXISTS`，应用启动安全调用）
    pub fn init_content_schema(&self) -> Result<(), DbError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS photo_content_scan (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                photo_hash   TEXT    NOT NULL UNIQUE,
                path         TEXT    NOT NULL,
                parent_dir   TEXT    NOT NULL,
                album_id     INTEGER,
                user_id      INTEGER NOT NULL,
                content      TEXT,
                category     TEXT,
                sub_category TEXT,
                label        TEXT,
                confidence   REAL,
                top3_json    TEXT,
                person_ids   TEXT,
                person_count INTEGER DEFAULT 0,
                shoot_time   TEXT,
                location     TEXT,
                shutter_speed TEXT,
                iso          TEXT,
                aperture     TEXT,
                focal_length TEXT,
                lat          REAL,
                lon          REAL,
                scanned_at   INTEGER NOT NULL
            );",
        )?;
        // 索引：父目录 + 绝对地址（需求 R3），另加 user_id / album_id 隔离索引
        let _ = self
            .conn
            .execute_batch("CREATE INDEX IF NOT EXISTS idx_pcs_parent ON photo_content_scan(parent_dir);");
        let _ = self
            .conn
            .execute_batch("CREATE INDEX IF NOT EXISTS idx_pcs_path ON photo_content_scan(path);");
        let _ = self
            .conn
            .execute_batch("CREATE INDEX IF NOT EXISTS idx_pcs_user ON photo_content_scan(user_id);");
        let _ = self
            .conn
            .execute_batch("CREATE INDEX IF NOT EXISTS idx_pcs_album ON photo_content_scan(album_id);");
        Ok(())
    }

    /// 写入/覆盖一条内容扫描记录（按 photo_hash 唯一标定）
    ///
    /// 二次扫描同哈希 → 用新结果覆盖（以二次扫描为准，需求 R2）。
    pub fn upsert_photo_content(&self, rec: &PhotoContentRecord) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO photo_content_scan
                (photo_hash, path, parent_dir, album_id, user_id, content,
                 category, sub_category, label, confidence, top3_json, person_ids, person_count,
                 shoot_time, location, shutter_speed, iso, aperture, focal_length, lat, lon, scanned_at)
             VALUES (?1,?2,?3,?4,?5,?6, ?7,?8,?9,?10,?11,?12,?13, ?14,?15,?16,?17,?18,?19,?20,?21,?22)
             ON CONFLICT(photo_hash) DO UPDATE SET
                 path=excluded.path, parent_dir=excluded.parent_dir, album_id=excluded.album_id,
                 content=excluded.content, category=excluded.category, sub_category=excluded.sub_category,
                 label=excluded.label, confidence=excluded.confidence, top3_json=excluded.top3_json,
                 person_ids=excluded.person_ids, person_count=excluded.person_count,
                 shoot_time=excluded.shoot_time, location=excluded.location,
                 shutter_speed=excluded.shutter_speed, iso=excluded.iso, aperture=excluded.aperture,
                 focal_length=excluded.focal_length, lat=excluded.lat, lon=excluded.lon,
                 scanned_at=excluded.scanned_at",
            params![
                rec.photo_hash, rec.path, rec.parent_dir, rec.album_id, rec.user_id, rec.content,
                rec.category, rec.sub_category, rec.label, rec.confidence, rec.top3_json,
                rec.person_ids, rec.person_count,
                rec.shoot_time, rec.location, rec.shutter_speed, rec.iso, rec.aperture,
                rec.focal_length, rec.lat, rec.lon,
                Database::now_secs(),
            ],
        )?;
        Ok(())
    }

    /// 批量写入内容扫描记录（同一事务，原子性）
    pub fn upsert_photo_contents(&self, recs: &[PhotoContentRecord]) -> Result<(), DbError> {
        let tx: Transaction = self.conn.unchecked_transaction()?;
        for rec in recs {
            upsert_one(&tx, rec)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 按关键词搜索照片内容（智能搜索）
    ///
    /// - `user_id`：多用户隔离（硬性限定）
    /// - `album_id`：`Some` → 单相册内部搜索；`None` → 群相册/全局搜索（需求 R4）
    /// - 匹配字段：聚合 `content`（含大类/细类/label/人物标号）
    pub fn search_photo_content(
        &self,
        keyword: &str,
        user_id: i64,
        album_id: Option<i64>,
    ) -> Result<Vec<ContentSearchHit>, DbError> {
        let kw = format!("%{}%", keyword.trim());
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.path, p.parent_dir, p.album_id, a.name, a.path,
                    p.content, p.category, p.sub_category, p.label, p.confidence,
                    p.person_ids, p.shoot_time, p.location, p.iso, p.aperture,
                    p.shutter_speed, p.focal_length
             FROM photo_content_scan p
             LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
             WHERE p.user_id = ?1
               AND (?2 IS NULL OR p.album_id = ?2)
               AND p.content LIKE ?3
             ORDER BY p.scanned_at DESC, p.id DESC",
        )?;
        let rows = stmt.query_map(params![user_id, album_id, kw], |r| {
            Ok(ContentSearchHit {
                id: r.get(0)?,
                path: r.get(1)?,
                parent_dir: r.get(2)?,
                album_id: r.get(3)?,
                album_name: r.get(4)?,
                album_path: r.get(5)?,
                content: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                category: r.get(7)?,
                sub_category: r.get(8)?,
                label: r.get(9)?,
                confidence: r.get(10)?,
                person_ids: r
                    .get::<_, Option<String>>(11)?
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default(),
                shoot_time: r.get(12)?,
                location: r.get(13)?,
                iso: r.get(14)?,
                aperture: r.get(15)?,
                shutter_speed: r.get(16)?,
                focal_length: r.get(17)?,
            })
        })?;
        let mut out: Vec<ContentSearchHit> = rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)?;
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// 删除某个相册的全部内容扫描记录（删除相册时级联调用）
    pub fn delete_album_content(&self, album_id: i64) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM photo_content_scan WHERE album_id = ?1", params![album_id])?;
        Ok(())
    }
}

/// 事务内写入一条记录（供批量 upsert 复用）
fn upsert_one(tx: &Transaction, rec: &PhotoContentRecord) -> Result<(), DbError> {
    tx.execute(
        "INSERT INTO photo_content_scan
            (photo_hash, path, parent_dir, album_id, user_id, content,
             category, sub_category, label, confidence, top3_json, person_ids, person_count,
             shoot_time, location, shutter_speed, iso, aperture, focal_length, lat, lon, scanned_at)
         VALUES (?1,?2,?3,?4,?5,?6, ?7,?8,?9,?10,?11,?12,?13, ?14,?15,?16,?17,?18,?19,?20,?21,?22)
         ON CONFLICT(photo_hash) DO UPDATE SET
             path=excluded.path, parent_dir=excluded.parent_dir, album_id=excluded.album_id,
             content=excluded.content, category=excluded.category, sub_category=excluded.sub_category,
             label=excluded.label, confidence=excluded.confidence, top3_json=excluded.top3_json,
             person_ids=excluded.person_ids, person_count=excluded.person_count,
             shoot_time=excluded.shoot_time, location=excluded.location,
             shutter_speed=excluded.shutter_speed, iso=excluded.iso, aperture=excluded.aperture,
             focal_length=excluded.focal_length, lat=excluded.lat, lon=excluded.lon,
             scanned_at=excluded.scanned_at",
        params![
            rec.photo_hash, rec.path, rec.parent_dir, rec.album_id, rec.user_id, rec.content,
            rec.category, rec.sub_category, rec.label, rec.confidence, rec.top3_json,
            rec.person_ids, rec.person_count,
            rec.shoot_time, rec.location, rec.shutter_speed, rec.iso, rec.aperture,
            rec.focal_length, rec.lat, rec.lon,
            Database::now_secs(),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn mem_db() -> Database {
        let conn = Connection::open_in_memory().unwrap();
        let db = Database { conn };
        db.init_schema().unwrap();
        db.init_content_schema().unwrap();
        db
    }

    fn sample_rec(hash: &str, path: &str) -> PhotoContentRecord {
        PhotoContentRecord {
            photo_hash: hash.into(),
            path: path.into(),
            parent_dir: "/x".into(),
            album_id: Some(1),
            user_id: 1,
            content: "动物 狗 golden retriever P001".into(),
            category: Some("animal".into()),
            sub_category: Some("dog".into()),
            label: Some("golden retriever".into()),
            confidence: Some(0.9),
            top3_json: Some(r#"[{"category":"animal","label":"dog","confidence":0.9}]"#.into()),
            person_ids: Some(r#"["P001"]"#.into()),
            person_count: 1,
            shoot_time: Some("2023-01-15 10:30:00".into()),
            location: Some("四川省 · 达州市".into()),
            shutter_speed: Some("1/200s".into()),
            iso: Some("100".into()),
            aperture: Some("f/2.8".into()),
            focal_length: Some("50mm".into()),
            lat: Some(31.921282),
            lon: Some(107.6375),
        }
    }

    #[test]
    fn upsert_dedup_by_hash_second_wins() {
        let db = mem_db();
        let r1 = sample_rec("HASH1", "/x/a.jpg");
        db.upsert_photo_content(&r1).unwrap();
        // 同哈希二次扫描 → 覆盖，不新增行
        let r2 = PhotoContentRecord { label: Some("labrador".into()), ..r1 };
        db.upsert_photo_content(&r2).unwrap();
        // content 仍含 "狗"，应命中 1 行，且 label 为二次扫描结果
        let hits = db.search_photo_content("狗", 1, None).unwrap();
        assert_eq!(hits.len(), 1, "同哈希应只保留一行（二次结果覆盖）");
        assert_eq!(hits[0].label.as_deref(), Some("labrador"));
        // 不同哈希 → 独立一行
        db.upsert_photo_content(&sample_rec("HASH2", "/x/b.jpg")).unwrap();
        let all = db.search_photo_content("狗", 1, None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn search_scope_user_and_album() {
        let db = mem_db();
        db.upsert_photo_content(&sample_rec("A", "/alb1/a.jpg")).unwrap();
        // 用户隔离：user_id=2 看不到
        assert!(db.search_photo_content("狗", 2, None).unwrap().is_empty());
        // 单相册范围：album_id=2 看不到 album_id=1 的记录
        assert!(db.search_photo_content("狗", 1, Some(2)).unwrap().is_empty());
        // 正确范围命中
        let hits = db.search_photo_content("狗", 1, Some(1)).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "/alb1/a.jpg");
    }

    #[test]
    fn delete_album_content() {
        let db = mem_db();
        db.upsert_photo_content(&sample_rec("A", "/alb1/a.jpg")).unwrap();
        db.delete_album_content(1).unwrap();
        assert!(db.search_photo_content("狗", 1, None).unwrap().is_empty());
    }
}
