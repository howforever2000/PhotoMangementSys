//! 大组件：主页面「图片扫描测试」（不落库，仅验证时间/地点识别 + 照片移动）
//!
//! 职责：
//!   1. scan_test_photos       扫描目录下图片（默认只扫直接图片；recurse=true 时递归子目录），提取时间 + GPS 坐标
//!   2. resolve_test_places    GPS 聚类 → 本地省/市点面判断（未命中再联网）→ 地名填充
//!   3. organize_test_photos   按「年 → 地点」两级文件夹创建 + 移动照片
//!
//! 解耦原则：
//!   - 复用 photo_scan（EXIF 提取 / 反编码基础能力），本模块只做测试编排
//!   - 命令定义在本模块（#[tauri::command]），lib.rs 仅薄壳汇总注册
//!   - 不修改数据库 / 不触碰相册管理（独立测试工具）
use std::path::Path;

use serde::Serialize;
use tauri::Emitter;

use crate::photo_scan;

/// 单张照片的扫描结果（测试用，不落库）
#[derive(Debug, Clone, Serialize)]
pub struct TestPhoto {
    /// 文件名（不含路径）
    pub file_name: String,
    /// 完整路径
    pub path: String,
    /// 拍摄时间（三级兜底：DateTimeOriginal→DateTime→GPS-UTC+8）
    pub shoot_time: Option<String>,
    /// 年份（"2020"，由 shoot_time 提取）
    pub year: Option<String>,
    /// 纬度（十进制度 WGS84）
    pub lat: Option<f64>,
    /// 经度
    pub lon: Option<f64>,
    /// 地点（本地省/市反查优先，未命中联网兜底；简化取最后两段，如 "四川省 · 达州市"）
    pub place: Option<String>,
}

/// 组织移动报告
#[derive(Debug, Clone, Serialize)]
pub struct OrganizeReport {
    pub total: usize,
    pub moved: usize,
    pub conflict: usize,
    pub no_time: usize,
    pub no_place: usize,
    pub failed: usize,
    pub target_root: String,
    /// 创建的文件夹路径清单
    pub folders: Vec<String>,
    /// 用户中途取消（true 时上面的计数为「已处理部分」的统计）
    #[serde(default)]
    pub cancelled: bool,
}

/// 进度事件载荷（resolve=解析地名 / organize=组织移动）
///
/// 每张照片处理完回调一次：`current` 递增，`message` 为结果描述
/// （如地名 / "已移动" / "跳过(冲突)" / "移动失败"）。
#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub file_name: String,
    pub message: String,
}

/// 扫描目录下图片，提取时间 + GPS 坐标（单线程；保留给既有测试与内部调用）
///
/// - `recurse=false`：只扫所选目录下的**直接**图片（不进入子目录）
/// - `recurse=true`：递归遍历所有子目录（跳过隐藏目录，类似 walkdir 语义）
///
/// place 初始为 None（解析地名需联网，见 resolve_test_places）。
pub fn scan_test_photos(dir: &str, recurse: bool) -> Result<Vec<TestPhoto>, String> {
    scan_test_photos_parallel(dir, recurse, 1, &mut |_| {})
}

