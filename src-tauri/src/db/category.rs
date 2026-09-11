//! 语义分类持久层（v5）—— 用户自定义分类 + 命中物化 + 关键词向量缓存
//!
//! ## 三张表
//!
//! | 表 | 作用 |
//! |---|---|
//! | `photo_categories` | 分类定义（名称/图标/关键词/排除词/阈值/来源） |
//! | `photo_category_hits` | 命中物化（多对多，一张图可属多个分类）；浏览走 SQL 聚合 |
//! | `clip_text_cache` | 关键词向量缓存（改阈值/只改名称时不重复编码文本） |
//!
//! ## 为什么物化
//!
//! 关键词/阈值改动需要"重算"，但**浏览**必须秒开且要计数+封面+筛选叠加；
//! 实时现算拿不到这些。物化后浏览 = 一次 GROUP BY / 一次 ORDER BY score。
//!
//! ## 分类来源（source）
//!
//! - `builtin`：规则通道产出（portrait/street/night_scene/document），命中由
//!   `photo_content_scan.category` 用 SQL 直接物化（`slug` 即类别 key），不跑 CLIP
//! - `preset`：随应用内置的语义分类模板（用户可改词/调阈值/删除）
//! - `user`：用户自建
//!
//! ## 与既有表的生命周期同构
//!
//! `photo_category_hits` 同样冗余 `path` / `album_id`，删除 / 移动照片时
//! 与 `photo_content_scan` / `photo_embeddings` **三处同步维护**。
//!
//! 本模块只做持久化（建表/CRUD/批量命中/聚合），匹配编排在 `crate::category` 完成。

use rusqlite::{params, Transaction};
use serde::{Deserialize, Serialize};

use super::{DbError, Database};

/// 分类来源
pub const SOURCE_BUILTIN: &str = "builtin";
/// 内置预设（可改词/调阈值/删除，语义匹配）
#[allow(dead_code)]
pub const SOURCE_PRESET: &str = "preset";
/// 用户自建（语义匹配）
#[allow(dead_code)]
pub const SOURCE_USER: &str = "user";

/// 系统（规则）分类定义：slug → (名称, 图标, 说明)
/// slug 与 python/vcr/services/arbitrator.py 的 category 取值严格一致
pub const BUILTIN_CATEGORIES: &[(&str, &str, &str, &str)] = &[
    ("portrait", "人物", "👤", "人物特写 / 合影（YOLOv8n-det 人脸检测）"),
    ("street", "扫街", "🚶", "街头多人 / 街拍（人数≥3 且人框较小）"),
    ("night_scene", "夜景", "🌃", "低照度照片（影调 avg_luma < 45）"),
    ("document", "文档", "📄", "文字 / 截图（PaddleOCR 检测 + 截图启发式）"),
];

