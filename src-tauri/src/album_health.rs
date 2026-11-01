//! 入库体检（FEAT-152）—— 只读诊断：扫完之后数据到底干不干净
//!
//! ## 它回答的问题
//!
//! 「全局扫描入库」把照片写成三类数据：扫描记录（`photo_content_scan` 的 EXIF /
//! 内容 / 影调）、缩略图缓存（`photo_thumb_cache`）、图像向量（`photo_embeddings`）。
//! 三者的写入时机不同（缩略图首次访问才生成、向量只在勾选语义时才写），相册目录
//! 又会被用户全系统里直接改名 / 移动 / 删除 —— 于是库与磁盘必然出现偏差。
//!
//! 这个命令把偏差摊开成一行行可读的事实，供前端给结论与入口：
//!   · 磁盘读数（`album_stats` 缓存）vs 已入库行数 → 有没有漏扫
//!   · 已入库 vs 有缩略图 / 有向量 → 缩略图与语义索引覆盖到哪
//!   · 相册根是否还全 / 抽样路径是否还全 → 记录是否已指向空气
//!   · 缩略图缓存占用 → 缓存成本（与源图体积并列才看得出值不值）
//!
//! ## 为什么是只读 + 抽样
//!
//! - 只读：诊断动作不该改变被诊断对象（不触发重扫、不修库、不写缓存）。
//!   修复入口交给现有的「全局扫描」「预生成缩略图」等命令，由用户显式触发。
//! - 抽样：逐张 stat 全库全原盘 / 网络盘上要几十秒到几分钟，而"这册是不是整体
//!   失联"只需要看几十张。每册抽样上限 [`crate::db::SAMPLE_PER_ALBUM`]，
//!   结论里如实带上"抽样 n 条、失效 m 条"，不冒充全量结论。

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use crate::db::{is_virtual_album_kind, AlbumFacts, SAMPLE_PER_ALBUM};

/// 单个相册的体检结果（**只有事实，不给结论**）
///
/// 结论（正常 / 提示 / 严重）由前端 `utils/albumHealth.ts` 纯函数推导：
/// 判定口径会随产品表述变化，放全一处可测的纯函数里比全后端硬编码更好改。
#[derive(Debug, Clone, Serialize)]
pub struct AlbumHealthRow {
    pub album_id: i64,
    pub name: String,
    pub path: String,
    /// `normal` / `archived` / `hidden`（虚拟相册不参与体检）
    pub kind: String,
    /// 磁盘上的照片数（`album_stats` 缓存口径，不触发重扫）
    pub photo_count: i64,
    /// 磁盘占用（同上）
    pub size_bytes: u64,
    /// 已入库行数（按目录前缀在全库路径上匹配）
    pub scanned: i64,
    /// 有缩略图缓存的照片数
    pub thumbs: i64,
    /// 有图像向量的照片数（任意模型）
    pub embeddings: i64,
    /// 缩略图缓存占用字节
    pub thumb_bytes: i64,
    /// 相册根目录是否还全
    pub root_exists: bool,
    /// 抽样检查了多少条已入库路径
    pub sampled: usize,
    /// 抽样里文件已不存全的条数
    pub missing_files: usize,
}

/// 一次体检的整份报告
#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub albums: Vec<AlbumHealthRow>,
    /// 参与体检的相册数（虚拟相册不计）
    pub album_count: usize,
    pub total_photos: i64,
    pub total_scanned: i64,
    pub total_thumbs: i64,
    pub total_embeddings: i64,
    /// 缩略图缓存总占用
    pub thumb_bytes: i64,
    /// 每册抽样上限（前端据此解释"抽样"含义）
    pub sampled_per_album: usize,
    /// 根目录已不存全的相册数（最严重的一类）
    pub missing_roots: usize,
    pub elapsed_ms: u64,
}

/// 对一批相册做磁盘侧抽样（阻塞 IO，调用方负责放进阻塞线程）
fn probe_disk(rows: &mut [AlbumHealthRow], samples: &HashMap<i64, Vec<String>>) {
    for row in rows.iter_mut() {
        row.root_exists = Path::new(&row.path).is_dir();
        // 根目录都没了就不必再逐张 stat：结论已经确定（整册失联）
        if !row.root_exists {
            continue;
        }
        let Some(list) = samples.get(&row.album_id) else {
            continue;
        };
        row.sampled = list.len();
        row.missing_files = list.iter().filter(|p| !Path::new(p.as_str()).is_file()).count();
    }
}

// ===========================================================================
// 命令层（薄壳；lib.rs 只注册）
// ===========================================================================
pub mod commands {
    use super::*;
    use crate::{logger, require_user, AppState, SessionState};

