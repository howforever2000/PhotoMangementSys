//! 开发者视角（原 lib.rs「开发者视角」两节整体迁出，行为零改动）
//! ===================================================================
//! 职责：诊断用命令与两个只读副窗口 ——
//! 1. `app_info`：运行态构建指纹（版本 / dev / exe 路径与构建时间）；
//! 2. 实时日志副窗口（label `dev-logs`）：tail_dev_log + 建窗；
//! 3. 「数据与路径」副窗口（label `dev-data`）：路径清单 / 表浏览 /
//!    只读 SQL 沙箱 / 资源管理器定位（全部只读，见 devdata.rs）。
//!
//! 迁移说明（lib.rs 瘦身第一期）：代码自 lib.rs 原样剪切，仅把可见性
//! 提升为 `pub`（供 lib.rs 的 generate_handler 继续按原名引用），
//! 逻辑、注释、日志口径均未改动。

use std::path::Path;

use tauri::Manager;

use crate::{devdata, logger};

/// 运行态构建信息（诊断）：版本 / 是否 dev / 可执行文件路径与构建时间
///
/// 由来：调试版走 vite（前端实时最新），release/打包版走 dist——而 dist 可能是
/// 陈旧构建（实测曾停留在 09-09，不含任何新功能）。把构建指纹显示在日志窗口
/// 状态栏，避免再出现『我跑的是哪个包』的误判。
#[derive(Debug, serde::Serialize)]
pub struct AppInfo {
    version: String,
    dev: bool,
    exe: String,
    exe_mtime_unix: Option<u64>,
}

#[tauri::command]
pub fn app_info(app: tauri::AppHandle) -> AppInfo {
    let exe_path = std::env::current_exe().ok();
    let exe = exe_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let exe_mtime_unix = exe_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    AppInfo {
        version: app.package_info().version.to_string(),
        dev: tauri::is_dev(),
        exe,
        exe_mtime_unix,
    }
}

/// 尾读 app.log 增量：返回上次偏移之后的新日志行（日志窗口 500ms 轮询一次）
///
/// 必须 async + spawn_blocking：Tauri 非 async 命令在主线程（事件循环）上执行，
/// 而所有窗口共用这一个事件循环——文件 IO/序列化放主线程会把全部界面一起卡住，
/// 日志高峰期（扫描）还会形成"越卡越读、越读越卡"的死亡螺旋。见 BUG-2026-0910-001。
#[tauri::command]
pub async fn tail_dev_log(offset: u64) -> Result<logger::TailResult, String> {
    let t0 = std::time::Instant::now();
    let r = tauri::async_runtime::spawn_blocking(move || logger::tail_log(offset, 64 * 1024))
        .await
        .map_err(|e| format!("tail_dev_log 执行失败: {e}"))?;
    // 慢查询诊断（BUG-2026-0910-001）：>200ms 且距上次告警 >5s 才写一行，
    // 避免告警本身在日志里形成新的反馈洪峰
    let ms = t0.elapsed().as_millis();
    if ms > 200 {
        static LAST_WARN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let prev = LAST_WARN.load(std::sync::atomic::Ordering::Relaxed);
        if now.saturating_sub(prev) >= 5
            && LAST_WARN
                .compare_exchange(
                    prev,
                    now,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
        {
            logger::log_info(&format!(
                "tail_dev_log 慢查询 {ms}ms（offset={offset} file_len={}）",
                r.file_len
            ));
        }
    }
    Ok(r)
}

/// 打开（或聚焦已存在的）开发者视角日志窗口（单例）
///
/// 新窗口 label 固定为 `dev-logs`，前端 main.ts 按 label 分支独立挂载日志组件，
/// 不走 router（避开登录守卫）。权限见 capabilities/dev-logs.json。
///
/// 必须 async + 后台线程建窗（BUG-2026-0910-005）：不用 async 时命令跑在主线程
/// （事件循环）内，`build()` 同步建窗会停在 about:blank（窗口空白、标题为空）、
/// 卡住整个事件循环，甚至导致进程以 0xcfffffff 退出——实测点击（IPC）必现，
/// 而后台线程建窗（启动自开诊断开关）完全正常。
#[tauri::command]
pub async fn open_dev_log_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("dev-logs") {
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || build_dev_log_window(app))
        .await
        .map_err(|e| format!("日志窗口创建任务失败: {e}"))?
}

