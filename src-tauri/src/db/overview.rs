//! 全局概览快照（FEAT-123）
//!
//! ## 为什么要有这个模块
//!
//! 概览页原先为了画 11 块 widget，每次都要拉一次 `list_timeline`（24,980 行 / 14,438 KB），
//! 再在主线程对 2.5 万行做 6 遍遍历（年份跨度 / 今年月度 / 人脸覆盖 / 年份 Top5 /
//! 最近 6 张 / 那年今日），外加一次 `category_index_stats`（实测 312~574 ms，
//! 慢在 `known` 那次三表 UNION 计数）。日志里一次「概览构建簇」横跨 **3~44 秒**。
//!
//! 而这堆数只在**照片库发生变化**时才会变。所以这里把它做成「变动即重算、否则零成本」：
//!
//! | 层 | 行为 |
//! |---|---|
//! | 签名 | `(张数, max(scanned_at))` × `photo_content_scan` / `albums` / `photo_marks` / `photo_embeddings` + 模型名 |
//! | 命中 | 签名不变 → 直接返回上次的 JSON 快照，**一句聚合都不跑** |
//! | 写入 | 签名变了 → 重新聚合，落 `overview_cache` 表（重启后仍命中） |
//!
//! 返回体积约 **2~4 KB**（对比改前 14 MB），且前端不再需要 2.5 万行明细。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::DbError;

/// 快照里的「最近导入 / 那年今日」卡片（只需要能渲染封面的最小字段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewCard {
    pub path: String,
    pub album_id: Option<i64>,
    pub album_name: Option<String>,
    pub shoot_time: Option<String>,
    pub location: Option<String>,
}

/// 全局概览快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewStats {
    /// 已入库照片总数
    pub total_scanned: i64,
    /// 无拍摄时间张数
    pub no_time: i64,
    /// 带人脸的照片数（person_ids 非空）
    pub with_face: i64,
    /// 带 GPS 坐标的照片数（lat/lon 都有）
    pub with_gps: i64,
    /// 拍摄年份跨度
    pub year_min: Option<i32>,
    pub year_max: Option<i32>,
    /// `"YYYY-MM"` → 张数（不含未记录拍摄时间）
    pub months: Vec<(String, i64)>,
    /// 最近入库 6 张（按 id 倒序）
    pub recent: Vec<OverviewCard>,
    /// 那年今日：匹配 (月-日, 往年) 的最新一张
    pub on_this_day: Option<OverviewCard>,
    /// 那年今日的总张数
    pub on_this_day_count: i64,
    /// 语义索引覆盖（原 `category_index_stats`，慢在 known 的三表 UNION，故一并缓存）
    pub indexed: i64,
    pub stale: i64,
    pub known: i64,
    pub semantic_categories: i64,
    pub model: String,
    /// 数据签名（变了才重算）
    pub signature: String,
    /// 构建时间（Unix 秒）
    pub built_at: i64,
}