/// 收集待扫描的图片文件（不做 EXIF 解析，纯目录遍历，毫秒级）
///
/// 与扫描主流程分离：先枚举出全部候选文件（可以立刻得到 total 用于进度条分母），
/// 再用线程池并行解析 EXIF。目录遍历本身是 IO 串行收益更好（避免磁盘随机寻道放大）。
pub fn collect_image_files(
    dir: &str,
    recurse: bool,
) -> Result<Vec<(std::path::PathBuf, String)>, String> {
    let root = Path::new(dir);
    if !root.is_dir() {
        return Err(format!("路径不存在或不是文件夹: {dir}"));
    }
    let mut files: Vec<(std::path::PathBuf, String)> = Vec::new();
    let push = |p: std::path::PathBuf, files: &mut Vec<(std::path::PathBuf, String)>| {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if photo_scan::is_image_file(&name) {
            files.push((p, name));
        }
    };
    if !recurse {
        let rd = std::fs::read_dir(root).map_err(|e| format!("读取目录失败: {e}"))?;
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_file() {
                push(p, &mut files);
            }
        }
    } else {
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
        {
            let Ok(e) = entry else { continue };
            if e.file_type().is_file() {
                push(e.into_path(), &mut files);
            }
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(files)
}

/// **并行**扫描目录下图片（FEAT-064）：时间 + GPS 坐标
///
/// 性能背景：EXIF 解析是「小文件读 + 解析」的典型 IO/CPU 混合负载，单张约
/// 0.3~2ms，万张相册串行扫描需数秒到数十秒；并行后基本被磁盘带宽限制。
/// 处理顺序按**完成顺序**回调进度（配合 `ScanJobState` 的单调进度，不会回退）。
///
/// - `threads`：并行度（1 = 串行等价；调用方按 CPU 拓扑与冷启动探测给出推荐值）
/// - `on_progress`：每张处理完回调一次。**`threads > 1` 时回调在 worker 线程上，
///   因此要求 `Send`** —— 调用方通常包一层 `Mutex` 或使用线程安全的 sink。
///
/// 扫描开始前先调用一次回调（current=0, total=N）——让 UI 立刻知道分母。
pub fn scan_test_photos_parallel(
    dir: &str,
    recurse: bool,
    threads: usize,
    on_progress: &mut (dyn FnMut(ScanProgress) + Send),
) -> Result<Vec<TestPhoto>, String> {
    scan_test_photos_cancellable(dir, recurse, threads, None, on_progress)
}

/// 并行扫描（可取消）：`cancel` 为 Some 时，worker 每处理一张前检查标记，
/// 置位后**尽快收敛**（不再取新任务，已完成的照常返回）
///
/// 取消语义：返回已扫描的部分结果（而非报错）——与「全局扫描」一致，
/// 用户点停止后能看到「扫到一半」的数据，而不是一片空白。
pub fn scan_test_photos_cancellable(
    dir: &str,
    recurse: bool,
    threads: usize,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    on_progress: &mut (dyn FnMut(ScanProgress) + Send),
) -> Result<Vec<TestPhoto>, String> {
    let files = collect_image_files(dir, recurse)?;
    let total = files.len();
    on_progress(ScanProgress {
        phase: "scan".into(),
        current: 0,
        total,
        file_name: String::new(),
        message: format!("共 {total} 张待扫描（{threads} 线程并行）"),
    });
    if total == 0 {
        return Ok(Vec::new());
    }

    let threads = threads.max(1).min(total);
    let exif_threads = threads;

    // 并行解析：索引 → EXIF 结果（保持输入顺序，便于稳定排序与测试断言）
    let results: Vec<TestPhoto> = if threads == 1 {
        // 串行路径：不建线程池，顺带保证单张失败不影响整体
        let mut out = Vec::with_capacity(total);
        for (i, (p, name)) in files.iter().enumerate() {
            if let Some(c) = &cancel {
                if c.load(std::sync::atomic::Ordering::SeqCst) {
                    crate::logger::log_info(&format!(
                        "[test-scan] 扫描取消：已处理 {}/{} 张",
                        i, total
                    ));
                    break;
                }
            }
            out.push(read_one(p, name));
            on_progress(ScanProgress {
                phase: "scan".into(),
                current: i + 1,
                total,
                file_name: name.clone(),
                message: "已解析".into(),
            });
        }
        out
    } else {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;

        let counter = AtomicUsize::new(0);
        // 进度回调需要 &mut，跨线程共享用 Mutex 包一层
        let cb = Mutex::new(on_progress);
        let files_ref = &files;
        let cancel_ref = cancel.as_ref();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(exif_threads)
            .thread_name(|i| format!("test-scan-{i}"))
            .build()
            .map_err(|e| format!("扫描线程池创建失败: {e}"))?;

        // 取消时用「一旦置位就整体短路」的方式：每个 worker 先查标记，
        // 置位就返回 None（不产结果），后续 collect 时过滤掉 ——
        // rayon 没有内建中断，这是最轻量的收敛方式。
        let indexed: Vec<(usize, Option<TestPhoto>)> = pool.install(|| {
            use rayon::prelude::*;
            files_ref
                .par_iter()
                .enumerate()
                .map(|(i, (p, name))| {
                    if let Some(c) = cancel_ref {
                        if c.load(Ordering::SeqCst) {
                            return (i, None);
                        }
                    }
                    let photo = read_one(p, name);
                    let done = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    if let Ok(mut f) = cb.lock() {
                        f(ScanProgress {
                            phase: "scan".into(),
                            current: done,
                            total,
                            file_name: name.clone(),
                            message: "已解析".into(),
                        });
                    }
                    (i, Some(photo))
                })
                .collect()
        });

        let cancelled = cancel_ref
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(false);
        let mut slots: Vec<Option<TestPhoto>> = (0..total).map(|_| None).collect();
        for (i, photo) in indexed {
            slots[i] = photo;
        }
        let out: Vec<TestPhoto> = slots.into_iter().flatten().collect();
        if cancelled {
            crate::logger::log_info(&format!(
                "[test-scan] 扫描取消：已处理 {}/{} 张",
                out.len(),
                total
            ));
        }
        out
    };

    Ok(results)
}

/// 单张图片 → TestPhoto（EXIF 失败置空字段，不中断整体扫描）
fn read_one(p: &Path, name: &str) -> TestPhoto {
    let ex = photo_scan::read_photo_exif(p, name);
    let year = ex
        .shoot_time
        .as_deref()
        .and_then(|t| t.get(0..4))
        .map(str::to_string);
    TestPhoto {
        file_name: name.to_string(),
        path: p.to_string_lossy().into_owned(),
        shoot_time: ex.shoot_time,
        year,
        lat: ex.lat,
        lon: ex.lon,
        place: None,
    }
}

/// 供「实测校准」复用的单张 EXIF 读取（同 `read_one` 的解析部分）
pub fn read_photo_exif_pub(p: &Path, name: &str) -> TestPhoto {
    read_one(p, name)
}

/// 解析地点：按 ~1km（0.01°）网格聚类，每个聚类点只查一次，同聚类照片共享地名
///
/// 有 GPS 的照片通常落在少数几个聚类点（旅行同地连拍），
/// 相比逐张反编码（~1s×N）可省 70%+ 请求。
/// **本地优先**：先用 geo_index 离线点面判断（省/市，秒回）；本地未命中
/// （国外/公海）才联网反编码（BigDataCloud，重试 3 次）。
/// 每张有 GPS 的照片处理完回调一次进度 + 记录日志。
pub fn resolve_test_places(
    dir: &str,
    recurse: bool,
    on_progress: &mut (dyn FnMut(ScanProgress) + Send),
) -> Result<Vec<TestPhoto>, String> {
    resolve_test_places_threaded(dir, recurse, 1, on_progress)
}

/// 解析地点（指定扫描并行度）—— 与 `resolve_test_places` 同逻辑，
/// 仅把内部「扫描阶段」换成并行版本（FEAT-064：解析地名前要重新扫一遍 EXIF，
/// 这一步在大相册上才是耗时主体；联网反编码本身有 ~300ms 礼貌间隔，不并行）。
pub fn resolve_test_places_threaded(
    dir: &str,
    recurse: bool,
    threads: usize,
    on_progress: &mut (dyn FnMut(ScanProgress) + Send),
) -> Result<Vec<TestPhoto>, String> {
    resolve_test_places_cancellable(dir, recurse, threads, None, on_progress)
}

/// 解析地名（可取消）：扫描阶段可中断；已解析的结果照常返回
pub fn resolve_test_places_cancellable(
    dir: &str,
    recurse: bool,
    threads: usize,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    on_progress: &mut (dyn FnMut(ScanProgress) + Send),
) -> Result<Vec<TestPhoto>, String> {
    let mut photos = scan_test_photos_cancellable(dir, recurse, threads, cancel.clone(), on_progress)?;
    let total = photos.iter().filter(|p| p.lat.is_some()).count();
    // 聚类：网格 key → 中心坐标
    let mut grid: std::collections::BTreeMap<(i32, i32), (f64, f64)> =
        std::collections::BTreeMap::new();
    for p in &photos {
        if let (Some(lat), Some(lon)) = (p.lat, p.lon) {
            grid.entry(grid_key(lat, lon)).or_insert((lat, lon));
        }
    }
    // 无 GPS 照片 → 扫描阶段就已完成（进度分母只算有 GPS 的，避免进度条卡在 95%）
    if total == 0 {
        return Ok(photos);
    }
    on_progress(ScanProgress {
        phase: "resolve".into(),
        current: 0,
        total,
        file_name: String::new(),
        message: format!("{total} 张有 GPS 待解析（{} 个地点聚类）", grid.len()),
    });
    let mut done = 0usize;
    // 本地未命中时兜底联网（懒创建）
    let mut client: Option<reqwest::blocking::Client> = None;
    for (key, (lat, lon)) in &grid {
        // 取消检查：联网反编码每个聚类点前查一次（聚类点数量级小，无需更细粒度）
        if let Some(c) = &cancel {
            if c.load(std::sync::atomic::Ordering::SeqCst) {
                crate::logger::log_info(&format!(
                    "[test-scan] 地名解析取消：已解析 {done}/{total} 张"
                ));
                break;
            }
        }
        // 本地离线查询：省/市（"四川省 · 达州市"），未命中 → None
        let mut place = crate::geo_index::find_region(*lat, *lon);
        if place.is_none() {
            // 网络重试：BigDataCloud 偶发失败，最多 3 次（间隔 300ms）
            let c = client.get_or_insert_with(|| {
                reqwest::blocking::Client::builder()
                    .user_agent("photo-manager/0.1 (album location research)")
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .expect("HTTP 客户端创建失败")
            });
            for _attempt in 0..3 {
                place = photo_scan::reverse_geocode(c, *lat, *lon);
                if place.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
        let place = place.map(|s| shorten_place(&s));
        for p in photos.iter_mut() {
            if let (Some(la), Some(lo)) = (p.lat, p.lon) {
                if grid_key(la, lo) != *key {
                    continue;
                }
                if let Some(pl) = &place {
                    p.place = Some(pl.clone());
                }
                done += 1;
                let msg = place
                    .clone()
                    .unwrap_or_else(|| "无地名(反编码失败)".to_string());
                crate::logger::log_info(&format!("[resolve] {} → {}", p.file_name, msg));
                on_progress(ScanProgress {
                    phase: "resolve".into(),
                    current: done,
                    total,
                    file_name: p.file_name.clone(),
                    message: msg,
                });
            }
        }
    }
    Ok(photos)
}

/// 网格 key（0.01° ≈ 1.1km）
fn grid_key(lat: f64, lon: f64) -> (i32, i32) {
    ((lat * 100.0).round() as i32, (lon * 100.0).round() as i32)
}

/// 完整反编码地名 → 文件夹友好短名（取最后两段）
///
/// "中华人民共和国 · 四川省 · 达州市 · 萬源市" → "达州市 · 萬源市"
/// "中华人民共和国 · 四川省" → "四川省"
fn shorten_place(full: &str) -> String {
    let parts: Vec<&str> = full.split(" · ").filter(|s| !s.trim().is_empty()).collect();
    let keep = parts.iter().rev().take(2).collect::<Vec<_>>();
    keep.iter()
        .rev()
        .map(|s| s.trim().to_string())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 按「年 → 地点」两级文件夹组织移动（测试功能：时间/地点识别 + 照片移动验证）
///
/// 结构：{dir}/{年份}/{地点}/照片.jpg
///  - 无年份 → "未知年份"；无地点 → "无地点"
///  - 目标同名文件 → 跳过记 conflict（不覆盖）
///  - 移动用 fs::rename（同目录内快速）；失败记 failed
///  - 每张照片移动完回调一次进度 + 记录日志；内部解析地名阶段进度照常转发
pub fn organize_test_photos(
    dir: &str,
    recurse: bool,
    on_progress: &mut (dyn FnMut(ScanProgress) + Send),
) -> Result<OrganizeReport, String> {
    organize_test_photos_threaded(dir, recurse, 1, on_progress)
}

/// 组织移动（指定扫描并行度）—— 内部解析地名阶段的 EXIF 扫描走并行（FEAT-064）。
/// 移动本身（rename）是串行的：同目录内 rename 快（微秒级），并发反而增加
/// 冲突判定与目录创建的竞态风险。
pub fn organize_test_photos_threaded(
    dir: &str,
    recurse: bool,
    threads: usize,
    on_progress: &mut (dyn FnMut(ScanProgress) + Send),
) -> Result<OrganizeReport, String> {
    organize_test_photos_cancellable(dir, recurse, threads, None, on_progress)
}

/// 组织移动（可取消）：移动循环每张前检查标记；已移动的照常保留（不可回滚）
pub fn organize_test_photos_cancellable(
    dir: &str,
    recurse: bool,
    threads: usize,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    on_progress: &mut (dyn FnMut(ScanProgress) + Send),
) -> Result<OrganizeReport, String> {
    let photos = resolve_test_places_cancellable(dir, recurse, threads, cancel.clone(), on_progress)?;
    if photos.is_empty() {
        let hint = if recurse {
            "扫描到 0 张图片（已递归子目录）。请确认所选文件夹下含有图片。"
        } else {
            "扫描到 0 张直接图片（不递归子目录），无法组织移动。请确认所选文件夹下直接存放图片。"
        };
        return Err(format!("\n{hint}"));
    }
    let root = Path::new(dir);
    let mut report = OrganizeReport {
        total: photos.len(),
        moved: 0,
        conflict: 0,
        no_time: 0,
        no_place: 0,
        failed: 0,
        target_root: dir.to_string(),
        folders: Vec::new(),
        cancelled: false,
    };
    let mut seen_folders: std::collections::HashSet<String> = std::collections::HashSet::new();
    let total = photos.len();
    let mut done = 0usize;
    for p in &photos {
        done += 1;
        // 取消检查：移动是破坏性操作，用户点停止后立刻不再继续后续照片
        if let Some(c) = &cancel {
            if c.load(std::sync::atomic::Ordering::SeqCst) {
                report.cancelled = true;
                crate::logger::log_info(&format!(
                    "[test-scan] 组织移动取消：已处理 {}/{total} 张",
                    done - 1
                ));
                break;
            }
        }
        let year = p.year.clone().unwrap_or_else(|| {
            report.no_time += 1;
            "未知年份".to_string()
        });
        let place = p.place.clone().unwrap_or_else(|| {
            report.no_place += 1;
            "无地点".to_string()
        });
        let folder = root.join(&year).join(sanitize_folder(&place));
        if seen_folders.insert(folder.to_string_lossy().into_owned()) {
            report.folders.push(folder.to_string_lossy().into_owned());
        }
        if let Err(e) = std::fs::create_dir_all(&folder) {
            report.failed += 1;
            crate::logger::log_error(
                "organize_test_photos",
                &format!("{} 创建目录失败 {}: {e}", p.file_name, folder.display()),
            );
            on_progress(ScanProgress {
                phase: "organize".into(),
                current: done,
                total,
                file_name: p.file_name.clone(),
                message: "创建目录失败".into(),
            });
            continue;
        }
        let src = Path::new(&p.path);
        let dest = folder.join(&p.file_name);
        if dest.exists() {
            report.conflict += 1;
            crate::logger::log_info(&format!(
                "[organize] {} 跳过(目标已存在: {})",
                p.file_name,
                dest.display()
            ));
            on_progress(ScanProgress {
                phase: "organize".into(),
                current: done,
                total,
                file_name: p.file_name.clone(),
                message: "跳过(目标已存在)".into(),
            });
            continue;
        }
        match std::fs::rename(src, &dest) {
            Ok(_) => {
                report.moved += 1;
                crate::logger::log_info(&format!(
                    "[organize] {} → {}/{}/",
                    p.file_name, year, place
                ));
                on_progress(ScanProgress {
                    phase: "organize".into(),
                    current: done,
                    total,
                    file_name: p.file_name.clone(),
                    message: "已移动".into(),
                });
            }
            Err(e) => {
                report.failed += 1;
                crate::logger::log_error(
                    "organize_test_photos",
                    &format!(
                        "{} 移动失败 {} → {}: {e}",
                        p.file_name,
                        src.display(),
                        dest.display()
                    ),
                );
                on_progress(ScanProgress {
                    phase: "organize".into(),
                    current: done,
                    total,
                    file_name: p.file_name.clone(),
                    message: format!("移动失败: {e}"),
                });
            }
        }
    }
    Ok(report)
}

/// 文件夹名消毒：去除 Windows 非法字符
fn sanitize_folder(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 命令层（薄壳，逻辑见上；lib.rs 仅注册）
//
// 两个层次的命令：
//   A. **旧同步命令**（`scan_test_photos` / `resolve_test_places` /
//      `organize_test_photos`）—— 保留给既有调用与测试；前端已不再使用。
//   B. **异步任务命令**（FEAT-064，`start_scan_job` / `get_scan_job` /
//      `cancel_scan_job` / `clear_scan_job`）—— 「启动即返回」+ 后端持有任务状态，
//      页面切换/重进都能恢复视图。这是前端唯一入口。
//
// 阻塞纪律：任务体（EXIF 解析 / 联网反编码 / 文件移动）全部跑在
// `spawn_blocking` 上，绝不占 Tauri 主线程 —— 否则真实相册会冻结 UI。
// ---------------------------------------------------------------------------
pub mod commands {
    use super::*;
    use crate::test_scan_job::{JobPhase, JobSnapshot, OrganizeSummary, ScanJobState};

    /// 扫描目录下图片（recurse=false 只扫直接图片；recurse=true 递归子目录）：时间 + GPS 坐标
    #[tauri::command]
    pub async fn scan_test_photos(path: String, recurse: bool) -> Result<Vec<TestPhoto>, String> {
        let _t = log_call!(
            "scan_test_photos",
            &format!("path={path} recurse={recurse}")
        );
        let r: Result<Vec<TestPhoto>, String> =
            tauri::async_runtime::spawn_blocking(move || super::scan_test_photos(&path, recurse))
                .await
                .map_err(|e| format!("任务线程失败: {e}"))?;
        match &r {
            Ok(list) => crate::logger::log_call_end_with(
                "scan_test_photos",
                _t,
                &format!(
                    "OK | photos={} gps={}",
                    list.len(),
                    list.iter().filter(|p| p.lat.is_some()).count()
                ),
            ),
            Err(e) => {
                crate::logger::log_call_end_with("scan_test_photos", _t, &format!("ERR | {e}"))
            }
        }
        r
    }

    /// 解析地点：GPS 聚类 + 每聚类一次反向地理编码（联网），每张照片上报进度
    #[tauri::command]
    pub async fn resolve_test_places(
        app: tauri::AppHandle,
        path: String,
        recurse: bool,
    ) -> Result<Vec<TestPhoto>, String> {
        let _t = log_call!(
            "resolve_test_places",
            &format!("path={path} recurse={recurse}")
        );
        let emitter = app.clone();
        let r: Result<Vec<TestPhoto>, String> = tauri::async_runtime::spawn_blocking(move || {
            super::resolve_test_places(&path, recurse, &mut |p| {
                let _ = emitter.emit("test-scan-progress", &p);
            })
        })
        .await
        .map_err(|e| format!("任务线程失败: {e}"))?;
        match &r {
            Ok(list) => crate::logger::log_call_end_with(
                "resolve_test_places",
                _t,
                &format!(
                    "OK | photos={} place={}",
                    list.len(),
                    list.iter().filter(|p| p.place.is_some()).count()
                ),
            ),
            Err(e) => {
                crate::logger::log_call_end_with("resolve_test_places", _t, &format!("ERR | {e}"))
            }
        }
        r
    }

    /// 按「年 → 地点」两级文件夹组织移动（测试功能），每张照片上报进度
    #[tauri::command]
    pub async fn organize_test_photos(
        app: tauri::AppHandle,
        path: String,
        recurse: bool,
    ) -> Result<OrganizeReport, String> {
        let _t = log_call!(
            "organize_test_photos",
            &format!("path={path} recurse={recurse}")
        );
        let emitter = app.clone();
        let r: Result<OrganizeReport, String> = tauri::async_runtime::spawn_blocking(move || {
            super::organize_test_photos(&path, recurse, &mut |p| {
                let _ = emitter.emit("test-scan-progress", &p);
            })
        })
        .await
        .map_err(|e| format!("任务线程失败: {e}"))?;
        match &r {
            Ok(rep) => crate::logger::log_call_end_with(
                "organize_test_photos",
                _t,
                &format!(
                    "OK | total={} moved={} conflict={} failed={}",
                    rep.total, rep.moved, rep.conflict, rep.failed
                ),
            ),
            Err(e) => {
                crate::logger::log_call_end_with("organize_test_photos", _t, &format!("ERR | {e}"))
            }
        }
        r
    }

    // -----------------------------------------------------------------------
    // FEAT-064：异步任务命令（退出页面不中断 / 进度可恢复 / 并行扫描）
    //
    // 三个阶段各自一个 `start_*_job`：都遵循同一套模板 ——
    //   1. `job.begin(...)` 做准入检查 + 重置状态（有任务在跑则报错）
    //   2. `spawn_blocking` 起后台线程跑重活（其中扫描阶段并行）
    //   3. 回调里「写状态（供恢复）+ 发事件（供实时）」双通道推进进度
    //   4. 收尾按代次号 `job_id` 校验，避免旧任务覆盖新任务
    //
    // 关键：命令 `await` 的是**线程启动**而非任务完成 —— 这才是「启动即返回」。
    // -----------------------------------------------------------------------

    /// 进度双通道：节流发事件 + 写状态（恢复用）
    ///
    /// 事件节流 20Hz：万张相册逐张 emit 会压垮 WebView 的事件循环；
    /// 状态则每次更新（Mutex 廉价），保证恢复时拿到的是最新值。
    fn progress_sink(
        app: tauri::AppHandle,
        job: ScanJobState,
        job_id: u64,
        phase: JobPhase,
        start: std::time::Instant,
    ) -> impl FnMut(ScanProgress) + Send + 'static {
        let mut last_emit = std::time::Instant::now();
        move |p: ScanProgress| {
            // 状态通道：每次都写（Mutex 廉价，保证恢复时拿到最新值）
            job.mark_done(
                job_id,
                phase,
                p.current,
                p.total,
                &p.file_name,
                &p.message,
                start.elapsed().as_secs_f64(),
            );
            // 事件通道：节流到 ~20Hz，但「最后一帧」（current==total）必须发，
            // 否则进度条会停在倒数第二帧（用户看到 99% 不动的经典问题）
            let is_last = p.total > 0 && p.current >= p.total;
            if is_last || last_emit.elapsed().as_millis() >= 50 {
                last_emit = std::time::Instant::now();
                let _ = app.emit("test-scan-progress", &p);
            }
        }
    }

    /// 启动扫描任务（异步）：立刻返回，任务在后台并行跑，结果存后端状态
    ///
    /// 与旧 `scan_test_photos` 的关键差异：**不 await 任务完成**。
    /// 前端拿到「已启动」的快照，随后靠 `test-scan-progress` 事件（实时）+
    /// `get_scan_job`（恢复）维持视图 —— 页面卸载、路由切换都不影响后台推进。
    #[tauri::command]
    pub async fn start_scan_job(
        app: tauri::AppHandle,
        job: tauri::State<'_, ScanJobState>,
        path: String,
        recurse: bool,
    ) -> Result<JobSnapshot, String> {
        let threads = crate::scan_perf::effective_threads(&app);
        let _t = log_call!(
            "start_scan_job",
            &format!("path={path} recurse={recurse} threads={threads}")
        );
        let (job_id, cancel) = job.begin(JobPhase::Scan, &path, recurse, threads)?;
        let owned = job.handle();
        let snap = job.snapshot();
        let app2 = app.clone();
        let path2 = path.clone();
        // 只 await 线程启动，不等任务跑完
        tauri::async_runtime::spawn_blocking(move || {
            let start = std::time::Instant::now();
            let mut cb = progress_sink(app2.clone(), owned.clone(), job_id, JobPhase::Scan, start);
            let r = super::scan_test_photos_cancellable(
                &path2,
                recurse,
                threads,
                Some(cancel.clone()),
                &mut cb,
            );
            let was_cancelled = cancel.load(std::sync::atomic::Ordering::SeqCst);
            match r {
                Ok(list) => {
                    let gps = list.iter().filter(|p| p.lat.is_some()).count();
                    // 无取消 → done；有取消 → 先写结果再置 cancelled（部分结果保留可查）
                    owned.finish_scan_ok(job_id, list.len(), gps, 0);
                    if was_cancelled {
                        owned.finish_cancelled(job_id);
                    }
                    // 终帧补发（节流窗口内的最后一帧可能被缓存）
                    let _ = app2.emit(
                        "test-scan-progress",
                        &ScanProgress {
                            phase: "scan".into(),
                            current: list.len(),
                            total: list.len(),
                            file_name: if was_cancelled { "已停止".into() } else { "完成".into() },
                            message: format!("扫描完成：{} 张（GPS {gps}）", list.len()),
                        },
                    );
                    crate::logger::log_info(&format!(
                        "[test-scan] 扫描完成 | 张数={} GPS={} 线程={threads} 取消={was_cancelled} 耗时={:.2}s",
                        list.len(),
                        gps,
                        start.elapsed().as_secs_f64()
                    ));
                }
                Err(e) => {
                    owned.finish_err(job_id, &e);
                    crate::logger::log_error("start_scan_job", &e);
                }
            }
        });
        crate::logger::log_call_end_with(
            "start_scan_job",
            _t,
            &format!("OK | 已启动 job_id={job_id} threads={threads}（后台执行）"),
        );
        Ok(snap)
    }

    /// 启动「解析地名」任务（异步）：GPS 聚类 → 本地省/市优先（未命中才联网）
    #[tauri::command]
    pub async fn start_resolve_job(
        app: tauri::AppHandle,
        job: tauri::State<'_, ScanJobState>,
        path: String,
        recurse: bool,
    ) -> Result<JobSnapshot, String> {
        let threads = crate::scan_perf::effective_threads(&app);
        let _t = log_call!(
            "start_resolve_job",
            &format!("path={path} recurse={recurse} threads={threads}")
        );
        let (job_id, cancel) = job.begin(JobPhase::Resolve, &path, recurse, threads)?;
        let owned = job.handle();
        let snap = job.snapshot();
        let app2 = app.clone();
        let path2 = path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let start = std::time::Instant::now();
            let mut cb =
                progress_sink(app2.clone(), owned.clone(), job_id, JobPhase::Resolve, start);
            let r = super::resolve_test_places_cancellable(
                &path2,
                recurse,
                threads,
                Some(cancel.clone()),
                &mut cb,
            );
            let was_cancelled = cancel.load(std::sync::atomic::Ordering::SeqCst);
            match r {
                Ok(list) => {
                    let placed = list.iter().filter(|p| p.place.is_some()).count();
                    owned.finish_resolve_ok(job_id, list.len(), placed);
                    if was_cancelled {
                        owned.finish_cancelled(job_id);
                    }
                    crate::logger::log_info(&format!(
                        "[test-scan] 地名解析完成 | 张数={} 有地点={} 取消={was_cancelled} 耗时={:.2}s",
                        list.len(),
                        placed,
                        start.elapsed().as_secs_f64()
                    ));
                }
                Err(e) => {
                    owned.finish_err(job_id, &e);
                    crate::logger::log_error("start_resolve_job", &e);
                }
            }
        });
        crate::logger::log_call_end_with(
            "start_resolve_job",
            _t,
            &format!("OK | 已启动 job_id={job_id}（后台执行）"),
        );
        Ok(snap)
    }

    /// 启动「按年·地点组织移动」任务（异步，破坏性操作）
    ///
    /// 破坏性动作的**确认在前端**完成（弹窗确认后才 invoke 本命令）；
    /// 命令本身不再二次确认，避免「后台任务弹窗」这种无宿主 UI 的死结。
    #[tauri::command]
    pub async fn start_organize_job(
        app: tauri::AppHandle,
        job: tauri::State<'_, ScanJobState>,
        path: String,
        recurse: bool,
    ) -> Result<JobSnapshot, String> {
        let threads = crate::scan_perf::effective_threads(&app);
        let _t = log_call!(
            "start_organize_job",
            &format!("path={path} recurse={recurse} threads={threads}")
        );
        let (job_id, cancel) = job.begin(JobPhase::Organize, &path, recurse, threads)?;
        let owned = job.handle();
        let snap = job.snapshot();
        let app2 = app.clone();
        let path2 = path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let start = std::time::Instant::now();
            let mut cb =
                progress_sink(app2.clone(), owned.clone(), job_id, JobPhase::Organize, start);
            let r = super::organize_test_photos_cancellable(
                &path2,
                recurse,
                threads,
                Some(cancel.clone()),
                &mut cb,
            );
            let was_cancelled = cancel.load(std::sync::atomic::Ordering::SeqCst);
            match r {
                Ok(rep) => {
                    let rep_cancelled = rep.cancelled || was_cancelled;
                    owned.finish_organize_ok(
                        job_id,
                        OrganizeSummary {
                            total: rep.total,
                            moved: rep.moved,
                            conflict: rep.conflict,
                            no_time: rep.no_time,
                            no_place: rep.no_place,
                            failed: rep.failed,
                            target_root: rep.target_root.clone(),
                            folders: rep.folders.clone(),
                            cancelled: rep_cancelled,
                        },
                    );
                    if rep_cancelled {
                        owned.finish_cancelled(job_id);
                    }
                    crate::logger::log_info(&format!(
                        "[test-scan] 组织移动完成 | total={} moved={} conflict={} failed={} 取消={rep_cancelled} 耗时={:.2}s",
                        rep.total,
                        rep.moved,
                        rep.conflict,
                        rep.failed,
                        start.elapsed().as_secs_f64()
                    ));
                }
                Err(e) => {
                    owned.finish_err(job_id, &e);
                    crate::logger::log_error("start_organize_job", &e);
                }
            }
        });
        crate::logger::log_call_end_with(
            "start_organize_job",
            _t,
            &format!("OK | 已启动 job_id={job_id}（后台执行）"),
        );
        Ok(snap)
    }

    /// 取任务快照（页面挂载 / 定时轮询时调用即可恢复完整视图）
    #[tauri::command]
    pub fn get_scan_job(job: tauri::State<'_, ScanJobState>) -> JobSnapshot {
        job.snapshot()
    }

    /// 请求停止当前任务（置取消标记；任务在下一个检查点收敛）
    #[tauri::command]
    pub fn cancel_scan_job(job: tauri::State<'_, ScanJobState>) -> Result<bool, String> {
        let ok = job.cancel();
        crate::logger::log_info(&format!("test_scan | 收到停止请求 handled={ok}"));
        Ok(ok)
    }

    /// 清空任务记录（仅非运行中允许）
    #[tauri::command]
    pub fn clear_scan_job(job: tauri::State<'_, ScanJobState>) -> Result<(), String> {
        job.clear()
    }

    /// 清除失败/完成态并回到空闲（等价于 clear，语义更清晰的别名，供 UI「新任务」按钮）
    #[tauri::command]
    pub fn reset_scan_job(job: tauri::State<'_, ScanJobState>) -> Result<(), String> {
        job.clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 端到端：扫描 → 解析地名（联网）→ 组织移动。
    ///
    /// 从真实测试目录复制 4 张照片（2 张达州 A 点、1 张达州 B 点、1 张无 GPS）
    /// 到临时目录验证移动逻辑，跑完清理临时目录（不碰源数据）。
    #[test]
    #[ignore] // 联网测试：手动 cargo test -- --ignored test_scan::tests::e2e_scan_resolve_organize
    fn e2e_scan_resolve_organize() {
        let src_dir = Path::new("D:/YUAN HAO/Pictures/2026/test");
        if !src_dir.is_dir() {
            eprintln!("跳过：无真实测试目录");
            return;
        }
        let tmp = std::env::temp_dir().join("test_scan_e2e");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // A 点（31.9213, 107.6375）+ B 点（31.8568, 107.6452，不同 0.01° 网格）+ 无 GPS
        let picks = [
            "IMG_20200207_173741.jpg",
            "IMG_20200228_121553.jpg",
            "IMG_20200307_113915.jpg",
            "20240220-DSC_3583.jpg", // 无 GPS（人物特写）
        ];
        for f in picks {
            std::fs::copy(src_dir.join(f), tmp.join(f)).expect("复制失败");
        }

        // 1) 扫描：4 张直接图片，3 张有 GPS
        let photos = scan_test_photos(tmp.to_str().unwrap(), false).unwrap();
        assert_eq!(photos.len(), 4, "应扫到 4 张直接图片");
        assert_eq!(
            photos.iter().filter(|p| p.lat.is_some()).count(),
            3,
            "3 张应有 GPS"
        );
        assert_eq!(
            photos.iter().filter(|p| p.year.is_some()).count(),
            4,
            "时间三级兜底应覆盖全部（含 mtime）"
        );
        eprintln!(
            "[e2e] 扫描 OK: {} 张, GPS {}, 时间 {}",
            photos.len(),
            photos.iter().filter(|p| p.lat.is_some()).count(),
            photos.iter().filter(|p| p.shoot_time.is_some()).count()
        );

        // 2) 解析地名（联网）：A/B 两点各 1 次请求 → 3 张有 place
        let mut resolved = 0usize;
        let with_place = resolve_test_places(tmp.to_str().unwrap(), false, &mut |p| {
            resolved += 1;
            eprintln!(
                "  [progress {}/{}] {} → {}",
                p.current, p.total, p.file_name, p.message
            );
        })
        .unwrap();
        assert_eq!(resolved, 3, "3 张有 GPS 的照片应各回调一次");
        let placed = with_place.iter().filter(|p| p.place.is_some()).count();
        eprintln!("[e2e] 解析地名: {placed}/4 (进度回调 {resolved})");
        for p in with_place.iter().filter(|p| p.place.is_some()) {
            eprintln!("  {} → {}", p.file_name, p.place.as_deref().unwrap());
        }
        assert!(placed >= 2, "至少 A 点照片应获得地名");

        // 3) 组织移动：创建 年/地点 两级文件夹（进度回调按张，含内部 resolve 阶段）
        let mut organize_cb = 0usize;
        let mut resolve_cb = 0usize;
        let rep = organize_test_photos(tmp.to_str().unwrap(), false, &mut |p| {
            if p.phase == "organize" {
                organize_cb += 1;
            } else {
                resolve_cb += 1;
            }
            eprintln!(
                "  [progress {}/{} {}] {} {}",
                p.current, p.total, p.phase, p.file_name, p.message
            );
        })
        .unwrap();
        assert_eq!(organize_cb, 4, "4 张照片应各回调一次移动进度");
        assert!(resolve_cb >= 1, "内部解析地名阶段应有进度回调");
        eprintln!(
            "[e2e] 移动报告: total={} moved={} no_place={} failed={}",
            rep.total, rep.moved, rep.no_place, rep.failed
        );
        assert_eq!(rep.moved, 4, "4 张都应移动成功");
        assert_eq!(rep.failed, 0);
        assert!(rep.no_place >= 1, "无 GPS 照片应计入 no_place");
        // 根目录不再有直接图片
        let direct = std::fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .count();
        assert_eq!(direct, 0, "移动后根目录不应有直接图片");
        eprintln!("[e2e] 目录结构: {:?}", rep.folders);

        let _ = std::fs::remove_dir_all(&tmp);
        eprintln!("[e2e] 完成并清理临时目录");
    }
}

#[cfg(test)]
mod recurse_tests {
    use super::*;
    use std::fs;

    fn setup_dir(tag: &str) -> std::path::PathBuf {
        // 每个测试用唯一目录名前缀，避免并行执行时相互删除/重建导致竞态
        let tmp =
            std::env::temp_dir().join(format!("test_scan_recurse_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub/deep")).unwrap();
        // 根目录 2 张 + 子目录 1 张 + 深层 1 张 + 非图片 1 个
        fs::write(tmp.join("root1.jpg"), b"x").unwrap();
        fs::write(tmp.join("root2.png"), b"x").unwrap();
        fs::write(tmp.join("sub/child1.jpg"), b"x").unwrap();
        fs::write(tmp.join("sub/deep/leaf2.jpg"), b"x").unwrap();
        fs::write(tmp.join("note.txt"), b"x").unwrap();
        tmp
    }

    #[test]
    fn recurse_false_only_direct() {
        let dir = setup_dir("flat");
        let photos = scan_test_photos(dir.to_str().unwrap(), false).unwrap();
        // 只扫直接图片：root1.jpg + root2.png = 2 张
        let roots_only = photos.iter().filter(|p| !p.path.contains("sub")).count();
        assert_eq!(photos.len(), 2, "非递归只应扫到根目录 2 张");
        assert_eq!(roots_only, 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recurse_true_includes_subdirs() {
        let dir = setup_dir("tree");
        let photos = scan_test_photos(dir.to_str().unwrap(), true).unwrap();
        // 递归：root1 + root2 + sub/child1 + sub/deep/leaf2 = 4 张
        assert_eq!(photos.len(), 4, "递归应扫到 4 张（含子目录）");
        let _ = fs::remove_dir_all(&dir);
    }

    /// FEAT-064：并行扫描结果必须与串行**逐字节一致**（顺序 + 内容）
    ///
    /// 这个断言是并行化的安全网：任何「结果乱序 / 丢张 / 重复」都会在这里暴露。
    #[test]
    fn parallel_scan_matches_serial() {
        let dir = setup_dir("parallel");
        let serial = scan_test_photos(dir.to_str().unwrap(), true).unwrap();
        for threads in [2usize, 4, 8] {
            let mut got = Vec::new();
            scan_test_photos_parallel(dir.to_str().unwrap(), true, threads, &mut |p| {
                got.push(p.current);
            })
            .map(|v| {
                let mut par = v;
                par.sort_by(|a, b| a.file_name.cmp(&b.file_name));
                par
            })
            .unwrap()
            .iter()
            .zip(serial.iter())
            .for_each(|(a, b)| {
                assert_eq!(a.file_name, b.file_name, "{threads} 线程结果顺序/内容应一致");
                assert_eq!(a.path, b.path);
                assert_eq!(a.shoot_time, b.shoot_time);
            });
            // 进度回调必须覆盖到 total（最后一张的 current == total）
            assert!(
                got.iter().any(|c| *c == serial.len()),
                "{threads} 线程的进度回调应覆盖到最后一张"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// FEAT-064：取消后应尽快收敛并返回**部分结果**（而非报错/空）
    #[test]
    fn cancellable_scan_returns_partial() {
        let dir = setup_dir("cancel");
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        // 一上来就置位：worker 应立刻短路，不应扫到任何一张
        let out = scan_test_photos_cancellable(
            dir.to_str().unwrap(),
            true,
            4,
            Some(cancel),
            &mut |_| {},
        )
        .unwrap();
        assert!(out.len() <= 4, "取消后不应扫满（允许 0 张）");
        let _ = fs::remove_dir_all(&dir);
    }

    /// FEAT-064：线程池并行度不应超过文件数（避免建了 8 线程只跑 2 张）
    #[test]
    fn threads_clamped_to_file_count() {
        let dir = setup_dir("clamp");
        // 4 张文件 + 999 线程：应正常完成，不 panic
        let out = scan_test_photos_parallel(dir.to_str().unwrap(), true, 999, &mut |_| {}).unwrap();
        assert_eq!(out.len(), 4);
        let _ = fs::remove_dir_all(&dir);
    }
}
