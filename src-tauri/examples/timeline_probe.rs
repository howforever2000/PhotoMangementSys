// 时间线接口量测探针（FEAT-122）
//
// 目的：把「首包有多大 / 后端各花多少 ms」量出来，改前改后对比，避免凭感觉优化。
// 只读访问数据库副本，不写库、不跑迁移。
//
// 用法：
//   1) 拷贝一份库（含 -wal）到临时目录，避免和应用抢写锁
//   2) cargo run --example timeline_probe -- "<副本 photos.db>" [user_id]
//
// 输出四组数：
//   list_timeline（改前：全量 18 列）      → ms / 字节
//   timeline_index（改后：骨架）           → ms / 字节
//   timeline_month 最大月（改后：单月）    → ms / 字节
//   timeline_month 未记录首页 300 张        → ms / 字节
use std::time::Instant;

use rusqlite::{params, Connection};
use serde::Serialize;

/// 概览快照（与 `db::overview::OverviewStats` 同形，仅用于量体积）
#[derive(Serialize)]
struct OverviewBench {
    total_scanned: i64,
    no_time: i64,
    with_face: i64,
    with_gps: i64,
    year_min: Option<i32>,
    year_max: Option<i32>,
    months: Vec<(String, i64)>,
    recent: Vec<Card>,
    on_this_day: Option<Card>,
    on_this_day_count: i64,
    indexed: i64,
    stale: i64,
    known: i64,
    semantic_categories: i64,
    model: String,
}

#[derive(Serialize)]
struct OldHit {
    id: i64,
    path: String,
    parent_dir: String,
    album_id: Option<i64>,
    album_name: Option<String>,
    album_path: Option<String>,
    content: String,
    category: Option<String>,
    sub_category: Option<String>,
    label: Option<String>,
    confidence: Option<f64>,
    person_ids: Vec<String>,
    shoot_time: Option<String>,
    location: Option<String>,
    iso: Option<String>,
    aperture: Option<String>,
    shutter_speed: Option<String>,
    focal_length: Option<String>,
}

#[derive(Serialize)]
struct MonthIdx {
    year: String,
    month: String,
    count: i64,
}

#[derive(Serialize)]
struct Index {
    months: Vec<MonthIdx>,
    total: i64,
    no_time: i64,
    no_location: i64,
}

#[derive(Serialize)]
struct Card {
    id: i64,
    path: String,
    album_id: Option<i64>,
    album_name: Option<String>,
    shoot_time: Option<String>,
    location: Option<String>,
    label: Option<String>,
    thumb_path: Option<String>,
}

fn kb(bytes: usize) -> f64 {
    bytes as f64 / 1024.0
}

fn report(label: &str, ms: u128, bytes: usize, rows: usize) {
    println!(
        "{label:<34} {ms:>6} ms   {:>10.2} KB   rows={rows}",
        kb(bytes)
    );
}

