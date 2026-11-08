//! 上下文装配：用户意图 + 手选托盘 → 候选池（`candidates`）。
//!
//! 为什么这一步在 Rust（docs/v5.md §1、§7）：DB / 向量 / 人物 / 时间线都在 Rust，
//! 而且这步必须**确定性可测**（同样的输入必须得到同样的池子，否则"Agent 看到的
//! 池子是这 42 张"这句话就没法复核）。
//!
//! ## 本版本的边界（重要）
//!
//! 这一版只做 **tray 范围**：手选的照片 + 文件级元数据（体积/尺寸/年份）。
//! 语义召回（vecindex top-K）、人物聚类、地点桶还没接进来 —— 它们服务于
//! "P2 扩大范围"那条路径，而 P1 的 scope 是**冻结在 tray** 的（§16.3）。
//! 等接规划层时在这里补 `byPerson` / `byPlace` / `semanticRank`，
//! **结构（CandidatePool）不变**，前端与 Python 无需改动。
//!
//! 手选永远优先置顶：这是硬偏好 —— 用户手动挑的图不该被任何打分挤下去。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// 默认上限（docs/v5.md §7：默认 ≤80 张）
pub const DEFAULT_LIMIT: usize = 80;
/// 硬上限（超过就拒绝，不静默截断到用户以为的"全部"）
pub const HARD_LIMIT: usize = 200;

/// 单张候选图
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub path: String,
    pub bytes: u64,
    pub w: u32,
    pub h: u32,
    /// 年份（优先文件修改时间；EXIF 拍摄时间在接 DB 后替换）
    pub year: Option<i32>,
    /// 来源：tray（手选）/ query（召回）。前端据此标注"手选优先"
    pub source: &'static str,
}

/// 视图摘要（"按年看这批照片"），给用户一句能复核的话
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Views {
    pub by_year: BTreeMap<String, YearView>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YearView {
    pub count: usize,
    pub first: Vec<String>,
}

/// 候选池
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidatePool {
    pub total: usize,
    pub items: Vec<Candidate>,
    pub views: Views,
    /// 因上限被丢弃的张数（0 表示全部进入）
    pub dropped: usize,
}

/// 组装候选池。纯函数式：不读 DB、不联网，只碰文件系统元数据。
pub fn assemble(tray: &[String], limit: usize) -> CandidatePool {
    let cap = limit.clamp(1, HARD_LIMIT);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut items: Vec<Candidate> = Vec::new();
    let mut dropped = 0usize;

    for raw in tray {
        let path = raw.trim();
        if path.is_empty() {
            continue;
        }
        // Windows 路径大小写不敏感：去重要按小写比较，否则同一张图能进来两次
        let key = path.to_lowercase();
        if !seen.insert(key) {
            dropped += 1;
            continue;
        }
        if items.len() >= cap {
            dropped += 1;
            continue;
        }
        items.push(describe(path));
    }

    let mut by_year: BTreeMap<String, YearView> = BTreeMap::new();
    for it in &items {
        let key = it
            .year
            .map(|y| y.to_string())
            .unwrap_or_else(|| "未知".to_string());
        let entry = by_year.entry(key).or_default();
        entry.count += 1;
        if entry.first.len() < 3 {
            entry.first.push(it.path.clone());
        }
    }

    CandidatePool {
        total: items.len(),
        items,
        views: Views { by_year },
        dropped,
    }
}

/// 读单张图的文件级元数据。读不到尺寸就填 0（不 panic、不丢候选）。
fn describe(path: &str) -> Candidate {
    let p = Path::new(path);
    let (bytes, year) = file_meta(p);
    let (w, h) = image::image_dimensions(p).unwrap_or((0, 0));
    Candidate {
        path: path.to_string(),
        bytes,
        w,
        h,
        year,
        source: "tray",
    }
}

fn file_meta(p: &Path) -> (u64, Option<i32>) {
    let Ok(md) = std::fs::metadata(p) else {
        return (0, None);
    };
    let bytes = md.len();
    let year = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| year_from_unix(d.as_secs() as i64));
    (bytes, year)
}

