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

use std::collections::HashMap;

use rusqlite::{params, Transaction};
use serde::Serialize;

use super::{DbError, Database};

/// FTS5 虚拟表索引损坏（SQLITE_CORRUPT_VTAB = SQLITE_CORRUPT(11) | (1 << 8) = 267）
///
/// `photo_content_fts` 是外部内容表（`content='photo_content_scan'`），索引由触发器维护。
/// 一旦索引与主表失步（典型场景：FTS5 为后续版本新增，旧库主表已有数据但索引为空），
/// UPDATE/DELETE 触发器执行的 `'delete'` 命令会直接抛该错误，
/// 使**整批入库事务回滚**——这正是「全局照片扫描入库」报
/// `database disk image is malformed` 的根因。
const SQLITE_CORRUPT_VTAB: i32 = 267;

/// 是否为 FTS5 索引失步导致的虚拟表损坏错误（可被索引重建自愈）
fn is_fts_corrupt(e: &DbError) -> bool {
    matches!(
        e,
        DbError::Sqlite(rusqlite::Error::SqliteFailure(err, _))
            if err.extended_code == SQLITE_CORRUPT_VTAB
    )
}

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
    // FEAT-026：数值化 EXIF（供范围筛选）+ 影调类型 + 平均亮度
    pub iso_num: Option<u32>,
    pub focal_num: Option<f64>,
    pub aperture_num: Option<f64>,
    pub shutter_num: Option<f64>,
    pub tone_type: Option<String>,
    pub avg_luma: Option<f64>,
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

/// 单相册内容读表行（FEAT-026）：统一呈现 EXIF / 影调 / AI 全部字段
#[derive(Debug, Clone, Serialize)]
pub struct AlbumContentRow {
    pub id: i64,
    pub path: String,
    pub parent_dir: String,
    pub album_id: Option<i64>,
    pub album_name: Option<String>,
    pub album_path: Option<String>,
    // EXIF
    pub iso: Option<String>,
    pub aperture: Option<String>,
    pub shutter_speed: Option<String>,
    pub focal_length: Option<String>,
    pub shoot_time: Option<String>,
    // EXIF 数值版（供前端范围展示）
    pub iso_num: Option<u32>,
    pub focal_num: Option<f64>,
    pub aperture_num: Option<f64>,
    pub shutter_num: Option<f64>,
    // 影调
    pub tone_type: Option<String>,
    pub avg_luma: Option<f64>,
    // AI 内容
    pub content: String,
    pub category: Option<String>,
    pub sub_category: Option<String>,
    pub label: Option<String>,
    pub confidence: Option<f64>,
    pub top3_json: Option<String>,
    pub person_ids: Vec<String>,
    pub person_count: i64,
}

/// 智能搜索结果行（FEAT-034）：检索命中照片 + 展示所需字段（含 location）
#[derive(Debug, Clone, Serialize)]
pub struct SmartHit {
    pub id: i64,
    pub path: String,
    pub album_id: Option<i64>,
    pub album_name: Option<String>,
    pub category: Option<String>,
    pub sub_category: Option<String>,
    pub label: Option<String>,
    pub location: Option<String>,
    pub shoot_time: Option<String>,
    pub tone_type: Option<String>,
    pub person_ids: Vec<String>,
}

/// 内容搜索过滤条件（FEAT-026）：未设置（None）表示不启用该维度过滤
#[derive(Debug)]
pub struct ContentFilters {
    pub iso_min: Option<u32>,
    pub iso_max: Option<u32>,
    pub shutter_min: Option<f64>,
    pub shutter_max: Option<f64>,
    pub aperture_min: Option<f64>,
    pub aperture_max: Option<f64>,
    pub focal_min: Option<f64>,
    pub focal_max: Option<f64>,
    pub tone_type: Option<String>,
}

/// FEAT-048：内容分类两级聚合行（大类 → 细类，含计数与封面）
#[derive(Debug, Clone, Serialize)]
pub struct CategoryGroupRow {
    pub category: String,
    pub sub_category: Option<String>,
    pub count: i64,
    /// 该大类封面（置信度最高的一张原图路径；可能为 None）
    pub cover_path: Option<String>,
    /// 封面照片归属相册（get_photo_thumbs 复用真实相册缓存命名；可能为 None）
    pub cover_album_id: Option<i64>,
}

/// FEAT-049：地点聚合行（location = None 表示「未记录地点」组）
#[derive(Debug, Clone, Serialize)]
pub struct LocationGroupRow {
    /// 地名（geo_index 省/市级）；None = 无 GPS 或反查未命中（境外）
    pub location: Option<String>,
    pub count: i64,
    pub cover_path: Option<String>,
    pub cover_album_id: Option<i64>,
}

/// 内容命中行统一列清单（list_timeline / list_photos_by_category 共用，
/// 列序与 `map_content_search_hit` 严格对应）
const CONTENT_HIT_SELECT: &str = "SELECT p.id, p.path, p.parent_dir, p.album_id, a.name, a.path,
            p.content, p.category, p.sub_category, p.label, p.confidence,
            p.person_ids, p.shoot_time, p.location, p.iso, p.aperture,
            p.shutter_speed, p.focal_length
     FROM photo_content_scan p
     LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id";