/// 实际的建窗逻辑（在阻塞线程执行，不走主线程事件循环内同步建窗）
fn build_dev_log_window(app: tauri::AppHandle) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    // dev 走 vite 固定端口（vite.config.ts strictPort 1420）；打包走静态资源。
    // 窗口组件由 main.ts 按 window label（dev-logs）识别，无需 query 参数。
    let url = if tauri::is_dev() {
        tauri::WebviewUrl::External("http://localhost:1420/".parse().expect("valid dev url"))
    } else {
        tauri::WebviewUrl::App("index.html".into())
    };
    tauri::WebviewWindowBuilder::new(&app, "dev-logs", url)
        .title("开发者视角 · 实时日志")
        .inner_size(920.0, 620.0)
        .min_inner_size(560.0, 380.0)
        .resizable(true)
        .center()
        // 原生层固定纯黑（#0c0e14）：窗口创建瞬间/页面加载前/渲染滞后时不露白底
        .background_color(tauri::window::Color(12, 14, 20, 255))
        .build()
        .map_err(|e| format!("打开日志窗口失败: {e}"))?;
    logger::log_info(&format!(
        "打开日志窗口耗时 {}ms（后台线程建窗，不阻塞主线程）",
        t0.elapsed().as_millis()
    ));
    Ok(())
}

// =====================================================================
// 开发者视角（数据与路径副窗口）
//
// 由来：排障时反复要回答「数据到底写哪去了」——相册主库在 %APPDATA%、
// 人物库曾由编译期常量指向项目目录（BUG-2026-0916-005）、模型目录只在
// 只读时才转成 app_data 下的硬链接副本（BUG-2026-0916-004）。这些口径
// 应当能在界面上直接看到，而不是靠翻代码推断。
// 纪律：全部只读（只读连接 + 白名单 + 敏感列打码），见 devdata.rs。
// =====================================================================

/// 运行态路径清单（只读）：DB / 缓存 / 模型 / 日志的绝对路径与体量
///
/// 必须 async + spawn_blocking：目录占用要递归统计（thumbs 实测数万文件），
/// 放主线程事件循环会把全部界面一起卡住（BUG-2026-0910-001）。
#[tauri::command]
pub async fn dev_data_paths(app: tauri::AppHandle) -> Result<Vec<devdata::PathEntry>, String> {
    let _t = log_call!("dev_data_paths");
    let h = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || devdata::paths(&h))
        .await
        .map_err(|e| format!("dev_data_paths 执行失败: {e}"))?;
    match &r {
        Ok(v) => logger::log_call_end_with("dev_data_paths", _t, &format!("OK | {} 项", v.len())),
        Err(e) => logger::log_call_end_with("dev_data_paths", _t, &format!("ERR | {e}")),
    }
    r
}

/// 两个库的表清单与行数（只读）
#[tauri::command]
pub async fn dev_db_tables(app: tauri::AppHandle) -> Result<Vec<devdata::DbInfo>, String> {
    let _t = log_call!("dev_db_tables");
    let h = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || devdata::tables(&h))
        .await
        .map_err(|e| format!("dev_db_tables 执行失败: {e}"))?;
    match &r {
        Ok(v) => logger::log_call_end_with(
            "dev_db_tables",
            _t,
            &format!(
                "OK | {} 库 / {} 表",
                v.len(),
                v.iter().map(|d| d.tables.len()).sum::<usize>()
            ),
        ),
        Err(e) => logger::log_call_end_with("dev_db_tables", _t, &format!("ERR | {e}")),
    }
    r
}

/// 表数据预览（只读；敏感列打码；行数上限 200）
#[tauri::command]
pub async fn dev_db_rows(
    app: tauri::AppHandle,
    db: String,
    table: String,
    limit: i64,
) -> Result<devdata::RowsResult, String> {
    let _t = log_call!("dev_db_rows", &format!("db={db} table={table} limit={limit}"));
    let h = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || devdata::rows(&h, &db, &table, limit))
        .await
        .map_err(|e| format!("dev_db_rows 执行失败: {e}"))?;
    match &r {
        Ok(v) => logger::log_call_end_with(
            "dev_db_rows",
            _t,
            &format!("OK | {} 行 / 共 {}", v.rows.len(), v.total),
        ),
        Err(e) => logger::log_call_end_with("dev_db_rows", _t, &format!("ERR | {e}")),
    }
    r
}