/// 分类定义（读表结果）
#[derive(Debug, Clone, Serialize)]
pub struct CategoryDef {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub icon: String,
    /// builtin | preset | user
    pub source: String,
    /// builtin 规则键（其余为空）
    pub slug: String,
    pub keywords: Vec<String>,
    pub exclude_keywords: Vec<String>,
    /// 语义匹配强度阈值（净增益；builtin 不参与语义匹配）
    pub threshold: f64,
    pub sort_order: i64,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 新建/更新分类入参
#[derive(Debug, Clone, Deserialize)]
pub struct CategoryInput {
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub exclude_keywords: Vec<String>,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_threshold() -> f64 {
    crate::category::DEFAULT_THRESHOLD
}

fn default_true() -> bool {
    true
}

/// 待写入的一条命中
#[derive(Debug, Clone)]
pub struct CategoryHitRecord {
    pub category_id: i64,
    pub user_id: i64,
    pub photo_hash: String,
    pub path: String,
    pub score: f64,
    pub matched_keyword: String,
}

/// 分类总览行（卡片：计数 + 封面）
#[derive(Debug, Clone, Serialize)]
pub struct CategoryOverviewRow {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub source: String,
    pub slug: String,
    pub threshold: f64,
    pub enabled: bool,
    pub keywords: Vec<String>,
    pub exclude_keywords: Vec<String>,
    pub count: i64,
    pub cover_path: Option<String>,
    pub cover_album_id: Option<i64>,
    pub cover_photo_hash: Option<String>,
}

/// 分类下一张照片（浏览行）
#[derive(Debug, Clone, Serialize)]
pub struct CategoryPhotoRow {
    pub photo_hash: String,
    pub path: String,
    pub album_id: Option<i64>,
    pub album_name: Option<String>,
    pub shoot_time: Option<String>,
    pub location: Option<String>,
    pub tone_type: Option<String>,
    pub person_ids: Vec<String>,
    /// 规则分类（人物/夜景/文档；仅 builtin 命中行有值）
    pub category: Option<String>,
    pub sub_category: Option<String>,
    /// 语义匹配强度（净增益）
    pub score: f64,
    pub matched_keyword: String,
    pub category_id: i64,
    pub category_name: String,
}

/// 索引覆盖统计（UI 提示「已索引 N/M」）
#[derive(Debug, Clone, Serialize)]
pub struct CategoryIndexStats {
    /// 当前档位模型下的向量数（可参与语义匹配的照片数）
    pub indexed: i64,
    /// 其他档位模型留下的向量数（换档后需重建）
    pub stale: i64,
    /// 已入库照片总数（photo_content_scan ∪ photo_thumb_cache 去重）
    pub known: i64,
    /// 当前生效档位模型标识
    pub model: String,
    /// 语义分类数（含内置预设，不含 builtin 规则分类）
    pub semantic_categories: i64,
}

fn parse_list(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

fn dump_list(v: &[String]) -> Option<String> {
    if v.is_empty() {
        None
    } else {
        serde_json::to_string(v).ok()
    }
}

impl Database {
    /// 建表（v5 语义分类）：幂等，应用启动安全调用
    pub fn init_category_schema(&self) -> Result<(), DbError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS photo_categories (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id          INTEGER NOT NULL,
                name             TEXT    NOT NULL,
                icon             TEXT    NOT NULL DEFAULT '',
                source           TEXT    NOT NULL DEFAULT 'user',
                slug             TEXT,
                keywords         TEXT,
                exclude_keywords TEXT,
                threshold        REAL    NOT NULL,
                sort_order       INTEGER NOT NULL DEFAULT 0,
                enabled          INTEGER NOT NULL DEFAULT 1,
                created_at       INTEGER NOT NULL,
                updated_at       INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS photo_category_hits (
                category_id     INTEGER NOT NULL,
                user_id         INTEGER NOT NULL,
                photo_hash      TEXT    NOT NULL,
                path            TEXT    NOT NULL,
                score           REAL    NOT NULL,
                matched_keyword TEXT,
                updated_at      INTEGER NOT NULL,
                PRIMARY KEY (category_id, photo_hash)
            );
            CREATE TABLE IF NOT EXISTS clip_text_cache (
                model        TEXT    NOT NULL,
                text         TEXT    NOT NULL,
                dim          INTEGER NOT NULL,
                embedding    BLOB    NOT NULL,
                generated_at INTEGER NOT NULL,
                PRIMARY KEY (model, text)
            );",
        )?;
        let _ = self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_pcat_user ON photo_categories(user_id);
             CREATE UNIQUE INDEX IF NOT EXISTS idx_pcat_user_name ON photo_categories(user_id, name);
             CREATE INDEX IF NOT EXISTS idx_pch_user ON photo_category_hits(user_id);
             CREATE INDEX IF NOT EXISTS idx_pch_cat ON photo_category_hits(category_id, score DESC);
             CREATE INDEX IF NOT EXISTS idx_pch_path ON photo_category_hits(path);
             CREATE INDEX IF NOT EXISTS idx_pch_hash ON photo_category_hits(photo_hash);",
        );
        Ok(())
    }