/// 内容命中行统一映射（与 CONTENT_HIT_SELECT 列序严格对应）
fn map_content_search_hit(r: &rusqlite::Row) -> rusqlite::Result<ContentSearchHit> {
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
                user_tags    TEXT,
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
        // FEAT-026 新增列（数值 EXIF + 影调；旧库无这些列，IF NOT EXISTS 等价）
        let _ = self.conn.execute_batch("ALTER TABLE photo_content_scan ADD COLUMN iso_num INTEGER;");
        let _ = self.conn.execute_batch("ALTER TABLE photo_content_scan ADD COLUMN focal_num REAL;");
        let _ = self.conn.execute_batch("ALTER TABLE photo_content_scan ADD COLUMN aperture_num REAL;");
        // FEAT-050：用户标签（评分已有独立 photo_ratings 表，不重复建列）
        let _ = self
            .conn
            .execute_batch("ALTER TABLE photo_content_scan ADD COLUMN user_tags TEXT;");
        let _ = self.conn.execute_batch("ALTER TABLE photo_content_scan ADD COLUMN shutter_num REAL;");
        let _ = self.conn.execute_batch("ALTER TABLE photo_content_scan ADD COLUMN tone_type TEXT;");
        let _ = self.conn.execute_batch("ALTER TABLE photo_content_scan ADD COLUMN avg_luma REAL;");

        // P2 FTS5 全文索引：加速内容搜索（label/category/sub_category/person_ids）
        // FTS5 表通过 Porter stemming 分词（中文场景按字分），content 列同步主表
        let _ = self.conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS photo_content_fts USING fts5(
                photo_hash,
                content,
                tokenize='porter unicode61',
                content='photo_content_scan',
                content_rowid='id'
            );"
        );
        // 同步触发器：INSERT / UPDATE / DELETE 主表时同步 FTS5
        let _ = self.conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS pcs_fts_insert AFTER INSERT ON photo_content_scan BEGIN
                INSERT INTO photo_content_fts(rowid, photo_hash, content) VALUES (new.id, new.photo_hash, new.content);
             END;"
        );
        let _ = self.conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS pcs_fts_update AFTER UPDATE ON photo_content_scan BEGIN
                INSERT INTO photo_content_fts(photo_content_fts, rowid, photo_hash, content)
                    VALUES ('delete', old.id, old.photo_hash, old.content);
                INSERT INTO photo_content_fts(rowid, photo_hash, content)
                    VALUES (new.id, new.photo_hash, new.content);
             END;"
        );
        let _ = self.conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS pcs_fts_delete AFTER DELETE ON photo_content_scan BEGIN
                INSERT INTO photo_content_fts(photo_content_fts, rowid, photo_hash, content)
                    VALUES ('delete', old.id, old.photo_hash, old.content);
             END;"
        );
        // 启动自愈：索引与内容表失步时重建索引。
        // 不修复的话，后续任何一次扫描入库都会因 'delete' 未命中索引抛
        // SQLITE_CORRUPT_VTAB(267) 而整批失败（旧库升级后首次重扫必然触发）。
        // 失败也不阻断启动（写入路径还有一次兜底自愈）。
        let _ = self.repair_fts_index();
        Ok(())
    }

    /// 校验 FTS5 索引与内容表的一致性，不一致则从主表全量重建（自愈）
    ///
    /// 外部内容表（`content='photo_content_scan'`）的索引只由触发器维护，以下情况会失步：
    /// 1. FTS5 为后续版本引入：旧库主表已有数据，索引却是空的；
    /// 2. 索引曾损坏且未修复。
    ///
    /// 失步后再执行一次 `'delete'` 就会抛 SQLITE_CORRUPT_VTAB(267)，
    /// 导致所有写入（扫描入库）全部失败，因此必须在启动与写入失败时自愈。
    ///
    /// 主表为空时索引必然一致，直接跳过，避免每次启动的无谓开销。
    pub fn repair_fts_index(&self) -> Result<(), DbError> {
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM photo_content_scan", [], |r| r.get(0))
            .unwrap_or(0);
        if total == 0 {
            return Ok(());
        }
        // 索引真实条目数：外部内容表的 `SELECT COUNT(*) FROM photo_content_fts` 会退化为
        // 扫描内容表（实测恒等于主表行数，检测不出缺行），故查 docsize 影子表——
        // 每篇已索引文档一行，直接反映索引条目数，且为 O(1) 的 b-tree 计数（启动开销可忽略）。
        match self
            .conn
            .query_row("SELECT COUNT(*) FROM photo_content_fts_docsize", [], |r| r.get::<_, i64>(0))
        {
            Ok(n) if n == total => return Ok(()), // 快路径：行数一致即视为健康
            Ok(_) => {}                           // 缺行/多出 → 落到下方重建
            Err(_) => {
                // 影子表不可读（未来版本改名）：退化为 FTS5 自带完整性校验，
                // 通过则说明索引与内容表一致，可跳过重建。
                if self
                    .conn
                    .execute_batch(
                        "INSERT INTO photo_content_fts(photo_content_fts) VALUES('integrity-check');",
                    )
                    .is_ok()
                {
                    return Ok(());
                }
            }
        }
        // rebuild：以主表为准全量重建索引（幂等，数据不丢）
        self.conn
            .execute_batch("INSERT INTO photo_content_fts(photo_content_fts) VALUES('rebuild');")
            .map_err(DbError::Sqlite)?;
        Ok(())
    }

    /// 写入/覆盖一条内容扫描记录（按 photo_hash 唯一标定）
    ///
    /// 二次扫描同哈希 → 用新结果覆盖（以二次扫描为准，需求 R2）。
    /// 与批量写入共用同一条 UPSERT（含 FEAT-026 全部列），避免两条 SQL 行为分叉。
    pub fn upsert_photo_content(&self, rec: &PhotoContentRecord) -> Result<(), DbError> {
        self.upsert_photo_contents(std::slice::from_ref(rec))
    }

    /// 批量写入内容扫描记录（同一事务，原子性）
    ///
    /// FTS5 索引失步自愈：外部内容表的同步触发器在索引与主表不一致时会抛
    /// SQLITE_CORRUPT_VTAB(267)，使整批事务回滚、扫描入库整体失败。
    /// 这里捕获该错误 → 重建索引 → 重试一次；仍失败才向上抛。
    pub fn upsert_photo_contents(&self, recs: &[PhotoContentRecord]) -> Result<(), DbError> {
        match self.upsert_photo_contents_inner(recs) {
            Ok(()) => Ok(()),
            Err(e) if is_fts_corrupt(&e) => {
                let _ = self.repair_fts_index();
                self.upsert_photo_contents_inner(recs)
            }
            Err(e) => Err(e),
        }
    }

    /// 事务内批量写入（无自愈，供 `upsert_photo_contents` 重试复用）
    fn upsert_photo_contents_inner(&self, recs: &[PhotoContentRecord]) -> Result<(), DbError> {
        let tx: Transaction = self.conn.unchecked_transaction()?;
        for rec in recs {
            upsert_one(&tx, rec)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// P2 FTS5 全文搜索：利用 photo_content_fts 虚拟表加速关键词匹配
    /// FALLBACK: FTS5 表不存在/查询失败时降级为 LIKE 模糊搜索
    pub fn search_photo_content(
        &self,
        keyword: &str,
        user_id: i64,
        album_id: Option<i64>,
    ) -> Result<Vec<ContentSearchHit>, DbError> {
        // 尝试 FTS5 全文搜索
        let fts_kw = keyword.trim();
        if !fts_kw.is_empty() {
            if let Ok(hits) = self.search_photo_content_fts(fts_kw, user_id, album_id) {
                return Ok(hits);
            }
        }
        // FALLBACK: LIKE 模糊搜索（兼容无 FTS5 表的旧库）
        self.search_photo_content_like(keyword, user_id, album_id)
    }

    /// FTS5 全文搜索实现
    fn search_photo_content_fts(
        &self,
        keyword: &str,
        user_id: i64,
        album_id: Option<i64>,
    ) -> Result<Vec<ContentSearchHit>, DbError> {
        // FTS5 MATCH 支持 * 前缀匹配（label*），转为 rank/bm25 排序
        let fts_query = format!("{}{}",
            keyword.replace(' ', " OR "),
            if keyword.contains('*') { "" } else { "*" }
        );
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.path, p.parent_dir, p.album_id, a.name, a.path,
                    p.content, p.category, p.sub_category, p.label, p.confidence,
                    p.person_ids, p.shoot_time, p.location, p.iso, p.aperture,
                    p.shutter_speed, p.focal_length
             FROM photo_content_scan p
             JOIN photo_content_fts f ON f.rowid = p.id
             LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
             WHERE f MATCH ?1
               AND p.user_id = ?2
               AND (?3 IS NULL OR p.album_id = ?3)
             ORDER BY rank",
        )?;
        let rows = stmt.query_map(params![fts_query, user_id, album_id], |r| {
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

    /// LIKE 模糊搜索（降级路径，兼容旧库）
    fn search_photo_content_like(
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
        rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
    }

    // ---- FEAT-033（dev-ai002）：跨相册照片时间线 ----

    /// 跨相册照片时间线：把当前用户所有已扫描且有拍摄时间的照片按时间升序返回（空时间置底）
    ///
    /// 复用 `ContentSearchHit`（含 path / album_name / shoot_time / location / category / label），
    /// 供前端按年·月分组展示。
    pub fn list_timeline(&self, user_id: i64) -> Result<Vec<ContentSearchHit>, DbError> {
        let sql = format!(
            "{CONTENT_HIT_SELECT} WHERE p.user_id = ?1
             ORDER BY (p.shoot_time IS NULL), p.shoot_time ASC, p.id ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![user_id], map_content_search_hit)?;
        rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
    }

    /// FEAT-048：按大类/细类列出照片（分类浏览二级视图）
    ///
    /// - `sub_category = None` → 该大类全部照片（含细类为空的行）
    /// - 复用时间线行结构（含相册名/拍摄时间等展示字段）；多用户隔离
    pub fn list_photos_by_category(
        &self,
        user_id: i64,
        category: &str,
        sub_category: Option<&str>,
    ) -> Result<Vec<ContentSearchHit>, DbError> {
        let sql = format!(
            "{CONTENT_HIT_SELECT} WHERE p.user_id = ?1 AND p.category = ?2
                AND (?3 IS NULL OR p.sub_category = ?3)
             ORDER BY (p.shoot_time IS NULL), p.shoot_time DESC, p.id DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows =
            stmt.query_map(params![user_id, category, sub_category], map_content_search_hit)?;
        rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
    }

    /// FEAT-048：内容分类两级聚合（大类 → 细类计数 + 各大类封面）
    ///
    /// - 行粒度 (category, sub_category)，前端聚合为大类卡片 + 细类 chips
    /// - 封面取该大类置信度最高的一张（窗口函数一次取全）
    /// - 多用户隔离
    pub fn list_content_categories(&self, user_id: i64) -> Result<Vec<CategoryGroupRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT category, sub_category, COUNT(*)
             FROM photo_content_scan
             WHERE user_id = ?1 AND category IS NOT NULL AND category != ''
             GROUP BY category, sub_category
             ORDER BY COUNT(*) DESC",
        )?;
        let mut rows: Vec<(String, Option<String>, i64)> = stmt
            .query_map(params![user_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<_, _>>()?;

        let mut cover_stmt = self.conn.prepare(
            "SELECT category, path, album_id FROM (
                 SELECT category, path, album_id,
                        ROW_NUMBER() OVER (PARTITION BY category ORDER BY confidence DESC) AS rn
                 FROM photo_content_scan
                 WHERE user_id = ?1 AND category IS NOT NULL AND category != ''
             ) WHERE rn = 1",
        )?;
        let covers: HashMap<String, (String, Option<i64>)> = cover_stmt
            .query_map(params![user_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?),
                ))
            })?
            .collect::<Result<_, _>>()?;

        rows.drain(..)
            .map(|(category, sub_category, count)| {
                let (cover_path, cover_album_id) = match covers.get(&category) {
                    Some((p, aid)) => (Some(p.clone()), *aid),
                    None => (None, None),
                };
                Ok(CategoryGroupRow {
                    category,
                    sub_category,
                    count,
                    cover_path,
                    cover_album_id,
                })
            })
            .collect()
    }

    /// FEAT-049：地点聚合（geo_index 离线反查 + 回写 location 列作持久缓存）
    ///
    /// location 列历史上全空（内容扫描管线未做地名反查），本方法：
    /// 1. location 已有值 → 直接计入；
    /// 2. location 为空但 lat/lon 存在 → geo_index 离线反查省/市（万张 <1s），
    ///    计入并**事务回写 location 列**（下次调用 0 反查成本；回写失败不阻塞展示）；
    /// 3. 无 GPS / 反查未命中（境外）→ 归入「未记录地点」（location = None）。
    ///
    /// 返回按 count 降序，「未记录地点」固定排最后。多用户隔离。
    pub fn list_photo_locations(&self, user_id: i64) -> Result<Vec<LocationGroupRow>, DbError> {
        struct Row {
            photo_hash: String,
            path: String,
            album_id: Option<i64>,
            location: Option<String>,
            lat: Option<f64>,
            lon: Option<f64>,
            confidence: Option<f64>,
        }
        let mut stmt = self.conn.prepare(
            "SELECT photo_hash, path, album_id, location, lat, lon, confidence
             FROM photo_content_scan WHERE user_id = ?1",
        )?;
        let rows: Vec<Row> = stmt
            .query_map(params![user_id], |r| {
                Ok(Row {
                    photo_hash: r.get(0)?,
                    path: r.get(1)?,
                    album_id: r.get(2)?,
                    location: r.get(3)?,
                    lat: r.get(4)?,
                    lon: r.get(5)?,
                    confidence: r.get(6)?,
                })
            })?
            .collect::<Result<_, _>>()?;

        // 聚合：key = Some(地名) / None(未记录)；值 = (count, best_conf, cover, cover_album)
        let mut agg: HashMap<Option<String>, (i64, Option<f64>, Option<String>, Option<i64>)> =
            HashMap::new();
        let mut to_write: Vec<(String, String)> = Vec::new();
        for r in &rows {
            let mut name = r.location.clone().filter(|s| !s.trim().is_empty());
            if name.is_none() {
                if let (Some(lat), Some(lon)) = (r.lat, r.lon) {
                    if let Some(place) = crate::geo_index::find_region(lat, lon) {
                        name = Some(place.clone());
                        to_write.push((r.photo_hash.clone(), place));
                    }
                }
            }
            let e = agg.entry(name).or_insert((0, None, None, None));
            e.0 += 1;
            let conf = r.confidence.unwrap_or(-1.0);
            if e.1.map(|c| conf > c).unwrap_or(true) {
                e.1 = Some(conf);
                e.2 = Some(r.path.clone());
                e.3 = r.album_id;
            }
        }

        // 回写 location（事务；单行失败忽略，下次再补）
        if !to_write.is_empty() {
            if let Ok(tx) = self.conn.unchecked_transaction() {
                for (hash, place) in &to_write {
                    let _ = tx.execute(
                        "UPDATE photo_content_scan SET location = ?1 WHERE photo_hash = ?2",
                        params![place, hash],
                    );
                }
                let _ = tx.commit();
            }
        }

        let mut named: Vec<LocationGroupRow> = Vec::new();
        let mut unknown: Option<LocationGroupRow> = None;
        for (name, (count, _conf, cover, album)) in agg {
            let row = LocationGroupRow {
                location: name,
                count,
                cover_path: cover,
                cover_album_id: album,
            };
            match row.location {
                Some(_) => named.push(row),
                None => unknown = Some(row),
            }
        }
        named.sort_by(|a, b| b.count.cmp(&a.count));
        if let Some(u) = unknown {
            named.push(u);
        }
        Ok(named)
    }

    /// FEAT-049：按地点列出照片（地点浏览二级视图）
    ///
    /// `location = None` → 「未记录地点」组（location 列为空的全部照片）。
    pub fn list_photos_by_location(
        &self,
        user_id: i64,
        location: Option<&str>,
    ) -> Result<Vec<ContentSearchHit>, DbError> {
        let sql = match location {
            Some(_) => format!(
                "{CONTENT_HIT_SELECT} WHERE p.user_id = ?1 AND p.location = ?2
                 ORDER BY (p.shoot_time IS NULL), p.shoot_time DESC, p.id DESC"
            ),
            None => format!(
                "{CONTENT_HIT_SELECT} WHERE p.user_id = ?1
                    AND (p.location IS NULL OR p.location = '')
                 ORDER BY (p.shoot_time IS NULL), p.shoot_time DESC, p.id DESC"
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let mut pv: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];
        if let Some(loc) = location {
            pv.push(Box::new(loc.to_string()));
        }
        let params_ref: Vec<&dyn rusqlite::ToSql> = pv.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), map_content_search_hit)?;
        rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
    }

    /// FEAT-036：批量统计每个相册的已入库照片数。
    ///
    /// 返回 `HashMap<album_id, count>`。用于：
    /// 1. 给相册卡片标记「是否已入库」（count > 0）；
    /// 2. 智慧相册 Hero 聚合「已入库相册数」。
    /// 多用户隔离：仅统计当前用户的相册。
    /// FEAT-036：统计各相册已入库照片数（按相册目录子树前缀匹配，BUG-2026-0909-001 修复）
    ///
    /// 语义：照片文件位于相册目录（含子目录）下且在 photo_content_scan 有行 → 计数，
    /// 不论该行的 album_id 归属哪个相册。
    ///
    /// 为什么不用 album_id 分组统计：父子相册共享同一批照片（父目录递归包含子目录），
    /// 每张照片在表中只有一行、album_id 只有一个归属，谁最后重扫这批照片行就归谁 ——
    /// 「先扫父相册再扫子相册」后父相册计数骤减，UI 表现为「之前入库的照片变未入库」。
    /// 目录前缀统计与 photo_count（递归文件数）口径一致，父子相册各自都显示完整覆盖。
    ///
    /// 实现：单查询取当前用户全部已入库路径，Rust 侧前缀匹配（避免逐相册 LIKE N+1）。
    pub fn count_scanned_by_prefix(
        &self,
        user_id: i64,
        album_paths: &[(i64, String)],
    ) -> Result<HashMap<i64, i64>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM photo_content_scan WHERE user_id = ?1")?;
        let rows = stmt.query_map(params![user_id], |r| r.get::<_, String>(0))?;
        let mut paths: Vec<String> = Vec::new();
        for r in rows {
            paths.push(r?);
        }
        // 前缀规范化：确保以分隔符结尾，避免「川西合集」误匹配「川西合集备份」
        let prefixes: Vec<(i64, String)> = album_paths
            .iter()
            .map(|(id, p)| {
                let trimmed = p.trim_end_matches(['\\', '/']);
                let sep = if p.contains('/') { '/' } else { '\\' };
                (*id, format!("{trimmed}{sep}"))
            })
            .collect();
        let mut map = HashMap::new();
        for (id, prefix) in &prefixes {
            let cnt = paths.iter().filter(|p| p.starts_with(prefix.as_str())).count() as i64;
            map.insert(*id, cnt);
        }
        Ok(map)
    }

    /// 按 path 读取单张照片已扫描的内容（FEAT-D）
    ///
    /// 大图查看器在打开原图时调用：若 photo_content_scan 中已有该 path 的记录，
    /// 直接返回 AlbumContentRow；若无则返回 None（让上层调 ensure_photo_scanned 触发扫描）。
    /// 多用户隔离：限定 `user_id`。
    /// FEAT-050：设置用户标签（整体覆盖式保存）
    ///
    /// 规范化：去空白、去重、过滤空值，上限 20 个、单个 30 字；存 JSON 数组。
    pub fn set_photo_tags(
        &self,
        user_id: i64,
        path: &str,
        tags: &[String],
    ) -> Result<Vec<String>, DbError> {
        let mut clean: Vec<String> = Vec::new();
        for t in tags {
            let t = t.trim();
            if t.is_empty() || t.chars().count() > 30 {
                continue;
            }
            if !clean.iter().any(|x| x == t) {
                clean.push(t.to_string());
            }
        }
        clean.truncate(20);
        let json = serde_json::to_string(&clean).map_err(|e| DbError::Other(format!("序列化标签失败: {e}")))?;
        self.conn
            .execute(
                "UPDATE photo_content_scan SET user_tags = ?1 WHERE path = ?2 AND user_id = ?3",
                params![json, path, user_id],
            )
            .map_err(DbError::Sqlite)?;
        Ok(clean)
    }

    /// FEAT-050：读取单张照片的用户标签（预览组件自包含拉取用）
    ///
    /// 评分不在本方法内：已有 photo_ratings 表与 get_photo_ratings 命令，前端复用。
    pub fn get_photo_tags(&self, user_id: i64, path: &str) -> Result<Vec<String>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT user_tags FROM photo_content_scan WHERE path = ?1 AND user_id = ?2",
        )?;
        let mut rows = stmt.query_map(params![path, user_id], |r| {
            Ok(r.get::<_, Option<String>>(0)?
                .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                .unwrap_or_default())
        })?;
        match rows.next() {
            Some(r) => r.map_err(DbError::Sqlite),
            None => Ok(Vec::new()),
        }
    }

    pub fn get_photo_content_by_path(
        &self,
        path: &str,
        user_id: i64,
    ) -> Result<Option<AlbumContentRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.path, p.parent_dir, p.album_id, a.name, a.path,
                    p.iso, p.aperture, p.shutter_speed, p.focal_length, p.shoot_time,
                    p.iso_num, p.focal_num, p.aperture_num, p.shutter_num,
                    p.tone_type, p.avg_luma,
                    p.content, p.category, p.sub_category, p.label, p.confidence,
                    p.top3_json, p.person_ids, p.person_count
             FROM photo_content_scan p
             LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
             WHERE p.user_id = ?1 AND p.path = ?2
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![user_id, path], |r| {
            Ok(AlbumContentRow {
                id: r.get(0)?,
                path: r.get(1)?,
                parent_dir: r.get(2)?,
                album_id: r.get(3)?,
                album_name: r.get(4)?,
                album_path: r.get(5)?,
                iso: r.get(6)?,
                aperture: r.get(7)?,
                shutter_speed: r.get(8)?,
                focal_length: r.get(9)?,
                shoot_time: r.get(10)?,
                iso_num: r.get(11)?,
                focal_num: r.get(12)?,
                aperture_num: r.get(13)?,
                shutter_num: r.get(14)?,
                tone_type: r.get(15)?,
                avg_luma: r.get(16)?,
                content: r.get::<_, Option<String>>(17)?.unwrap_or_default(),
                category: r.get(18)?,
                sub_category: r.get(19)?,
                label: r.get(20)?,
                confidence: r.get(21)?,
                top3_json: r.get(22)?,
                person_ids: r.get::<_, Option<String>>(23)?
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default(),
                person_count: r.get(24)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row.map_err(DbError::Sqlite)?)),
            None => Ok(None),
        }
    }

    // ---- FEAT-026：统一读表 + 条件搜索（数值 EXIF + 影调）----

    /// 单相册内容读表（分页）：把该相册已扫描的全部记录读出
    ///
    /// 返回 `(rows, total)`：`rows` 为当前页数据（`page_size` 条），`total` 为总条数。
    pub fn read_album_content(
        &self,
        album_id: i64,
        user_id: i64,
        page: i64,
        page_size: i64,
    ) -> Result<(Vec<AlbumContentRow>, i64), DbError> {
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM photo_content_scan WHERE user_id = ?1 AND album_id = ?2",
            params![user_id, album_id],
            |r| r.get(0),
        )?;
        let offset = (page - 1) * page_size;
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.path, p.parent_dir, p.album_id, a.name, a.path,
                    p.iso, p.aperture, p.shutter_speed, p.focal_length, p.shoot_time,
                    p.iso_num, p.focal_num, p.aperture_num, p.shutter_num,
                    p.tone_type, p.avg_luma,
                    p.content, p.category, p.sub_category, p.label, p.confidence,
                    p.top3_json, p.person_ids, p.person_count
             FROM photo_content_scan p
             LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
             WHERE p.user_id = ?1 AND p.album_id = ?2
             ORDER BY p.scanned_at DESC, p.path ASC
             LIMIT ?3 OFFSET ?4",
        )?;
        let rows = stmt.query_map(params![user_id, album_id, page_size, offset], |r| {
            Ok(AlbumContentRow {
                id: r.get(0)?,
                path: r.get(1)?,
                parent_dir: r.get(2)?,
                album_id: r.get(3)?,
                album_name: r.get(4)?,
                album_path: r.get(5)?,
                iso: r.get(6)?,
                aperture: r.get(7)?,
                shutter_speed: r.get(8)?,
                focal_length: r.get(9)?,
                shoot_time: r.get(10)?,
                iso_num: r.get(11)?,
                focal_num: r.get(12)?,
                aperture_num: r.get(13)?,
                shutter_num: r.get(14)?,
                tone_type: r.get(15)?,
                avg_luma: r.get(16)?,
                content: r.get::<_, Option<String>>(17)?.unwrap_or_default(),
                category: r.get(18)?,
                sub_category: r.get(19)?,
                label: r.get(20)?,
                confidence: r.get(21)?,
                top3_json: r.get(22)?,
                person_ids: r.get::<_, Option<String>>(23)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
                person_count: r.get(24)?,
            })
        })?;
        Ok((
            rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)?,
            total,
        ))
    }

    // ---- FEAT-034（dev-ai002）：智能搜索（半自然语言解析 + 多维筛选）----

    /// 跨相册智能搜索：`keyword` 宽匹配 + 可选结构化筛选
    ///
    /// 命中字段：聚合内容 / 地点 / 标签 / 子类 / 相册名 / 文件名
    /// 可选筛选：时间区间 / 地点 / 大类 / 标签 / 人物标号 / 影调
    /// 返回 `Vec<AlbumContentRow>`（统一字段），按拍摄时间降序。
    #[allow(clippy::too_many_arguments)]
    pub fn smart_search(
        &self,
        user_id: i64,
        keyword: &str,
        date_from: Option<&str>,
        date_to: Option<&str>,
        location: Option<&str>,
        category: Option<&str>,
        label: Option<&str>,
        person_id: Option<&str>,
        tone_type: Option<&str>,
    ) -> Result<Vec<SmartHit>, DbError> {
        let mut sql = String::from(
            "SELECT p.id, p.path, p.album_id, a.name, p.category, p.sub_category, p.label,
                    p.location, p.shoot_time, p.tone_type, p.person_ids
             FROM photo_content_scan p
             LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
             WHERE p.user_id = ?1",
        );
        let mut binds: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Integer(user_id)];
        let mut n = 2usize;

        let kw = keyword.trim();
        if !kw.is_empty() {
            let like = format!("%{kw}%");
            sql.push_str(&format!(
                " AND (p.content LIKE ?{n} OR p.location LIKE ?{n} OR p.label LIKE ?{n} OR p.sub_category LIKE ?{n} OR a.name LIKE ?{n} OR p.path LIKE ?{n})"
            ));
            binds.push(rusqlite::types::Value::Text(like));
            n += 1;
        }
        if let Some(df) = date_from {
            if !df.trim().is_empty() {
                sql.push_str(&format!(" AND p.shoot_time >= ?{n}"));
                binds.push(rusqlite::types::Value::Text(df.trim().to_string()));
                n += 1;
            }
        }
        if let Some(dt) = date_to {
            if !dt.trim().is_empty() {
                let end = format!("{} 23:59:59", dt.trim());
                sql.push_str(&format!(" AND p.shoot_time <= ?{n}"));
                binds.push(rusqlite::types::Value::Text(end));
                n += 1;
            }
        }
        if let Some(loc) = location {
            if !loc.trim().is_empty() {
                let like = format!("%{}%", loc.trim());
                sql.push_str(&format!(" AND p.location LIKE ?{n}"));
                binds.push(rusqlite::types::Value::Text(like));
                n += 1;
            }
        }
        if let Some(cat) = category {
            if !cat.trim().is_empty() {
                sql.push_str(&format!(" AND p.category = ?{n}"));
                binds.push(rusqlite::types::Value::Text(cat.trim().to_string()));
                n += 1;
            }
        }
        if let Some(lb) = label {
            if !lb.trim().is_empty() {
                let like = format!("%{}%", lb.trim());
                sql.push_str(&format!(" AND p.label LIKE ?{n}"));
                binds.push(rusqlite::types::Value::Text(like));
                n += 1;
            }
        }
        if let Some(pid) = person_id {
            if !pid.trim().is_empty() {
                let like = format!("%{}%", pid.trim());
                sql.push_str(&format!(" AND p.person_ids LIKE ?{n}"));
                binds.push(rusqlite::types::Value::Text(like));
                n += 1;
            }
        }
        if let Some(tone) = tone_type {
            if !tone.trim().is_empty() {
                sql.push_str(&format!(" AND p.tone_type = ?{n}"));
                binds.push(rusqlite::types::Value::Text(tone.trim().to_string()));
            }
        }
        sql.push_str(" ORDER BY (p.shoot_time IS NULL), p.shoot_time DESC, p.id DESC");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(binds.iter()), |r| {
            Ok(SmartHit {
                id: r.get(0)?,
                path: r.get(1)?,
                album_id: r.get(2)?,
                album_name: r.get(3)?,
                category: r.get(4)?,
                sub_category: r.get(5)?,
                label: r.get(6)?,
                location: r.get(7)?,
                shoot_time: r.get(8)?,
                tone_type: r.get(9)?,
                person_ids: r.get::<_, Option<String>>(10)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
            })
        })?;
        rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
    }

    /// 带过滤条件的单相册内容搜索（FEAT-026）
    ///
    /// - `keyword`：空串 → 不启用关键词过滤
    /// - `filters`：数值范围 + 影调类型过滤（未设置即不限）
    pub fn search_photo_content_with_filters(
        &self,
        keyword: &str,
        user_id: i64,
        album_id: Option<i64>,
        filters: &ContentFilters,
    ) -> Result<Vec<AlbumContentRow>, DbError> {
        let kw = if keyword.trim().is_empty() {
            None
        } else {
            Some(format!("%{}%", keyword.trim()))
        };
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.path, p.parent_dir, p.album_id, a.name, a.path,
                    p.iso, p.aperture, p.shutter_speed, p.focal_length, p.shoot_time,
                    p.iso_num, p.focal_num, p.aperture_num, p.shutter_num,
                    p.tone_type, p.avg_luma,
                    p.content, p.category, p.sub_category, p.label, p.confidence,
                    p.top3_json, p.person_ids, p.person_count
             FROM photo_content_scan p
             LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
             WHERE p.user_id = ?1
               AND (?2 IS NULL OR p.album_id = ?2)
               AND (?3 IS NULL OR p.content LIKE ?3)
               AND (?4 IS NULL OR p.iso_num >= ?4)
               AND (?5 IS NULL OR p.iso_num <= ?5)
               AND (?6 IS NULL OR p.shutter_num >= ?6)
               AND (?7 IS NULL OR p.shutter_num <= ?7)
               AND (?8 IS NULL OR p.aperture_num >= ?8)
               AND (?9 IS NULL OR p.aperture_num <= ?9)
               AND (?10 IS NULL OR p.focal_num >= ?10)
               AND (?11 IS NULL OR p.focal_num <= ?11)
               AND (?12 IS NULL OR p.tone_type = ?12)
             ORDER BY p.scanned_at DESC, p.path ASC",
        )?;
        let tone_filter = filters.tone_type.clone();
        let rows = stmt.query_map(
            params![
                user_id,
                album_id,
                kw,
                filters.iso_min,
                filters.iso_max,
                filters.shutter_min,
                filters.shutter_max,
                filters.aperture_min,
                filters.aperture_max,
                filters.focal_min,
                filters.focal_max,
                tone_filter.as_deref(),
            ],
            |r| {
                Ok(AlbumContentRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    parent_dir: r.get(2)?,
                    album_id: r.get(3)?,
                    album_name: r.get(4)?,
                    album_path: r.get(5)?,
                    iso: r.get(6)?,
                    aperture: r.get(7)?,
                    shutter_speed: r.get(8)?,
                    focal_length: r.get(9)?,
                    shoot_time: r.get(10)?,
                    iso_num: r.get(11)?,
                    focal_num: r.get(12)?,
                    aperture_num: r.get(13)?,
                    shutter_num: r.get(14)?,
                    tone_type: r.get(15)?,
                    avg_luma: r.get(16)?,
                    content: r.get::<_, Option<String>>(17)?.unwrap_or_default(),
                    category: r.get(18)?,
                    sub_category: r.get(19)?,
                    label: r.get(20)?,
                    confidence: r.get(21)?,
                    top3_json: r.get(22)?,
                    person_ids: r.get::<_, Option<String>>(23)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
                    person_count: r.get(24)?,
                })
            },
        )?;
        rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
    }

    /// 删除某个相册的全部内容扫描记录（删除相册时级联调用）
    /// FEAT-050：批量查询照片归属相册（path → album_id；无记录/无归属为 None）
    pub fn album_ids_by_paths(
        &self,
        paths: &[String],
    ) -> Result<HashMap<String, Option<i64>>, DbError> {
        let mut out = HashMap::new();
        for chunk in paths.chunks(500) {
            let placeholders = std::iter::repeat("?")
                .take(chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT path, album_id FROM photo_content_scan WHERE path IN ({placeholders})"
            );
            let params_vec: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_vec.as_slice(), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
            })?;
            for r in rows {
                let (path, album_id) = r?;
                out.insert(path, album_id);
            }
        }
        Ok(out)
    }

    /// FEAT-050：按源路径查 photo_thumb_cache 表内记录的缩略图文件路径
    pub fn list_thumb_paths_by_sources(&self, paths: &[String]) -> Result<Vec<String>, DbError> {
        let mut out = Vec::new();
        for chunk in paths.chunks(500) {
            let placeholders = std::iter::repeat("?")
                .take(chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT thumb_path FROM photo_thumb_cache WHERE source_path IN ({placeholders})"
            );
            let params_vec: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_vec.as_slice(), |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }

    pub fn delete_album_content(&self, album_id: i64) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM photo_content_scan WHERE album_id = ?1", params![album_id])?;
        Ok(())
    }

    /// 按绝对路径批量删除内容扫描记录（照片记录删除/文件删除后级联调用）
    /// 返回实际删除的行数。
    pub fn delete_content_by_paths(&self, paths: &[String]) -> Result<usize, DbError> {
        let mut n = 0usize;
        for p in paths {
            n += self
                .conn
                .execute("DELETE FROM photo_content_scan WHERE path = ?1", params![p])
                .map_err(DbError::Sqlite)?;
        }
        Ok(n)
    }
}