/// 只读 SQL 沙箱（FEAT-060）：仅 SELECT/WITH/EXPLAIN，自动补 LIMIT，
/// 敏感列 + 用户身份/凭据值一律打码（无「显示明文」开关）。
#[tauri::command]
pub async fn dev_db_sql(
    app: tauri::AppHandle,
    db: String,
    sql: String,
    limit: i64,
) -> Result<devdata::SqlResult, String> {
    let _t = log_call!("dev_db_sql", &format!("db={db} limit={limit} sql={sql}"));
    let h = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || devdata::sql(&h, &db, &sql, limit))
        .await
        .map_err(|e| format!("dev_db_sql 执行失败: {e}"))?;
    match &r {
        Ok(v) => logger::log_call_end_with(
            "dev_db_sql",
            _t,
            &format!("OK | 返回 {} 行 / {}ms", v.returned, v.elapsed_ms),
        ),
        Err(e) => logger::log_call_end_with("dev_db_sql", _t, &format!("ERR | {e}")),
    }
    r
}

/// 在资源管理器中定位某个已知数据路径（key 白名单，不接受任意路径）
#[tauri::command]
pub async fn dev_reveal_path(app: tauri::AppHandle, key: String) -> Result<(), String> {
    let _t = log_call!("dev_reveal_path", &format!("key={key}"));
    let h = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        let entries = devdata::paths(&h)?;
        let e = entries
            .into_iter()
            .find(|e| e.key == key)
            .ok_or_else(|| format!("未知路径 key: {key}"))?;
        reveal_in_explorer(Path::new(&e.path), e.is_dir)
    })
    .await
    .map_err(|e| format!("dev_reveal_path 执行失败: {e}"))?;
    match &r {
        Ok(()) => logger::log_call_end_with("dev_reveal_path", _t, "OK"),
        Err(e) => logger::log_call_end_with("dev_reveal_path", _t, &format!("ERR | {e}")),
    }
    r
}

/// 在资源管理器中定位路径：文件→选中定位，目录→直接打开；
/// 路径不存在时退到最近存在的父目录（避免只报「找不到」）。
#[cfg(target_os = "windows")]
fn reveal_in_explorer(path: &Path, is_dir: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let mut target = path.to_path_buf();
    let mut as_dir = is_dir;
    while !target.exists() {
        match target.parent() {
            Some(p) if !p.as_os_str().is_empty() => {
                target = p.to_path_buf();
                as_dir = true;
            }
            _ => return Err(format!("路径不存在: {}", path.display())),
        }
    }
    let mut cmd = std::process::Command::new("explorer");
    // explorer 对 /select 的解析比较挑：用 raw_arg 原样传递，避免 Command 自动加引号
    if as_dir {
        cmd.raw_arg(format!("\"{}\"", target.display()));
    } else {
        cmd.raw_arg(format!("/select,\"{}\"", target.display()));
    }
    cmd.spawn()
        .map_err(|e| format!("打开资源管理器失败: {e}"))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn reveal_in_explorer(path: &Path, _is_dir: bool) -> Result<(), String> {
    Err(format!(
        "仅 Windows 支持在资源管理器中定位（当前：{}）",
        path.display()
    ))
}

/// 打开（或聚焦已存在的）「数据与路径」副窗口（单例）
///
/// 与日志窗口同构：label 固定 `dev-data`，前端 main.ts 按 label 分支挂载组件，
/// 不走 router（避开登录守卫）；权限见 capabilities/dev-data.json。
/// 同样 async + 后台线程建窗（BUG-2026-0910-005：主线程同步建窗会空白并卡死事件循环）。
///
/// 注：曾为「数据库查看」入口加过 view 参数（隐藏左栏、库浏览占满），
/// 后确认「数据与路径」本身已包含库浏览与只读 SQL，一个入口就够（FEAT-064 已废弃），
/// 故不再保留该分支，避免死代码。
#[tauri::command]
pub async fn open_dev_data_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("dev-data") {
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || build_dev_data_window(app))
        .await
        .map_err(|e| format!("数据窗口创建任务失败: {e}"))?
}

fn build_dev_data_window(app: tauri::AppHandle) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let url = if tauri::is_dev() {
        tauri::WebviewUrl::External("http://localhost:1420/".parse().expect("valid dev url"))
    } else {
        tauri::WebviewUrl::App("index.html".into())
    };
    tauri::WebviewWindowBuilder::new(&app, "dev-data", url)
        .title("开发者视角 · 数据与路径")
        .inner_size(1080.0, 720.0)
        .min_inner_size(720.0, 460.0)
        .resizable(true)
        .center()
        .background_color(tauri::window::Color(12, 14, 20, 255))
        .build()
        .map_err(|e| format!("打开数据窗口失败: {e}"))?;
    logger::log_info(&format!(
        "打开数据与路径窗口耗时 {}ms（后台线程建窗）",
        t0.elapsed().as_millis()
    ));
    Ok(())
}