    // ------------------------------------------------------------------
    // 分类 CRUD
    // ------------------------------------------------------------------
    fn row_to_category(r: &rusqlite::Row) -> rusqlite::Result<CategoryDef> {
        Ok(CategoryDef {
            id: r.get(0)?,
            user_id: r.get(1)?,
            name: r.get(2)?,
            icon: r.get(3)?,
            source: r.get(4)?,
            slug: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            keywords: parse_list(r.get(6)?),
            exclude_keywords: parse_list(r.get(7)?),
            threshold: r.get(8)?,
            sort_order: r.get(9)?,
            enabled: r.get::<_, i64>(10)? != 0,
            created_at: r.get(11)?,
            updated_at: r.get(12)?,
        })
    }

    const CATEGORY_COLS: &'static str =
        "id, user_id, name, icon, source, slug, keywords, exclude_keywords, threshold,
         sort_order, enabled, created_at, updated_at";

    /// 列出用户全部分类（规则分类在前，其余按 sort_order/创建时间）
    pub fn list_categories(&self, user_id: i64) -> Result<Vec<CategoryDef>, DbError> {
        let sql = format!(
            "SELECT {} FROM photo_categories WHERE user_id = ?1
             ORDER BY (source != 'builtin'), sort_order ASC, id ASC",
            Self::CATEGORY_COLS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![user_id], Self::row_to_category)?;
        rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
    }

    /// 读取单个分类（校验归属）
    pub fn get_category(&self, id: i64, user_id: i64) -> Result<Option<CategoryDef>, DbError> {
        let sql = format!(
            "SELECT {} FROM photo_categories WHERE id = ?1 AND user_id = ?2",
            Self::CATEGORY_COLS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![id, user_id], Self::row_to_category)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// 内置分类是否已初始化
    pub fn builtin_ready(&self, user_id: i64) -> Result<bool, DbError> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM photo_categories WHERE user_id = ?1 AND source = 'builtin'",
            params![user_id],
            |r| r.get(0),
        )?;
        Ok(n >= BUILTIN_CATEGORIES.len() as i64)
    }

    /// 初始化系统（规则）分类（幂等；已存在同名不重复插入）
    pub fn ensure_builtin_categories(&self, user_id: i64) -> Result<usize, DbError> {
        let now = Self::now_secs();
        let mut added = 0usize;
        for (i, (slug, name, icon, _desc)) in BUILTIN_CATEGORIES.iter().enumerate() {
            let n = self.conn.execute(
                "INSERT OR IGNORE INTO photo_categories
                    (user_id, name, icon, source, slug, keywords, exclude_keywords,
                     threshold, sort_order, enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'builtin', ?4, NULL, NULL, 0, ?5, 1, ?6, ?6)",
                params![user_id, name, icon, slug, -(i as i64) - 1, now],
            )?;
            added += n;
        }
        Ok(added)
    }

    /// 幂等写入内置预设分类（同名已存在则跳过；用户改过的词不会被覆盖）
    pub fn seed_preset_categories(
        &self,
        user_id: i64,
        presets: &[(&str, &str, &[&str], f64)],
    ) -> Result<usize, DbError> {
        let now = Self::now_secs();
        let mut added = 0usize;
        for (i, (name, icon, keywords, threshold)) in presets.iter().enumerate() {
            let kw: Vec<String> = keywords.iter().map(|s| (*s).to_string()).collect();
            let n = self.conn.execute(
                "INSERT OR IGNORE INTO photo_categories
                    (user_id, name, icon, source, slug, keywords, exclude_keywords,
                     threshold, sort_order, enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'preset', NULL, ?4, NULL, ?5, ?6, 1, ?7, ?7)",
                params![user_id, name, icon, dump_list(&kw), threshold, i as i64, now],
            )?;
            added += n;
        }
        Ok(added)
    }