    /// 入库体检：只读聚合 + 有界抽样，返回逐册事实
    ///
    /// - 不触发重扫、不改库、不写缓存
    /// - 磁盘侧 stat 全阻塞线程执行（相册数 × 抽样上限次）
    #[tauri::command]
    pub async fn album_health_report(
        app: tauri::AppHandle,
        session: tauri::State<'_, SessionState>,
    ) -> Result<HealthReport, String> {
        let _t = log_call!("album_health_report", "只读体检");
        let user_id = require_user(&session)?;

        // 1. 库内事实（一次进内存；不持锁做 IO）
        let (albums, stats, facts) = {
            use tauri::Manager;
            let state = app.state::<AppState>();
            let db = state.0.lock().map_err(|e| format!("{e}"))?;
            let albums = db.get_albums(user_id).map_err(|e| format!("读取相册失败: {e}"))?;
            let paths: Vec<(i64, String)> = albums
                .iter()
                .filter(|a| !is_virtual_album_kind(&a.kind))
                .map(|a| (a.id, a.path.clone()))
                .collect();
            let facts = db
                .album_health_facts(user_id, &paths)
                .map_err(|e| format!("聚合入库事实失败: {e}"))?;
            let stats = db.album_stats_map().map_err(|e| format!("读取相册统计失败: {e}"))?;
            (albums, stats, facts)
        };

        // 2. 逐册拼装（虚拟相册跳过：哨兵路径不是真实目录）
        let mut rows: Vec<AlbumHealthRow> = Vec::new();
        let mut samples: HashMap<i64, Vec<String>> = HashMap::new();
        for a in &albums {
            if is_virtual_album_kind(&a.kind) {
                continue;
            }
            let (photo_count, size_bytes) = stats.get(&a.id).copied().unwrap_or((0, 0));
            let f = facts.get(&a.id).cloned().unwrap_or_default();
            let AlbumFacts { scanned, thumbs, embeddings, thumb_bytes, sample_paths } = f;
            if !sample_paths.is_empty() {
                samples.insert(a.id, sample_paths);
            }
            rows.push(AlbumHealthRow {
                album_id: a.id,
                name: a.name.clone(),
                path: a.path.clone(),
                kind: a.kind.clone(),
                photo_count,
                size_bytes,
                scanned,
                thumbs,
                embeddings,
                thumb_bytes,
                // 磁盘结论全阻塞线程里填
                root_exists: true,
                sampled: 0,
                missing_files: 0,
            });
        }

        // 3. 磁盘侧抽样（阻塞线程：每册最多 SAMPLE_PER_ALBUM 次 stat）
        let started = std::time::Instant::now();
        let rows = tauri::async_runtime::spawn_blocking(move || {
            let mut rows = rows;
            probe_disk(&mut rows, &samples);
            rows
        })
        .await
        .map_err(|e| format!("体检任务失败: {e}"))?;

        let report = HealthReport {
            album_count: rows.len(),
            total_photos: rows.iter().map(|r| r.photo_count).sum(),
            total_scanned: rows.iter().map(|r| r.scanned).sum(),
            total_thumbs: rows.iter().map(|r| r.thumbs).sum(),
            total_embeddings: rows.iter().map(|r| r.embeddings).sum(),
            thumb_bytes: rows.iter().map(|r| r.thumb_bytes).sum(),
            sampled_per_album: SAMPLE_PER_ALBUM,
            missing_roots: rows.iter().filter(|r| !r.root_exists).count(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            albums: rows,
        };

        // 台账：一行总账 + 最需要看的那几册（体检是低频手动操作，逐册明细可控）
        logger::log_info(&format!(
            "[health] 体检完成 | 相册 {} 个 · 磁盘 {} 张 / 已入库 {} 张 / 缩略图 {} / 向量 {} · 缓存 {} · 根失效 {} 册 · 抽样上限 {SAMPLE_PER_ALBUM}/册 · {}ms",
            report.album_count,
            report.total_photos,
            report.total_scanned,
            report.total_thumbs,
            report.total_embeddings,
            crate::dedupe::fmt_bytes(report.thumb_bytes.max(0) as u64),
            report.missing_roots,
            report.elapsed_ms
        ));
        for r in report.albums.iter().filter(|r| !r.root_exists) {
            logger::log_warn(&format!("[health] 相册根不存全：{} · {}", r.name, r.path));
        }
        for r in report.albums.iter().filter(|r| r.missing_files > 0) {
            logger::log_warn(&format!(
                "[health] 相册「{}」抽样 {} 条里有 {} 条文件已不存全",
                r.name, r.sampled, r.missing_files
            ));
        }

        logger::log_call_end_with(
            "album_health_report",
            _t,
            &format!(
                "OK | albums={} roots_missing={} scanned={}/{} ms={}",
                report.album_count,
                report.missing_roots,
                report.total_scanned,
                report.total_photos,
                report.elapsed_ms
            ),
        );
        Ok(report)
    }
}