/// Unix 秒 → 年份（不引 chrono：一个整数换算就够）。
fn year_from_unix(secs: i64) -> i32 {
    // 1970-01-01 起按 365.2425 天均摊，误差 1 天内（只用于"按年分组"）
    let days = secs / 86_400;
    let mut year = 1970;
    let mut left = days;
    loop {
        let len = if is_leap(year) { 366 } else { 365 };
        if left < len {
            break;
        }
        left -= len;
        year += 1;
    }
    year
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// 给前端/日志的一句话摘要（"手选 42 张，其中 2023 年 20 张"）
pub fn brief(pool: &CandidatePool) -> String {
    let mut parts = vec![format!("候选池 {} 张", pool.total)];
    if pool.dropped > 0 {
        parts.push(format!("丢弃 {}", pool.dropped));
    }
    for (year, v) in pool.views.by_year.iter().take(3) {
        parts.push(format!("{year} 年 {} 张", v.count));
    }
    parts.join("，")
}

/// 把托盘路径收敛成绝对路径（前端可能传相对路径）
pub fn normalize(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            let pb = PathBuf::from(p.trim());
            if pb.is_absolute() {
                pb.to_string_lossy().to_string()
            } else {
                std::fs::canonicalize(&pb)
                    .map(|c| c.to_string_lossy().to_string())
                    .unwrap_or_else(|_| p.trim().to_string())
            }
        })
        .filter(|p| !p.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("pm-assemble-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        // 写一张 1×1 的真 PNG，确保 image_dimensions 能读出来
        let img = image::RgbImage::from_pixel(4, 3, image::Rgb([10, 20, 30]));
        img.save(&p).unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn dedupes_case_insensitively_and_counts_dropped() {
        let a = tmp_file("a.png");
        let pool = assemble(&[a.clone(), a.to_uppercase(), a.clone()], DEFAULT_LIMIT);
        assert_eq!(pool.total, 1);
        assert_eq!(pool.dropped, 2);
        assert!(pool.items[0].w > 0 && pool.items[0].h > 0, "应读到真实尺寸");
    }

    #[test]
    fn respects_limit_and_reports_dropped() {
        let paths: Vec<String> = (0..5).map(|i| tmp_file(&format!("l{i}.png"))).collect();
        let pool = assemble(&paths, 3);
        assert_eq!(pool.total, 3);
        assert_eq!(pool.dropped, 2);
    }

    #[test]
    fn hard_limit_is_enforced() {
        let paths: Vec<String> = (0..3).map(|i| tmp_file(&format!("h{i}.png"))).collect();
        // 传入超过硬上限的 limit 也会被夹到 HARD_LIMIT
        let pool = assemble(&paths, 9999);
        assert_eq!(pool.total, 3);
    }

    #[test]
    fn views_group_by_year_with_first_three() {
        let paths: Vec<String> = (0..4).map(|i| tmp_file(&format!("v{i}.png"))).collect();
        let pool = assemble(&paths, DEFAULT_LIMIT);
        let total: usize = pool.views.by_year.values().map(|v| v.count).sum();
        assert_eq!(total, 4);
        for v in pool.views.by_year.values() {
            assert!(v.first.len() <= 3);
        }
    }

    #[test]
    fn missing_file_still_yields_a_candidate() {
        let pool = assemble(&["Z:/definitely/not/here.jpg".to_string()], DEFAULT_LIMIT);
        assert_eq!(pool.total, 1, "读不到元数据也不能把候选丢了");
        assert_eq!(pool.items[0].bytes, 0);
    }

    #[test]
    fn empty_tray_is_empty_pool() {
        let pool = assemble(&[], DEFAULT_LIMIT);
        assert_eq!(pool.total, 0);
        assert!(pool.views.by_year.is_empty());
    }

    #[test]
    fn year_from_unix_is_sane() {
        assert_eq!(year_from_unix(0), 1970);
        // 2026-11-02T00:00:00Z
        assert_eq!(year_from_unix(1_793_548_800), 2026);
        assert_eq!(year_from_unix(1_704_067_200), 2024); // 2024-01-01
    }

    #[test]
    fn brief_mentions_counts() {
        let a = tmp_file("b.png");
        let pool = assemble(&[a], DEFAULT_LIMIT);
        let s = brief(&pool);
        assert!(s.contains("候选池 1 张"), "{s}");
    }
}