    /// 新建分类 → 返回新 id
    pub fn create_category(&self, user_id: i64, input: &CategoryInput) -> Result<i64, DbError> {
        let now = Self::now_secs();
        self.conn.execute(
            "INSERT INTO photo_categories
                (user_id, name, icon, source, slug, keywords, exclude_keywords,
                 threshold, sort_order, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'user', NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                user_id,
                input.name.trim(),
                input.icon,
                dump_list(&input.keywords),
                dump_list(&input.exclude_keywords),
                input.threshold,
                input.sort_order,
                input.enabled as i64,
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 更新分类（名称/图标/关键词/排除词/阈值/启用状态；builtin 只允许改名与启用）
    pub fn update_category(
        &self,
        id: i64,
        user_id: i64,
        input: &CategoryInput,
    ) -> Result<bool, DbError> {
        let now = Self::now_secs();
        let n = self.conn.execute(
            "UPDATE photo_categories SET
                name = ?1, icon = ?2, keywords = ?3, exclude_keywords = ?4,
                threshold = ?5, sort_order = ?6, enabled = ?7, updated_at = ?8
             WHERE id = ?9 AND user_id = ?10",
            params![
                input.name.trim(),
                input.icon,
                dump_list(&input.keywords),
                dump_list(&input.exclude_keywords),
                input.threshold,
                input.sort_order,
                input.enabled as i64,
                now,
                id,
                user_id
            ],
        )?;
        Ok(n > 0)
    }

    /// 删除分类（连带命中行）
    pub fn delete_category(&self, id: i64, user_id: i64) -> Result<bool, DbError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM photo_category_hits WHERE category_id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
        let n = tx.execute(
            "DELETE FROM photo_categories WHERE id = ?1 AND user_id = ?2 AND source != 'builtin'",
            params![id, user_id],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    // ------------------------------------------------------------------
    // 命中物化
    // ------------------------------------------------------------------
    /// 整体替换某分类的命中集（事务；空集 = 清空）
    pub fn replace_category_hits(
        &self,
        category_id: i64,
        user_id: i64,
        hits: &[CategoryHitRecord],
    ) -> Result<usize, DbError> {
        let now = Self::now_secs();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM photo_category_hits WHERE category_id = ?1 AND user_id = ?2",
            params![category_id, user_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO photo_category_hits
                    (category_id, user_id, photo_hash, path, score, matched_keyword, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for h in hits {
                stmt.execute(params![
                    category_id,
                    user_id,
                    h.photo_hash,
                    h.path,
                    h.score,
                    h.matched_keyword,
                    now
                ])?;
            }
        }
        tx.commit()?;
        Ok(hits.len())
    }

    /// 由 `photo_content_scan.category` 物化 builtin 规则命中（SQL 直算，不跑 CLIP）
    pub fn replace_builtin_hits(&self, category_id: i64, user_id: i64, slug: &str) -> Result<usize, DbError> {
        let now = Self::now_secs();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM photo_category_hits WHERE category_id = ?1 AND user_id = ?2",
            params![category_id, user_id],
        )?;
        // 人物 = 特写 + 合影；扫街独立成类
        let cats: Vec<&str> = if slug == "portrait" {
            vec!["portrait"]
        } else {
            vec![slug]
        };
        let mut n = 0usize;
        for c in cats {
            n += tx.execute(
                "INSERT OR REPLACE INTO photo_category_hits
                    (category_id, user_id, photo_hash, path, score, matched_keyword, updated_at)
                 SELECT ?1, user_id, photo_hash, path, COALESCE(confidence, 0.5), category, ?4
                 FROM photo_content_scan WHERE user_id = ?2 AND category = ?3",
                params![category_id, user_id, c, now],
            )?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// 分类总览（计数 + 封面：命中集内语义强度最高的一张）
    pub fn category_overview(&self, user_id: i64) -> Result<Vec<CategoryOverviewRow>, DbError> {
        let cats = self.list_categories(user_id)?;
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*),
                    (SELECT h.photo_hash FROM photo_category_hits h
                      WHERE h.category_id = ?1 AND h.user_id = ?2
                      ORDER BY h.score DESC LIMIT 1),
                    (SELECT h.path FROM photo_category_hits h
                      WHERE h.category_id = ?1 AND h.user_id = ?2
                      ORDER BY h.score DESC LIMIT 1),
                    (SELECT p.album_id FROM photo_category_hits h
                      LEFT JOIN photo_content_scan p
                             ON p.photo_hash = h.photo_hash AND p.user_id = h.user_id
                      WHERE h.category_id = ?1 AND h.user_id = ?2
                      ORDER BY h.score DESC LIMIT 1)
             FROM photo_category_hits WHERE category_id = ?1 AND user_id = ?2",
        )?;
        let mut out = Vec::with_capacity(cats.len());
        for c in cats {
            let (count, cover_photo_hash, cover_path, cover_album_id): (
                i64,
                Option<String>,
                Option<String>,
                Option<i64>,
            ) = stmt.query_row(params![c.id, user_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            out.push(CategoryOverviewRow {
                id: c.id,
                name: c.name,
                icon: c.icon,
                source: c.source,
                slug: c.slug,
                threshold: c.threshold,
                enabled: c.enabled,
                keywords: c.keywords,
                exclude_keywords: c.exclude_keywords,
                count,
                cover_path,
                cover_album_id,
                cover_photo_hash,
            });
        }
        Ok(out)
    }

    /// 分类下的照片（按语义强度降序；三表 LEFT JOIN 兼容未跑 AI 扫描的照片）
    pub fn list_category_photos(
        &self,
        category_id: i64,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<CategoryPhotoRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT h.photo_hash, h.path, h.score, COALESCE(h.matched_keyword, ''),
                    p.album_id, a.name, p.shoot_time, p.location, p.tone_type,
                    p.person_ids, p.category, p.sub_category, c.name, h.category_id
             FROM photo_category_hits h
             LEFT JOIN photo_content_scan p
                    ON p.photo_hash = h.photo_hash AND p.user_id = h.user_id
             LEFT JOIN photo_thumb_cache t
                    ON t.photo_hash = h.photo_hash AND t.user_id = h.user_id
             LEFT JOIN albums a
                    ON a.id = COALESCE(p.album_id, t.album_id) AND a.user_id = h.user_id
             JOIN photo_categories c ON c.id = h.category_id
             WHERE h.category_id = ?1 AND h.user_id = ?2
             ORDER BY h.score DESC, h.photo_hash ASC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![category_id, user_id, limit], |r| {
            let person_ids: Option<String> = r.get(9)?;
            Ok(CategoryPhotoRow {
                photo_hash: r.get(0)?,
                path: r.get(1)?,
                score: r.get(2)?,
                matched_keyword: r.get(3)?,
                album_id: r.get(4)?,
                album_name: r.get(5)?,
                shoot_time: r.get(6)?,
                location: r.get(7)?,
                tone_type: r.get(8)?,
                person_ids: person_ids
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default(),
                category: r.get(10)?,
                sub_category: r.get(11)?,
                category_name: r.get(12)?,
                category_id: r.get(13)?,
            })
        })?;
        rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
    }

    /// 索引覆盖统计（语义分类页面提示「已索引 N/M」与换档提醒）
    pub fn category_index_stats(&self, user_id: i64, model: &str) -> Result<CategoryIndexStats, DbError> {
        let indexed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM photo_embeddings WHERE user_id = ?1 AND model = ?2",
            params![user_id, model],
            |r| r.get(0),
        )?;
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM photo_embeddings WHERE user_id = ?1",
            params![user_id],
            |r| r.get(0),
        )?;
        let known: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM (
                 SELECT photo_hash FROM photo_content_scan WHERE user_id = ?1
                 UNION SELECT photo_hash FROM photo_thumb_cache WHERE user_id = ?1
                 UNION SELECT photo_hash FROM photo_embeddings WHERE user_id = ?1
             )",
            params![user_id],
            |r| r.get(0),
        )?;
        let semantic_categories: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM photo_categories
             WHERE user_id = ?1 AND source != 'builtin' AND enabled = 1",
            params![user_id],
            |r| r.get(0),
        )?;
        Ok(CategoryIndexStats {
            indexed,
            stale: total - indexed,
            known,
            model: model.to_string(),
            semantic_categories,
        })
    }

    // ------------------------------------------------------------------
    // 关键词向量缓存
    // ------------------------------------------------------------------
    /// 批量读取关键词向量（缺哪些由调用方决定再补编码）
    pub fn load_text_vectors(
        &self,
        model: &str,
        texts: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<f32>>, DbError> {
        use std::collections::HashMap;
        let mut out: HashMap<String, Vec<f32>> = HashMap::new();
        if texts.is_empty() {
            return Ok(out);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT embedding FROM clip_text_cache WHERE model = ?1 AND text = ?2")?;
        for t in texts {
            let blob: Option<Vec<u8>> = stmt
                .query_row(params![model, t], |r| r.get(0))
                .ok();
            if let Some(b) = blob {
                out.insert(t.clone(), super::embedding::f32_vec_from_blob(&b));
            }
        }
        Ok(out)
    }

    /// 写入关键词向量缓存
    pub fn save_text_vectors(
        &self,
        model: &str,
        items: &[(String, Vec<f32>)],
    ) -> Result<(), DbError> {
        if items.is_empty() {
            return Ok(());
        }
        let now = Self::now_secs();
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO clip_text_cache
                    (model, text, dim, embedding, generated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (text, vec) in items {
                stmt.execute(params![
                    model,
                    text,
                    vec.len() as i64,
                    super::embedding::f32_blob(vec),
                    now
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // 级联（与 photo_content_scan / photo_embeddings 三处同步）
    // ------------------------------------------------------------------
    /// 按绝对路径批量删除命中（照片删除级联）
    pub fn delete_category_hits_by_paths(&self, paths: &[String]) -> Result<usize, DbError> {
        let mut n = 0usize;
        for p in paths {
            n += self
                .conn
                .execute("DELETE FROM photo_category_hits WHERE path = ?1", params![p])?;
        }
        Ok(n)
    }

    /// 移动照片时同步命中行 path（并按新相册归属刷新）
    pub fn move_category_hit_path(
        &self,
        user_id: i64,
        old_path: &str,
        new_path: &str,
    ) -> Result<usize, DbError> {
        let n = self.conn.execute(
            "UPDATE photo_category_hits SET path = ?1 WHERE user_id = ?2 AND path = ?3",
            params![new_path, user_id, old_path],
        )?;
        Ok(n)
    }

    /// 按相册删除命中（相册内照片与相册记录一并清理时使用）
    pub fn delete_category_hits_in_tx(
        tx: &Transaction,
        album_id: i64,
    ) -> Result<(), DbError> {
        tx.execute(
            "DELETE FROM photo_category_hits WHERE photo_hash IN (
                 SELECT photo_hash FROM photo_content_scan WHERE album_id = ?1
             )",
            params![album_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn db() -> Database {
        let conn = Connection::open_in_memory().unwrap();
        let db = Database { conn };
        db.init_schema().unwrap();
        db
    }

    fn input(name: &str, kws: &[&str], thr: f64) -> CategoryInput {
        CategoryInput {
            name: name.into(),
            icon: "🏷️".into(),
            keywords: kws.iter().map(|s| s.to_string()).collect(),
            exclude_keywords: vec![],
            threshold: thr,
            sort_order: 0,
            enabled: true,
        }
    }

    fn hit(cat: i64, hash: &str, path: &str, score: f64, kw: &str) -> CategoryHitRecord {
        CategoryHitRecord {
            category_id: cat,
            user_id: 1,
            photo_hash: hash.into(),
            path: path.into(),
            score,
            matched_keyword: kw.into(),
        }
    }

    #[test]
    fn builtin_and_presets_are_idempotent() {
        let db = db();
        let n1 = db.ensure_builtin_categories(1).unwrap();
        assert_eq!(n1, BUILTIN_CATEGORIES.len());
        // 再次调用不重复插入
        assert_eq!(db.ensure_builtin_categories(1).unwrap(), 0);
        assert!(db.builtin_ready(1).unwrap());

        let presets: &[(&str, &str, &[&str], f64)] = &[
            ("动物", "🐾", &["动物", "宠物"], 0.03),
            ("美食", "🍜", &["美食", "菜肴"], 0.03),
        ];
        assert_eq!(db.seed_preset_categories(1, presets).unwrap(), 2);
        assert_eq!(db.seed_preset_categories(1, presets).unwrap(), 0, "同名预设不重复插入");

        // 多用户隔离
        assert_eq!(db.seed_preset_categories(2, presets).unwrap(), 2);
        assert_eq!(db.list_categories(1).unwrap().len(), BUILTIN_CATEGORIES.len() + 2);
    }

    #[test]
    fn category_crud_and_keywords_roundtrip() {
        let db = db();
        let id = db.create_category(1, &input("我家的猫", &["一只猫", "猫咪"], 0.04)).unwrap();
        let c = db.get_category(id, 1).unwrap().unwrap();
        assert_eq!(c.name, "我家的猫");
        assert_eq!(c.keywords, vec!["一只猫", "猫咪"]);
        assert_eq!(c.source, SOURCE_USER);
        assert!((c.threshold - 0.04).abs() < 1e-9);

        let mut upd = input("猫猫", &["一只猫"], 0.02);
        upd.exclude_keywords = vec!["热狗".into()];
        assert!(db.update_category(id, 1, &upd).unwrap());
        let c = db.get_category(id, 1).unwrap().unwrap();
        assert_eq!(c.name, "猫猫");
        assert_eq!(c.exclude_keywords, vec!["热狗"]);

        // 跨用户不可见 / 不可改
        assert!(db.get_category(id, 2).unwrap().is_none());
        assert!(!db.update_category(id, 9, &upd).unwrap());

        assert!(db.delete_category(id, 1).unwrap());
        assert!(db.get_category(id, 1).unwrap().is_none());
    }

    #[test]
    fn builtin_categories_cannot_be_deleted() {
        let db = db();
        db.ensure_builtin_categories(1).unwrap();
        let cat = db.list_categories(1).unwrap().into_iter().next().unwrap();
        assert_eq!(cat.source, SOURCE_BUILTIN);
        assert!(!db.delete_category(cat.id, 1).unwrap(), "规则分类不可删");
        assert!(db.get_category(cat.id, 1).unwrap().is_some());
    }

    #[test]
    fn hits_replace_overview_and_photos() {
        let db = db();
        let id = db.create_category(1, &input("猫", &["一只猫"], 0.03)).unwrap();
        let hits = vec![
            hit(id, "h1", "/a/1.jpg", 0.05, "一只猫"),
            hit(id, "h2", "/a/2.jpg", 0.08, "一只猫"),
            hit(id, "h3", "/a/3.jpg", 0.04, "猫咪"),
        ];
        assert_eq!(db.replace_category_hits(id, 1, &hits).unwrap(), 3);

        let ov = db.category_overview(1).unwrap();
        let row = ov.iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.count, 3);
        // 封面 = 分数最高那张
        assert_eq!(row.cover_path.as_deref(), Some("/a/2.jpg"));
        assert_eq!(row.cover_photo_hash.as_deref(), Some("h2"));
        assert_eq!(row.keywords, vec!["一只猫"]);

        // 浏览按语义强度降序
        let photos = db.list_category_photos(id, 1, 100).unwrap();
        assert_eq!(photos.len(), 3);
        assert_eq!(photos[0].photo_hash, "h2");
        assert_eq!(photos[2].matched_keyword, "猫咪");
        assert_eq!(photos[1].score, 0.05);

        // 替换（不是追加）：第二次只留 1 条
        assert_eq!(db.replace_category_hits(id, 1, &[hit(id, "h1", "/a/1.jpg", 0.05, "一只猫")]).unwrap(), 1);
        assert_eq!(db.category_overview(1).unwrap().iter().find(|r| r.id == id).unwrap().count, 1);

        // 多用户隔离：user 2 看不到
        assert_eq!(db.category_overview(2).unwrap().iter().filter(|r| r.id == id).count(), 0);
    }

    #[test]
    fn builtin_hits_come_from_rule_scan_table() {
        let db = db();
        db.ensure_builtin_categories(1).unwrap();
        let portrait = db
            .list_categories(1)
            .unwrap()
            .into_iter()
            .find(|c| c.slug == "portrait")
            .unwrap();
        let night = db
            .list_categories(1)
            .unwrap()
            .into_iter()
            .find(|c| c.slug == "night_scene")
            .unwrap();

        db.conn
            .execute_batch(
                "INSERT INTO photo_content_scan
                   (photo_hash, path, parent_dir, user_id, category, confidence, scanned_at)
                 VALUES ('x1','/p/1.jpg','/p',1,'portrait',0.91,1),
                        ('x2','/p/2.jpg','/p',1,'night_scene',0.85,1),
                        ('x3','/p/3.jpg','/p',1,'other',0.10,1);",
            )
            .unwrap();

        assert_eq!(db.replace_builtin_hits(portrait.id, 1, "portrait").unwrap(), 1);
        assert_eq!(db.replace_builtin_hits(night.id, 1, "night_scene").unwrap(), 1);
        let rows = db.list_category_photos(portrait.id, 1, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/p/1.jpg");
        assert_eq!(rows[0].category.as_deref(), Some("portrait"));
    }

    #[test]
    fn cascades_on_delete_and_move() {
        let db = db();
        let id = db.create_category(1, &input("猫", &["一只猫"], 0.03)).unwrap();
        db.replace_category_hits(id, 1, &[hit(id, "h1", "/old/1.jpg", 0.05, "一只猫")])
            .unwrap();
        // 移动：path 跟着走
        assert_eq!(db.move_category_hit_path(1, "/old/1.jpg", "/new/1.jpg").unwrap(), 1);
        let photos = db.list_category_photos(id, 1, 10).unwrap();
        assert_eq!(photos[0].path, "/new/1.jpg");
        // 路径删除级联
        assert_eq!(db.delete_category_hits_by_paths(&["/new/1.jpg".into()]).unwrap(), 1);
        assert!(db.list_category_photos(id, 1, 10).unwrap().is_empty());
    }

    #[test]
    fn text_vector_cache_roundtrip() {
        let db = db();
        assert!(db.load_text_vectors("m1", &["一只猫".into()]).unwrap().is_empty());
        db.save_text_vectors("m1", &[("一只猫".into(), vec![0.5, 0.5])]).unwrap();
        let got = db.load_text_vectors("m1", &["一只猫".into()]).unwrap();
        assert_eq!(got.get("一只猫").unwrap(), &vec![0.5, 0.5]);
        // 档位隔离：另一模型查不到
        assert!(db.load_text_vectors("m2", &["一只猫".into()]).unwrap().is_empty());
        // 覆盖写
        db.save_text_vectors("m1", &[("一只猫".into(), vec![1.0])]).unwrap();
        assert_eq!(
            db.load_text_vectors("m1", &["一只猫".into()]).unwrap().get("一只猫").unwrap(),
            &vec![1.0]
        );
    }

    #[test]
    fn index_stats_counts_and_stale() {
        let db = db();
        db.create_category(1, &input("猫", &["一只猫"], 0.03)).unwrap();
        db.conn
            .execute_batch(
                "INSERT INTO photo_embeddings (photo_hash,user_id,album_id,path,dim,model,embedding,generated_at)
                 VALUES ('e1',1,NULL,'/p/1.jpg',512,'m_new',X'0000',1),
                        ('e2',1,NULL,'/p/2.jpg',512,'m_new',X'0000',1),
                        ('e3',1,NULL,'/p/3.jpg',512,'m_old',X'0000',1);
                 INSERT INTO photo_thumb_cache (photo_hash,source_path,thumb_path,user_id,size_bytes,mtime_ns,generated_at)
                 VALUES ('e1','/p/1.jpg','/t/1.jpg',1,1,1,1),
                        ('e4','/p/4.jpg','/t/4.jpg',1,1,1,1);",
            )
            .unwrap();
        let s = db.category_index_stats(1, "m_new").unwrap();
        assert_eq!(s.indexed, 2);
        assert_eq!(s.stale, 1, "旧档位向量计入 stale");
        assert_eq!(s.known, 4, "内容表 / 缩略图表 / 向量表三路去重后的入库照片数");
        assert_eq!(s.semantic_categories, 1);
    }
}