/// 参与签名的表：(表名, 时间戳列)
///
/// 为什么用「张数 + 最大时间戳」而不是 mtime/事件：这是**自证式**的 ——
/// 无论哪条代码路径改了数据（导入、扫描、删除、标记、重建向量），
/// 这两个量之一必然变；反过来只看计数会漏掉「改一删一」这种净变化为零的操作。
const SIG_TABLES: &[(&str, &str, &str)] = &[
    ("photo_content_scan", "user_id", "scanned_at"),
    ("albums", "user_id", "updated_at"),
    ("photo_marks", "user_id", "updated_at"),
    ("photo_embeddings", "user_id", "rowid"),
];

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 计算数据签名
fn signature(conn: &Connection, user_id: i64, model: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (table, user_col, ts_col) in SIG_TABLES {
        let sql = format!(
            "SELECT COUNT(*), COALESCE(MAX({ts_col}), 0) FROM {table} WHERE {user_col} = ?1"
        );
        let row: (i64, i64) = conn
            .query_row(&sql, params![user_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap_or((0, 0));
        parts.push(format!("{table}:{}:{}", row.0, row.1));
    }
    // 分类定义（启用/禁用、阈值）与生效模型都会改变「已索引 N/M」的口径
    let cats: (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(sort_order), 0) FROM photo_categories WHERE user_id = ?1",
            params![user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0, 0));
    parts.push(format!("photo_categories:{}:{}", cats.0, cats.1));
    parts.push(format!("model:{model}"));
    parts.join("|")
}

/// 核心聚合：一次把所有 widget 需要的数算完（约 6 条 SQL + 1 条 UNION 计数）
fn build(
    conn: &Connection,
    user_id: i64,
    model: &str,
    today_md: &str,
    today_year: i32,
) -> Result<OverviewStats, DbError> {
    let mut stmt = conn.prepare(
        "SELECT
            COUNT(*),
            SUM(CASE WHEN shoot_time IS NULL OR shoot_time = '' THEN 1 ELSE 0 END),
            SUM(CASE WHEN person_ids IS NOT NULL AND person_ids NOT IN ('', '[]') THEN 1 ELSE 0 END),
            SUM(CASE WHEN lat IS NOT NULL AND lon IS NOT NULL THEN 1 ELSE 0 END),
            MIN(CASE WHEN shoot_time GLOB '[12][0-9][0-9][0-9]*'
                     THEN CAST(substr(shoot_time, 1, 4) AS INTEGER) END),
            MAX(CASE WHEN shoot_time GLOB '[12][0-9][0-9][0-9]*'
                     THEN CAST(substr(shoot_time, 1, 4) AS INTEGER) END)
         FROM photo_content_scan WHERE user_id = ?1",
    )?;
    let (total_scanned, no_time, with_face, with_gps, year_min, year_max) = stmt
        .query_row(params![user_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                r.get::<_, Option<i32>>(4)?,
                r.get::<_, Option<i32>>(5)?,
            ))
        })
        .map_err(DbError::Sqlite)?;
    drop(stmt);

    // 月度分布（年份 Top5 / 今年月度柱都从这里派生，97 行左右）
    let mut stmt = conn.prepare(
        "SELECT substr(shoot_time, 1, 7) AS ym, COUNT(*)
         FROM photo_content_scan
         WHERE user_id = ?1 AND shoot_time IS NOT NULL AND length(shoot_time) >= 7
         GROUP BY ym ORDER BY ym",
    )?;
    let months: Vec<(String, i64)> = stmt
        .query_map(params![user_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(DbError::Sqlite)?
        .collect::<Result<_, _>>()
        .map_err(DbError::Sqlite)?;
    drop(stmt);

    // 最近入库 6 张
    let recent = query_cards(
        conn,
        "SELECT p.path, p.album_id, a.name, p.shoot_time, p.location
         FROM photo_content_scan p
         LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
         WHERE p.user_id = ?1
         ORDER BY p.id DESC LIMIT 6",
        &[&user_id],
    )?;

    // 那年今日（往年同月同日）：总数 + 最新一张
    let on_this_day_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM photo_content_scan
             WHERE user_id = ?1 AND shoot_time IS NOT NULL AND length(shoot_time) >= 10
               AND substr(shoot_time, 6, 5) = ?2
               AND CAST(substr(shoot_time, 1, 4) AS INTEGER) < ?3",
            params![user_id, today_md, today_year],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let on_this_day = query_cards(
        conn,
        "SELECT p.path, p.album_id, a.name, p.shoot_time, p.location
         FROM photo_content_scan p
         LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
         WHERE p.user_id = ?1 AND p.shoot_time IS NOT NULL AND length(p.shoot_time) >= 10
           AND substr(p.shoot_time, 6, 5) = ?2
           AND CAST(substr(p.shoot_time, 1, 4) AS INTEGER) < ?3
         ORDER BY p.shoot_time DESC, p.id DESC LIMIT 1",
        &[&user_id, &today_md, &today_year],
    )?
    .into_iter()
    .next();

    // 语义索引覆盖（原 category_index_stats 的四条计数）
    let indexed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM photo_embeddings WHERE user_id = ?1 AND model = ?2",
            params![user_id, model],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let total_emb: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM photo_embeddings WHERE user_id = ?1",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let known: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
                 SELECT photo_hash FROM photo_content_scan WHERE user_id = ?1
                 UNION SELECT photo_hash FROM photo_thumb_cache WHERE user_id = ?1
                 UNION SELECT photo_hash FROM photo_embeddings WHERE user_id = ?1
             )",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let semantic_categories: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM photo_categories
             WHERE user_id = ?1 AND source != 'builtin' AND enabled = 1",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Ok(OverviewStats {
        total_scanned,
        no_time,
        with_face,
        with_gps,
        year_min,
        year_max,
        months,
        recent,
        on_this_day,
        on_this_day_count,
        indexed,
        stale: total_emb - indexed,
        known,
        semantic_categories,
        model: model.to_string(),
        signature: String::new(), // 由调用方填
        built_at: now_secs(),
    })
}

fn query_cards(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<OverviewCard>, DbError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(args, |r| {
            Ok(OverviewCard {
                path: r.get(0)?,
                album_id: r.get(1)?,
                album_name: r.get(2)?,
                shoot_time: r.get(3)?,
                location: r.get(4)?,
            })
        })
        .map_err(DbError::Sqlite)?;
    rows.collect::<Result<_, _>>().map_err(DbError::Sqlite)
}

impl super::Database {
    /// 建概览快照表（幂等）
    pub fn init_overview_schema(&self) -> Result<(), DbError> {
        self.conn().execute_batch(
            "CREATE TABLE IF NOT EXISTS overview_cache (
                user_id   INTEGER PRIMARY KEY,
                signature TEXT    NOT NULL,
                payload   TEXT    NOT NULL,
                built_at  INTEGER NOT NULL
            );",
        )?;
        Ok(())
    }

    /// 取概览快照：签名不变直接读缓存，变了才重算并落库
    ///
    /// `today_md` / `today_year` 由前端传入（"10-27" / 2026）——「那年今日」的
    /// 口径跟日历有关，后端不引时间库，避免时区/夏令时各算一套。
    pub fn overview_stats(
        &self,
        user_id: i64,
        model: &str,
        today_md: &str,
        today_year: i32,
        force: bool,
    ) -> Result<OverviewStats, DbError> {
        let sig = signature(self.conn(), user_id, model);
        if !force {
            if let Some(hit) = self.overview_cached(user_id, &sig) {
                return Ok(hit);
            }
        }
        let mut stats = build(self.conn(), user_id, model, today_md, today_year)?;
        stats.signature = sig;
        let payload = serde_json::to_string(&stats)
            .map_err(|e| DbError::Other(format!("快照序列化失败: {e}")))?;
        let _ = self.conn().execute(
            "INSERT INTO overview_cache(user_id, signature, payload, built_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET
                signature = excluded.signature,
                payload   = excluded.payload,
                built_at  = excluded.built_at",
            params![user_id, stats.signature, payload, stats.built_at],
        );
        Ok(stats)
    }

    /// 读缓存（签名匹配才返回）
    fn overview_cached(&self, user_id: i64, sig: &str) -> Option<OverviewStats> {
        let payload: String = self
            .conn()
            .query_row(
                "SELECT payload FROM overview_cache WHERE user_id = ?1 AND signature = ?2",
                params![user_id, sig],
                |r| r.get(0),
            )
            .ok()?;
        serde_json::from_str::<OverviewStats>(&payload).ok()
    }
}