/// 事务内写入一条记录（供批量 upsert 复用）
fn upsert_one(tx: &Transaction, rec: &PhotoContentRecord) -> Result<(), DbError> {
    tx.execute(
        "INSERT INTO photo_content_scan
            (photo_hash, path, parent_dir, album_id, user_id, content,
             category, sub_category, label, confidence, top3_json, person_ids, person_count,
             shoot_time, location, shutter_speed, iso, aperture, focal_length, lat, lon,
             iso_num, focal_num, aperture_num, shutter_num, tone_type, avg_luma, scanned_at)
         VALUES (?1,?2,?3,?4,?5,?6, ?7,?8,?9,?10,?11,?12,?13, ?14,?15,?16,?17,?18,?19,?20,?21,
                 ?22,?23,?24,?25,?26,?27,?28)
         ON CONFLICT(photo_hash) DO UPDATE SET
             path=excluded.path, parent_dir=excluded.parent_dir, album_id=excluded.album_id,
             content=excluded.content, category=excluded.category, sub_category=excluded.sub_category,
             label=excluded.label, confidence=excluded.confidence, top3_json=excluded.top3_json,
             person_ids=excluded.person_ids, person_count=excluded.person_count,
             shoot_time=excluded.shoot_time, location=excluded.location,
             shutter_speed=excluded.shutter_speed, iso=excluded.iso, aperture=excluded.aperture,
             focal_length=excluded.focal_length, lat=excluded.lat, lon=excluded.lon,
             iso_num=excluded.iso_num, focal_num=excluded.focal_num,
             aperture_num=excluded.aperture_num, shutter_num=excluded.shutter_num,
             tone_type=excluded.tone_type, avg_luma=excluded.avg_luma,
             scanned_at=excluded.scanned_at",
        params![
            rec.photo_hash, rec.path, rec.parent_dir, rec.album_id, rec.user_id, rec.content,
            rec.category, rec.sub_category, rec.label, rec.confidence, rec.top3_json,
            rec.person_ids, rec.person_count,
            rec.shoot_time, rec.location, rec.shutter_speed, rec.iso, rec.aperture,
            rec.focal_length, rec.lat, rec.lon,
            rec.iso_num, rec.focal_num, rec.aperture_num, rec.shutter_num,
            rec.tone_type, rec.avg_luma,
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
            iso_num: Some(100),
            focal_num: Some(50.0),
            aperture_num: Some(2.8),
            shutter_num: Some(0.005),
            tone_type: Some("low-key".into()),
            avg_luma: Some(72.0),
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

    /// FEAT-036 + BUG-2026-0909-001：按相册目录前缀统计已入库照片数。
    /// 父子相册共享照片时各自都应显示完整覆盖（行归属谁不影响计数）。
    #[test]
    fn count_scanned_by_prefix_covers_shared_subtrees() {
        let db = mem_db();
        // 父相册 1（/alb1）目录树下 3 张：2 张归属 1，1 张归属子相册 4；相册 2 无记录
        db.upsert_photo_content(&sample_rec("A", "/alb1/a.jpg")).unwrap();
        db.upsert_photo_content(&sample_rec("B", "/alb1/sub/b.jpg")).unwrap();
        let mut rc = sample_rec("C", "/alb1/sub/c.jpg");
        rc.album_id = Some(4);
        db.upsert_photo_content(&rc).unwrap();
        let mut rd = sample_rec("D", "/alb3/d.jpg");
        rd.album_id = Some(3);
        db.upsert_photo_content(&rd).unwrap();

        let pairs = vec![
            (1i64, "/alb1".to_string()),
            (4i64, "/alb1/sub".to_string()),
            (2i64, "/alb2".to_string()),
            (3i64, "/alb3".to_string()),
        ];
        let map = db.count_scanned_by_prefix(1, &pairs).unwrap();
        // 父相册 1：目录树下 3 行（A/B/C），无论行归属 album 1 还是子相册 4
        assert_eq!(map.get(&1), Some(&3), "父相册应按目录子树统计到 3 张");
        // 子相册 4：自己的子树 2 行（B/C）
        assert_eq!(map.get(&4), Some(&2));
        // 相册 2 无记录 → 0；相册 3 → 1
        assert_eq!(map.get(&2), Some(&0));
        assert_eq!(map.get(&3), Some(&1));

        // 分隔符兜底：/alb11 不该被计入 /alb1（前缀必须以分隔符结尾）
        db.upsert_photo_content(&sample_rec("E", "/alb11/e.jpg")).unwrap();
        let map2 = db.count_scanned_by_prefix(1, &pairs).unwrap();
        assert_eq!(map2.get(&1), Some(&3), "/alb11 不该被计入 /alb1");

        // 其他用户看不到
        let map_u2 = db.count_scanned_by_prefix(2, &pairs).unwrap();
        assert_eq!(map_u2.get(&1), Some(&0));
    }

    /// 构造「FTS5 索引与主表失步」状态：主表保留数据，索引被替换为空表
    ///
    /// 还原旧库升级场景：FTS5（commit 0f1e1ff）晚于 photo_content_fts 所属主表引入，
    /// 旧库主表已有数据而索引为空。此处不经过 `init_content_schema`，
    /// 以免其自带的启动自愈把索引提前修好。
    fn desync_fts_index(db: &Database) {
        db.conn.execute_batch("DROP TABLE photo_content_fts;").unwrap();
        db.conn
            .execute_batch(
                "CREATE VIRTUAL TABLE photo_content_fts USING fts5(
                    photo_hash,
                    content,
                    tokenize='porter unicode61',
                    content='photo_content_scan',
                    content_rowid='id'
                );",
            )
            .unwrap();
    }

    /// 锁定根因：索引失步后，未经自愈的写入确实抛 SQLITE_CORRUPT_VTAB(267)
    ///
    /// 这正是「全局照片扫描入库」报 `database disk image is malformed` 的直接来源：
    /// 二次扫描走 `ON CONFLICT DO UPDATE` → 触发 'delete' 未命中索引 → 整批事务回滚。
    #[test]
    fn fts_desync_write_reports_corrupt_vtab() {
        let db = mem_db();
        db.upsert_photo_content(&sample_rec("H1", "/x/a.jpg")).unwrap();
        desync_fts_index(&db);

        let again = PhotoContentRecord {
            label: Some("labrador".into()),
            ..sample_rec("H1", "/x/a.jpg")
        };
        let err = db
            .upsert_photo_contents_inner(std::slice::from_ref(&again))
            .expect_err("索引失步时写入应失败");
        assert!(
            is_fts_corrupt(&err),
            "期望 SQLITE_CORRUPT_VTAB(267)，实际: {err:?}"
        );
    }

    /// 写入路径自愈：索引失步时重建索引并重试，入库不失败
    #[test]
    fn upsert_self_heals_when_fts_index_desynced() {
        let db = mem_db();
        db.upsert_photo_content(&sample_rec("H1", "/x/a.jpg")).unwrap();
        desync_fts_index(&db);

        // 二次扫描同一哈希 → 走 ON CONFLICT DO UPDATE → 触发 'delete' 未命中索引
        let again = PhotoContentRecord {
            label: Some("labrador".into()),
            ..sample_rec("H1", "/x/a.jpg")
        };
        db.upsert_photo_contents(std::slice::from_ref(&again))
            .expect("索引失步应自愈重试，不应抛 CORRUPT_VTAB");

        // 自愈后索引已重建：搜索命中且为二次扫描结果
        let hits = db.search_photo_content("狗", 1, None).unwrap();
        assert_eq!(hits.len(), 1, "重建索引后应能搜到该照片");
        assert_eq!(hits[0].label.as_deref(), Some("labrador"));
    }

    /// 启动自愈：`repair_fts_index` 检测失步并重建，后续写入恢复正常
    #[test]
    fn repair_fts_index_rebuilds_when_desynced() {
        let db = mem_db();
        db.upsert_photo_content(&sample_rec("H1", "/x/a.jpg")).unwrap();
        desync_fts_index(&db);

        db.repair_fts_index().expect("失步索引应重建成功");

        // 重建后可直接写入（不再触发 'delete' 未命中）
        let again = PhotoContentRecord {
            label: Some("labrador".into()),
            ..sample_rec("H1", "/x/a.jpg")
        };
        db.upsert_photo_contents_inner(std::slice::from_ref(&again))
            .expect("重建后写入应正常");
        // 主表幂等：仍是 1 行
        let total: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM photo_content_scan", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 1);
    }

    /// 索引正常时 `repair_fts_index` 应为空操作（不误伤已有索引、不重建）
    #[test]
    fn repair_fts_index_noop_when_healthy() {
        let db = mem_db();
        db.upsert_photo_content(&sample_rec("H1", "/x/a.jpg")).unwrap();
        db.repair_fts_index().unwrap();
        // 重建会改动索引，但主表数据不变、搜索仍命中
        let hits = db.search_photo_content("狗", 1, None).unwrap();
        assert_eq!(hits.len(), 1);
    }
}
