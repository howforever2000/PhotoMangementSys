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

/// 扫描目录下图片，提取时间 + GPS 坐标
///
/// - `recurse=false`：只扫所选目录下的**直接**图片（不进入子目录）
/// - `recurse=true`：递归遍历所有子目录（跳过隐藏目录，类似 walkdir 语义）
///
/// place 初始为 None（解析地名需联网，见 resolve_test_places）。
pub fn scan_test_photos(dir: &str, recurse: bool) -> Result<Vec<TestPhoto>, String> {
    let root = Path::new(dir);
    if !root.is_dir() {
        return Err(format!("路径不存在或不是文件夹: {dir}"));
    }
    let mut photos = Vec::new();
    if !recurse {
        // 只扫直接图片
        let rd = std::fs::read_dir(root).map_err(|e| format!("读取目录失败: {e}"))?;
        for entry in rd.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !photo_scan::is_image_file(&name) {
                continue;
            }
            let ex = photo_scan::read_photo_exif(&p, &name);
            let year = ex.shoot_time.as_deref().and_then(|t| t.get(0..4)).map(str::to_string);
            photos.push(TestPhoto {
                file_name: name,
                path: p.to_string_lossy().into_owned(),
                shoot_time: ex.shoot_time,
                year,
                lat: ex.lat,
                lon: ex.lon,
                place: None,
            });
        }
    } else {
        // 递归遍历子目录（跳过隐藏目录/文件）
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
        {
            let Ok(e) = entry else { continue };
            if !e.file_type().is_file() {
                continue;
            }
            let p = e.into_path();
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !photo_scan::is_image_file(&name) {
                continue;
            }
            let ex = photo_scan::read_photo_exif(&p, &name);
            let year = ex.shoot_time.as_deref().and_then(|t| t.get(0..4)).map(str::to_string);
            photos.push(TestPhoto {
                file_name: name,
                path: p.to_string_lossy().into_owned(),
                shoot_time: ex.shoot_time,
                year,
                lat: ex.lat,
                lon: ex.lon,
                place: None,
            });
        }
    }
    photos.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(photos)
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
    on_progress: &mut dyn FnMut(ScanProgress),
) -> Result<Vec<TestPhoto>, String> {
    let mut photos = scan_test_photos(dir, recurse)?;
    // 聚类：网格 key → 中心坐标
    let mut grid: std::collections::BTreeMap<(i32, i32), (f64, f64)> = std::collections::BTreeMap::new();
    for p in &photos {
        if let (Some(lat), Some(lon)) = (p.lat, p.lon) {
            grid.entry(grid_key(lat, lon)).or_insert((lat, lon));
        }
    }
    let total = photos.iter().filter(|p| p.lat.is_some()).count();
    let mut done = 0usize;
    // 本地未命中时兜底联网（懒创建）
    let mut client: Option<reqwest::blocking::Client> = None;
    for (key, (lat, lon)) in &grid {
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
                let msg = place.clone().unwrap_or_else(|| "无地名(反编码失败)".to_string());
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
    keep.iter().rev().map(|s| s.trim().to_string()).collect::<Vec<_>>().join(" · ")
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
    on_progress: &mut dyn FnMut(ScanProgress),
) -> Result<OrganizeReport, String> {
    let photos = resolve_test_places(dir, recurse, on_progress)?;
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
    };
    let mut seen_folders: std::collections::HashSet<String> = std::collections::HashSet::new();
    let total = photos.len();
    let mut done = 0usize;
    for p in &photos {
        done += 1;
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
            crate::logger::log_error("organize_test_photos", &format!("{} 创建目录失败 {}: {e}", p.file_name, folder.display()));
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
            crate::logger::log_info(&format!("[organize] {} 跳过(目标已存在: {})", p.file_name, dest.display()));
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
                crate::logger::log_info(&format!("[organize] {} → {}/{}/", p.file_name, year, place));
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
                crate::logger::log_error("organize_test_photos", &format!("{} 移动失败 {} → {}: {e}", p.file_name, src.display(), dest.display()));
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
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect()
}

// ---------------------------------------------------------------------------
// 命令层（薄壳，逻辑见上；lib.rs 仅注册）
//
// Tauri 2 同步命令在主线程执行——含网络/文件 IO 的命令必须 async +
// spawn_blocking，否则真实相册（数十聚类点 × 网络超时 + 移动）会冻结 UI。
// ---------------------------------------------------------------------------
pub mod commands {
    use super::*;

    /// 扫描目录下图片（recurse=false 只扫直接图片；recurse=true 递归子目录）：时间 + GPS 坐标
    #[tauri::command]
    pub async fn scan_test_photos(path: String, recurse: bool) -> Result<Vec<TestPhoto>, String> {
        let _t = log_call!("scan_test_photos", &format!("path={path} recurse={recurse}"));
        let r: Result<Vec<TestPhoto>, String> = tauri::async_runtime::spawn_blocking(move || {
            super::scan_test_photos(&path, recurse)
        })
        .await
        .map_err(|e| format!("任务线程失败: {e}"))?;
        match &r {
            Ok(list) => crate::logger::log_call_end_with(
                "scan_test_photos",
                _t,
                &format!("OK | photos={} gps={}", list.len(), list.iter().filter(|p| p.lat.is_some()).count()),
            ),
            Err(e) => crate::logger::log_call_end_with("scan_test_photos", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 解析地点：GPS 聚类 + 每聚类一次反向地理编码（联网），每张照片上报进度
    #[tauri::command]
    pub async fn resolve_test_places(app: tauri::AppHandle, path: String, recurse: bool) -> Result<Vec<TestPhoto>, String> {
        let _t = log_call!("resolve_test_places", &format!("path={path} recurse={recurse}"));
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
                &format!("OK | photos={} place={}", list.len(), list.iter().filter(|p| p.place.is_some()).count()),
            ),
            Err(e) => crate::logger::log_call_end_with("resolve_test_places", _t, &format!("ERR | {e}")),
        }
        r
    }

    /// 按「年 → 地点」两级文件夹组织移动（测试功能），每张照片上报进度
    #[tauri::command]
    pub async fn organize_test_photos(app: tauri::AppHandle, path: String, recurse: bool) -> Result<OrganizeReport, String> {
        let _t = log_call!("organize_test_photos", &format!("path={path} recurse={recurse}"));
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
                &format!("OK | total={} moved={} conflict={} failed={}", rep.total, rep.moved, rep.conflict, rep.failed),
            ),
            Err(e) => crate::logger::log_call_end_with("organize_test_photos", _t, &format!("ERR | {e}")),
        }
        r
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
        assert_eq!(photos.iter().filter(|p| p.lat.is_some()).count(), 3, "3 张应有 GPS");
        assert_eq!(photos.iter().filter(|p| p.year.is_some()).count(), 4, "时间三级兜底应覆盖全部（含 mtime）");
        eprintln!("[e2e] 扫描 OK: {} 张, GPS {}, 时间 {}", photos.len(),
            photos.iter().filter(|p| p.lat.is_some()).count(),
            photos.iter().filter(|p| p.shoot_time.is_some()).count());

        // 2) 解析地名（联网）：A/B 两点各 1 次请求 → 3 张有 place
        let mut resolved = 0usize;
        let with_place = resolve_test_places(tmp.to_str().unwrap(), false, &mut |p| {
            resolved += 1;
            eprintln!("  [progress {}/{}] {} → {}", p.current, p.total, p.file_name, p.message);
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
            eprintln!("  [progress {}/{} {}] {} {}", p.current, p.total, p.phase, p.file_name, p.message);
        })
        .unwrap();
        assert_eq!(organize_cb, 4, "4 张照片应各回调一次移动进度");
        assert!(resolve_cb >= 1, "内部解析地名阶段应有进度回调");
        eprintln!("[e2e] 移动报告: total={} moved={} no_place={} failed={}",
            rep.total, rep.moved, rep.no_place, rep.failed);
        assert_eq!(rep.moved, 4, "4 张都应移动成功");
        assert_eq!(rep.failed, 0);
        assert!(rep.no_place >= 1, "无 GPS 照片应计入 no_place");
        // 根目录不再有直接图片
        let direct = std::fs::read_dir(&tmp).unwrap()
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
        let tmp = std::env::temp_dir().join(format!("test_scan_recurse_{tag}_{}", std::process::id()));
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
}