fn main() {
    let path = std::env::args().nth(1).expect("需要数据库副本路径");
    let user_id: i64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    // 注意：副本是 WAL 模式且带 -wal 文件。只读打开时 SQLite 需要写 -shm 才能读，
    // 用 READ_ONLY 会报 "unable to open database file"，所以这里直接读写打开副本
    // （副本是临时文件，写坏了重拷即可，绝不碰真实库）。
    let conn = Connection::open(&path).expect("打不开数据库副本");
    println!("db = {path}   user_id = {user_id}\n");

    // ── 1. 改前：list_timeline（全量 18 列）─────────────────────────────
    let t = Instant::now();
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.path, p.parent_dir, p.album_id, a.name, a.path,
                    p.content, p.category, p.sub_category, p.label, p.confidence,
                    p.person_ids, p.shoot_time, p.location, p.iso, p.aperture,
                    p.shutter_speed, p.focal_length
             FROM photo_content_scan p
             LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
             WHERE p.user_id = ?1
             ORDER BY (p.shoot_time IS NULL), p.shoot_time ASC, p.id ASC",
        )
        .unwrap();
    let old: Vec<OldHit> = stmt
        .query_map(params![user_id], |r| {
            Ok(OldHit {
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
                person_ids: Vec::new(),
                shoot_time: r.get(12)?,
                location: r.get(13)?,
                iso: r.get(14)?,
                aperture: r.get(15)?,
                shutter_speed: r.get(16)?,
                focal_length: r.get(17)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let ms = t.elapsed().as_millis();
    let bytes = serde_json::to_vec(&old).unwrap().len();
    report("list_timeline（改前：全量）", ms, bytes, old.len());
    drop(stmt);

    // ── 2. 改后：骨架 ───────────────────────────────────────────────────
    let t = Instant::now();
    let mut stmt = conn
        .prepare(
            "SELECT CASE WHEN p.shoot_time IS NULL OR p.shoot_time = '' THEN ''
                         ELSE substr(p.shoot_time, 1, 7) END AS ym, COUNT(*)
             FROM photo_content_scan p
             WHERE p.user_id = ?1
             GROUP BY ym
             ORDER BY (ym = '') ASC, ym ASC",
        )
        .unwrap();
    let raw: Vec<(String, i64)> = stmt
        .query_map(params![user_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut months = Vec::new();
    let mut total = 0;
    let mut no_time = 0;
    for (ym, count) in &raw {
        total += *count;
        let (year, month) = if ym.is_empty() {
            no_time += *count;
            ("未记录".to_string(), String::new())
        } else {
            (
                ym.get(0..4).unwrap_or(ym).to_string(),
                ym.get(5..7).unwrap_or("").to_string(),
            )
        };
        months.push(MonthIdx {
            year,
            month,
            count: *count,
        });
    }
    let no_location: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM photo_content_scan
             WHERE user_id = ?1 AND (location IS NULL OR location = '')",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let idx = Index {
        months,
        total,
        no_time,
        no_location,
    };
    let ms = t.elapsed().as_millis();
    let bytes = serde_json::to_vec(&idx).unwrap().len();
    report("timeline_index（改后：骨架）", ms, bytes, idx.months.len());
    drop(stmt);

    // ── 3. 改后：最大月 + 未记录首页 ────────────────────────────────────
    let mut biggest: Option<(String, String, i64)> = None;
    for m in &idx.months {
        if biggest.as_ref().map(|b| m.count > b.2).unwrap_or(true) {
            biggest = Some((m.year.clone(), m.month.clone(), m.count));
        }
    }
    for (label, year, month, limit) in [
        ("timeline_month 最大月", None, None, -1i64),
        ("timeline_month 未记录首页", Some("未记录"), Some(""), 300i64),
    ] {
        let (y, m) = match (&year, &month) {
            (Some(y), Some(m)) => (y.to_string(), m.to_string()),
            _ => biggest
                .clone()
                .map(|b| (b.0, b.1))
                .unwrap_or(("2024".into(), "06".into())),
        };
        let t = Instant::now();
        let cards = query_month(&conn, user_id, &y, &m, limit, 0);
        let ms = t.elapsed().as_millis();
        let bytes = serde_json::to_vec(&cards).unwrap().len();
        report(&format!("{label} {y}-{m}"), ms, bytes, cards.len());
    }

    // ── 4. 缩略图 JOIN 覆盖率 ──────────────────────────────────────────
    let joined: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM photo_content_scan p
             JOIN photo_thumb_cache t ON t.photo_hash = p.photo_hash
             WHERE p.user_id = ?1",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let all: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM photo_content_scan WHERE user_id = ?1",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    println!("\n缩略图随行下发覆盖率: {joined}/{all}（缺 {} 张走 get_photo_thumbs 兜底）", all - joined);

    // ── 5. FEAT-123：概览快照 ──────────────────────────────────────────
    overview_bench(&conn, user_id, "chinese-clip-vit-b16-fp32");
}

/// 量概览快照：签名查询（= 缓存命中路径）与全量聚合（= 未命中路径）分开计时
fn overview_bench(conn: &Connection, user_id: i64, model: &str) {
    println!("\n── FEAT-123 概览快照 ──────────────────────────────────");

    // 签名：四张表的 (COUNT, MAX(ts)) + 分类数 + 模型名
    let t = Instant::now();
    let mut sig = String::new();
    for (table, ts) in [
        ("photo_content_scan", "scanned_at"),
        ("albums", "updated_at"),
        ("photo_marks", "updated_at"),
        ("photo_embeddings", "rowid"),
    ] {
        let sql = format!(
            "SELECT COUNT(*), COALESCE(MAX({ts}), 0) FROM {table} WHERE user_id = ?1"
        );
        let row: (i64, i64) = conn
            .query_row(&sql, params![user_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap_or((0, 0));
        sig.push_str(&format!("{table}:{}:{}|", row.0, row.1));
    }
    let ms_sig = t.elapsed().as_millis();
    println!("缓存命中路径（只跑签名）      {ms_sig:>5} ms   sig={}…", &sig[..sig.len().min(48)]);

    // 全量聚合
    let t = Instant::now();
    let (total_scanned, no_time, with_face, with_gps, year_min, year_max): (
        i64,
        i64,
        i64,
        i64,
        Option<i32>,
        Option<i32>,
    ) = conn
        .query_row(
            "SELECT COUNT(*),
                SUM(CASE WHEN shoot_time IS NULL OR shoot_time = '' THEN 1 ELSE 0 END),
                SUM(CASE WHEN person_ids IS NOT NULL AND person_ids NOT IN ('', '[]') THEN 1 ELSE 0 END),
                SUM(CASE WHEN lat IS NOT NULL AND lon IS NOT NULL THEN 1 ELSE 0 END),
                MIN(CASE WHEN shoot_time GLOB '[12][0-9][0-9][0-9]*' THEN CAST(substr(shoot_time,1,4) AS INTEGER) END),
                MAX(CASE WHEN shoot_time GLOB '[12][0-9][0-9][0-9]*' THEN CAST(substr(shoot_time,1,4) AS INTEGER) END)
             FROM photo_content_scan WHERE user_id = ?1",
            params![user_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    r.get::<_, Option<i32>>(4)?,
                    r.get::<_, Option<i32>>(5)?,
                ))
            },
        )
        .unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT substr(shoot_time,1,7), COUNT(*) FROM photo_content_scan
             WHERE user_id = ?1 AND shoot_time IS NOT NULL AND length(shoot_time) >= 7
             GROUP BY 1 ORDER BY 1",
        )
        .unwrap();
    let months: Vec<(String, i64)> = stmt
        .query_map(params![user_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    drop(stmt);
    let recent = query_cards(
        conn,
        user_id,
        "SELECT p.path, p.album_id, a.name, p.shoot_time, p.location
         FROM photo_content_scan p
         LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
         WHERE p.user_id = ?1 ORDER BY p.id DESC LIMIT 6",
    );
    let on_this_day_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM photo_content_scan WHERE user_id = ?1
               AND shoot_time IS NOT NULL AND length(shoot_time) >= 10
               AND substr(shoot_time, 6, 5) = '10-27'
               AND CAST(substr(shoot_time,1,4) AS INTEGER) < 2026",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let on_this_day = query_cards(
        conn,
        user_id,
        "SELECT p.path, p.album_id, a.name, p.shoot_time, p.location
         FROM photo_content_scan p
         LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
         WHERE p.user_id = ?1 AND p.shoot_time IS NOT NULL AND length(p.shoot_time) >= 10
           AND substr(p.shoot_time, 6, 5) = '10-27'
           AND CAST(substr(p.shoot_time,1,4) AS INTEGER) < 2026
         ORDER BY p.shoot_time DESC, p.id DESC LIMIT 1",
    )
    .into_iter()
    .next();
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
            "SELECT COUNT(*) FROM photo_categories WHERE user_id = ?1 AND source != 'builtin' AND enabled = 1",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let ms_build = t.elapsed().as_millis();

    let payload = OverviewBench {
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
    };
    let bytes = serde_json::to_vec(&payload).unwrap().len();
    println!(
        "未命中路径（聚合）          {ms_build:>5} ms   快照 {:.2} KB",
        kb(bytes)
    );
    println!(
        "合计：首次 {} ms，之后每次 {} ms（纯签名命中）",
        ms_sig + ms_build,
        ms_sig
    );
}

fn query_cards(conn: &Connection, user_id: i64, sql: &str) -> Vec<Card> {
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.query_map(params![user_id], |r| {
        Ok(Card {
            id: 0,
            path: r.get(0)?,
            album_id: r.get(1)?,
            album_name: r.get(2)?,
            shoot_time: r.get(3)?,
            location: r.get(4)?,
            label: None,
            thumb_path: None,
        })
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn query_month(
    conn: &Connection,
    user_id: i64,
    year: &str,
    month: &str,
    limit: i64,
    offset: i64,
) -> Vec<Card> {
    const BASE: &str = "SELECT p.id, p.path, p.album_id, a.name, p.shoot_time,
                p.location, p.label, t.thumb_path
         FROM photo_content_scan p
         LEFT JOIN albums a ON a.id = p.album_id AND a.user_id = p.user_id
         LEFT JOIN photo_thumb_cache t ON t.photo_hash = p.photo_hash
         WHERE p.user_id = ?1 AND ";
    let (tail, sql_params): (String, Vec<Box<dyn rusqlite::ToSql>>) = if year == "未记录" || year.is_empty() {
        (
            "(p.shoot_time IS NULL OR p.shoot_time = '') ORDER BY p.id ASC LIMIT ?2 OFFSET ?3"
                .to_string(),
            vec![
                Box::new(user_id),
                Box::new(limit),
                Box::new(offset),
            ],
        )
    } else if month.is_empty() {
        (
            "substr(p.shoot_time,1,4)=?2 AND length(p.shoot_time)<7 \
             ORDER BY p.id ASC LIMIT ?3 OFFSET ?4"
                .to_string(),
            vec![
                Box::new(user_id),
                Box::new(year.to_string()),
                Box::new(limit),
                Box::new(offset),
            ],
        )
    } else {
        (
            "substr(p.shoot_time,1,4)=?2 AND substr(p.shoot_time,6,2)=?3 \
             ORDER BY p.id ASC LIMIT ?4 OFFSET ?5"
                .to_string(),
            vec![
                Box::new(user_id),
                Box::new(year.to_string()),
                Box::new(month.to_string()),
                Box::new(limit),
                Box::new(offset),
            ],
        )
    };
    let sql = format!("{BASE}{tail}");
    let mut stmt = conn.prepare(&sql).unwrap();
    let refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
    stmt.query_map(refs.as_slice(), |r| {
        Ok(Card {
            id: r.get(0)?,
            path: r.get(1)?,
            album_id: r.get(2)?,
            album_name: r.get(3)?,
            shoot_time: r.get(4)?,
            location: r.get(5)?,
            label: r.get(6)?,
            thumb_path: r.get(7)?,
        })
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}
