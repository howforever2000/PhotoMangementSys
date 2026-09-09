//! 日志组件
//!
//! 提供 AOP（面向切面编程）风格的日志记录：
//! - `log_call_start` / `log_call_end`：函数调用前后绑定日志
//! - 支持字符串描述
//! - 文件持久化到 `app_data_dir/logs/`
//! - 时间戳记录
//! - 定时清理（默认保留 1 小时，可调节），定时删除并刷新日志文件
//!
//! 使用方式：
//! ```rust
//! let t = log_call_start("move_album", "album_id=1, folder_id=Some(2)");
//! // ... 业务逻辑 ...
//! log_call_end("move_album", t);
//! ```

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 日志目录名（位于 app_data_dir 下）
pub const LOGS_DIR: &str = "logs";
/// 当前日志文件名
pub const LOG_FILE: &str = "app.log";

/// 全局日志状态
struct LoggerState {
    /// 文件写入锁
    file: Mutex<Option<fs::File>>,
    /// 是否已初始化
    initialized: AtomicBool,
}

static LOGGER: LoggerState = LoggerState {
    file: Mutex::new(None),
    initialized: AtomicBool::new(false),
};

/// 初始化日志系统
///
/// - `log_dir`: 日志目录（通常为 app_data_dir/logs）
/// - `retention_minutes`: 日志保留时长（分钟），默认 4320（3 天）
/// - 启动后台定时清理线程
pub fn init(log_dir: &Path, retention_minutes: u64) {
    let dir = log_dir.join(LOGS_DIR);
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("[LOGGER] 创建日志目录失败: {e}");
        return;
    }

    let retention = Duration::from_secs(retention_minutes.saturating_mul(60));
    {
        let mut file_guard = LOGGER.file.lock().unwrap();
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(LOG_FILE))
            .ok();
        *file_guard = f;
    }

    // 记录全局配置（目录 + 保留时长）
    set_global_config(dir, retention);

    LOGGER.initialized.store(true, Ordering::SeqCst);
    write_log(&format!("[LOGGER] 日志系统初始化完成，保留 {} 分钟", retention_minutes));

    // 启动后台清理线程
    spawn_cleaner(retention_minutes);
}

/// 全局配置（通过一个 Box 持有）
static GLOBAL_CONFIG: Mutex<Option<Box<GlobalConfig>>> = Mutex::new(None);

struct GlobalConfig {
    dir: PathBuf,
    retention: Duration,
}

fn set_global_config(dir: PathBuf, retention: Duration) {
    let mut g = GLOBAL_CONFIG.lock().unwrap();
    *g = Some(Box::new(GlobalConfig { dir, retention }));
}

fn get_dir() -> Option<PathBuf> {
    let g = GLOBAL_CONFIG.lock().unwrap();
    g.as_ref().map(|c| c.dir.clone())
}

fn get_retention() -> Duration {
    let g = GLOBAL_CONFIG.lock().unwrap();
    g.as_ref().map(|c| c.retention).unwrap_or(Duration::from_secs(3600))
}

/// 后台定时清理线程：定期清理过期的日志文件并刷新
fn spawn_cleaner(retention_minutes: u64) {
    std::thread::spawn(move || {
        loop {
            // 休眠一个清理周期（设为保留时长的 1/6，默认 10 分钟检查一次）
            let sleep_secs = (retention_minutes * 60 / 6).max(60);
            std::thread::sleep(Duration::from_secs(sleep_secs));
            cleanup_logs();
        }
    });
}

/// 清理过期日志：删除超过保留时长的日志文件，并重置当前文件
fn cleanup_logs() {
    let Some(dir) = get_dir() else { return };
    let retention = get_retention();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    write_log("[LOGGER] 开始定时清理过期日志...");
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Ok(meta) = fs::metadata(&path) {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(dur) = modified.duration_since(UNIX_EPOCH) {
                            let modified_secs = dur.as_secs();
                            if now.saturating_sub(modified_secs) > retention.as_secs() {
                                // 过期文件：删除
                                let _ = fs::remove_file(&path);
                                eprintln!("[LOGGER] 删除过期日志: {}", path.display());
                            }
                        }
                    }
                }
            }
        }
    }

    // 若当前日志文件过大或过旧，重置（清空）当前文件
    let current = dir.join(LOG_FILE);
    if current.exists() {
        if let Ok(meta) = fs::metadata(&current) {
            // 超过 5MB 或超过保留期则清空刷新
            if meta.len() > 5 * 1024 * 1024 {
                let _ = fs::write(&current, "");
                write_log("[LOGGER] 当前日志文件超过 5MB，已清空刷新");
            }
        }
    }
}

/// 写入一条日志到文件（带时间戳）
fn write_log(message: &str) {
    let ts = format_timestamp();
    let line = format!("[{ts}] {message}\n");

    // 写入文件（若文件锁可用）
    let mut guard = LOGGER.file.lock().unwrap();
    if let Some(f) = guard.as_mut() {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    } else {
        // 文件不可用时尝试重新打开
        if let Some(dir) = get_dir() {
            if let Ok(f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(LOG_FILE))
            {
                *guard = Some(f);
                if let Some(f) = guard.as_mut() {
                    let _ = f.write_all(line.as_bytes());
                    let _ = f.flush();
                }
            }
        }
    }

    // 同时打印到终端（stderr，方便开发调试）
    eprintln!("{}", line.trim_end());
}

/// 当前时间戳，格式 YYYY-MM-DD HH:MM:SS.mmm
fn format_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let secs = now.as_secs();
    let millis = now.subsec_millis();

    // 简单换算 UTC 时间（不处理时区，日志足够）
    let days = secs / 86400;
    let day_secs = secs % 86400;
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;
    // 从 1970 算年月日（简化，够用）
    let year = 1970 + days / 365;
    let month = 1 + (days % 365) / 31;
    let day = 1 + (days % 365) % 31;

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{millis:03}")
}

/// AOP：记录函数调用开始，返回计时起点
///
/// `func_name`: 函数名
/// `desc`: 字符串描述（如参数信息）
pub fn log_call_start(func_name: &str, desc: &str) -> Instant {
    write_log(&format!("[AOP:CALL] {func_name} START | {desc}"));
    Instant::now()
}

/// AOP：记录函数调用结束
///
/// `func_name`: 函数名
/// `start`: `log_call_start` 返回的计时起点
#[allow(dead_code)]
pub fn log_call_end(func_name: &str, start: Instant) {
    let elapsed_ms = start.elapsed().as_millis();
    write_log(&format!("[AOP:RET]  {func_name} END   | 耗时 {elapsed_ms}ms"));
}

/// AOP：记录函数调用结束并带返回值摘要
pub fn log_call_end_with(func_name: &str, start: Instant, result_desc: &str) {
    let elapsed_ms = start.elapsed().as_millis();
    write_log(&format!("[AOP:RET]  {func_name} END   | 耗时 {elapsed_ms}ms | 结果 {result_desc}"));
}

/// AOP：记录一个异常/错误
#[allow(dead_code)]
pub fn log_error(func_name: &str, err: &str) {
    write_log(&format!("[AOP:ERR] {func_name} ERROR | {err}"));
}

/// AOP：记录一个普通事件/信息
pub fn log_info(desc: &str) {
    write_log(&format!("[INFO] {desc}"));
}
