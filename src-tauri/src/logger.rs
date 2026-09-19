//! 日志组件
//!
//! 提供 AOP（面向切面编程）风格的日志记录：
//! - `log_call_start` / `log_call_end`：函数调用前后绑定日志
//! - 支持字符串描述
//! - 文件持久化到 `app_data_dir/logs/`
//! - 时间戳记录（UTC+8，儒略日历精确换算）
//! - 级别过滤：DEBUG < INFO < WARN < ERROR，`PMS_LOG_LEVEL` 环境变量控制（默认 info；
//!   设为 debug 可恢复每条命令的 START 明细）
//! - 慢调用识别：命令耗时 ≥ `SLOW_CALL_MS` 自动升为 WARN 并打 SLOW 标记
//! - 轮转：app.log 超 5MB 自动改名归档（不再清空丢历史），保留期由定时清理兜底
//! - 运行监控：panic 落盘（`install_panic_hook`）、清理线程周期输出 MONITOR 心跳
//!   （运行时长 / 周期内 ERROR·WARN·SLOW 计数 / 当前文件大小）
//! - 定时清理（默认保留 3 天，可调节）
//!
//! 使用方式：
//! ```ignore
//! // crate 内函数，doctest（独立 crate）无法直接编译，标为 ignore 仅作展示
//! let t = log_call_start("move_album", "album_id=1, folder_id=Some(2)");
//! // ... 业务逻辑 ...
//! log_call_end("move_album", t);
//! ```
//!
//! 日志行格式：`[YYYY-MM-DD HH:MM:SS.mmm] [LEVEL] <原消息>`，
//! 原消息自带 `[AOP:CALL]` / `[AOP:RET]` / `[AOP:ERR]` / `[MONITOR]` 等来源标签。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 日志目录名（位于 app_data_dir 下）
pub const LOGS_DIR: &str = "logs";
/// 当前日志文件名
pub const LOG_FILE: &str = "app.log";

/// 慢调用阈值：命令耗时超过该值时 END 日志升为 WARN 并带 SLOW 标记
const SLOW_CALL_MS: u128 = 1000;
/// 单文件轮转阈值：app.log 超过该大小时改名归档为 app-<时间戳>.log
const ROTATE_BYTES: u64 = 5 * 1024 * 1024;
/// 每写入多少条日志做一次轮转大小检查（避免每条日志都 stat 文件）
const ROTATE_CHECK_EVERY: u64 = 128;
/// 日志时间戳时区偏移（秒）：固定 UTC+8，与目标部署时区一致
const TZ_OFFSET_SECS: i64 = 8 * 3600;

/// 日志级别（数值越小越详细）
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }

    fn num(self) -> u8 {
        self as u8
    }
}

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

/// 最低输出级别（init 时按 PMS_LOG_LEVEL 解析，默认 INFO）
static MIN_LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
/// 周期内统计：ERROR 条数（清理线程心跳输出后清零）
static STAT_ERROR: AtomicU64 = AtomicU64::new(0);
/// 周期内统计：WARN 条数
static STAT_WARN: AtomicU64 = AtomicU64::new(0);
/// 周期内统计：慢调用（SLOW）条数
static STAT_SLOW: AtomicU64 = AtomicU64::new(0);
/// 写入序号：用于按间隔触发轮转大小检查
static WRITE_SEQ: AtomicU64 = AtomicU64::new(0);
/// 进程启动时刻：MONITOR 心跳输出运行时长
static START_TIME: OnceLock<Instant> = OnceLock::new();

/// 解析 PMS_LOG_LEVEL 环境变量为最低输出级别（非法值/缺省回退 INFO）
fn level_from_env() -> Level {
    match std::env::var("PMS_LOG_LEVEL").as_deref() {
        Ok("debug") | Ok("DEBUG") => Level::Debug,
        Ok("warn") | Ok("WARN") => Level::Warn,
        Ok("error") | Ok("ERROR") => Level::Error,
        _ => Level::Info,
    }
}

/// 初始化日志系统
///
/// - `log_dir`: 日志目录（通常为 app_data_dir/logs）
/// - `retention_minutes`: 日志保留时长（分钟），默认 4320（3 天）
/// - 级别过滤由 `PMS_LOG_LEVEL` 控制（默认 info，debug 可见 AOP CALL 明细）
/// - 启动后台定时清理 + 监控心跳线程
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

    MIN_LEVEL.store(level_from_env().num(), Ordering::Relaxed);
    START_TIME.set(Instant::now()).ok();
    LOGGER.initialized.store(true, Ordering::SeqCst);
    write_log(
        Level::Info,
        &format!(
            "[LOGGER] 日志系统初始化完成，保留 {} 分钟，级别 {}",
            retention_minutes,
            match MIN_LEVEL.load(Ordering::Relaxed) {
                0 => "debug",
                1 => "info",
                2 => "warn",
                _ => "error",
            }
        ),
    );

    // 启动后台清理线程
    spawn_cleaner(retention_minutes);
}

/// 安装 panic 钩子：崩溃信息（线程 / 位置 / 消息）落盘后再走默认钩子。
///
/// 必须在 `logger::init` 之后调用，否则崩溃记录只打印到 stderr 不入文件。
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "未知位置".to_string());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_string();
        write_log(
            Level::Error,
            &format!("[PANIC] 线程 {thread} 于 {loc} 崩溃: {msg}"),
        );
        default_hook(info);
    }));
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

/// 清理过期日志 + 输出运行监控心跳
fn cleanup_logs() {
    let Some(dir) = get_dir() else { return };
    let retention = get_retention();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 过期归档文件：删除
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Ok(meta) = fs::metadata(&path) {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(dur) = modified.duration_since(UNIX_EPOCH) {
                            let modified_secs = dur.as_secs();
                            if now.saturating_sub(modified_secs) > retention.as_secs() {
                                let _ = fs::remove_file(&path);
                                eprintln!("[LOGGER] 删除过期日志: {}", path.display());
                            }
                        }
                    }
                }
            }
        }
    }

    // 运行监控心跳：运行时长 + 周期内错误/警告/慢调用计数 + 当前文件大小。
    // （超 5MB 的处理已改为写路径轮转归档，此处不再清空文件、不丢历史）
    let uptime_min = START_TIME.get().map(|t| t.elapsed().as_secs() / 60).unwrap_or(0);
    let errs = STAT_ERROR.swap(0, Ordering::Relaxed);
    let warns = STAT_WARN.swap(0, Ordering::Relaxed);
    let slows = STAT_SLOW.swap(0, Ordering::Relaxed);
    let file_kb = fs::metadata(dir.join(LOG_FILE))
        .map(|m| m.len() / 1024)
        .unwrap_or(0);
    write_log(
        Level::Info,
        &format!(
            "[MONITOR] 运行 {uptime_min} 分钟 | 周期内 ERROR={errs} WARN={warns} SLOW={slows} | app.log={file_kb} KB"
        ),
    );
}

/// 写入一条日志到文件（带时间戳与级别）
///
/// - 级别低于阈值（PMS_LOG_LEVEL，默认 INFO）时直接丢弃——AOP CALL 明细即靠此降噪
/// - ERROR / WARN / SLOW 计入周期统计，由清理线程的 MONITOR 心跳汇总输出
fn write_log(level: Level, message: &str) {
    if level.num() < MIN_LEVEL.load(Ordering::Relaxed) {
        return;
    }
    match level {
        Level::Error => {
            STAT_ERROR.fetch_add(1, Ordering::Relaxed);
        }
        Level::Warn => {
            STAT_WARN.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }

    let ts = format_timestamp();
    let line = format!("[{ts}] [{}] {message}\n", level.as_str());

    // 写入文件（若文件锁可用）
    let mut guard = LOGGER.file.lock().unwrap();
    write_through(&mut guard, &line);
    drop(guard);

    // 定期检查轮转（在锁外做，maybe_rotate 内部自行持锁换文件）
    if WRITE_SEQ.fetch_add(1, Ordering::Relaxed) % ROTATE_CHECK_EVERY == ROTATE_CHECK_EVERY - 1 {
        maybe_rotate();
    }

    // 同时打印到终端（stderr，方便开发调试）
    eprintln!("{}", line.trim_end());
}

/// 向已持有的文件句柄写入一行；句柄缺失时尝试按全局配置重新打开后重试
fn write_through(guard: &mut Option<fs::File>, line: &str) {
    if let Some(f) = guard.as_mut() {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
        return;
    }
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

/// 轮转检查：app.log 超过阈值时改名归档为 app-<时间戳>.log 并重新打开空文件。
///
/// 归档文件由清理线程按保留期统一删除；tail_log 的 reset 协议天然兼容
/// （新文件长度小于旧 offset → 前端收到 reset 后清屏重读）。
fn maybe_rotate() {
    let Some(dir) = get_dir() else { return };
    let current = dir.join(LOG_FILE);
    let Ok(meta) = fs::metadata(&current) else { return };
    if meta.len() < ROTATE_BYTES {
        return;
    }

    let rotated = dir.join(format!(
        "app-{}.log",
        format_timestamp_compact(SystemTime::now())
    ));
    {
        let mut guard = LOGGER.file.lock().unwrap();
        *guard = None; // 先关闭旧句柄，Windows 下 rename 才能成功
        if fs::rename(&current, &rotated).is_err() {
            // 改名失败（如被占用）：保持原文件继续写，下个检查点重试
            *guard = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&current)
                .ok();
            return;
        }
        *guard = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&current)
            .ok();
    }
    eprintln!("[LOGGER] app.log 超 5MB，已轮转归档: {}", rotated.display());
}

/// 当前时间戳，格式 YYYY-MM-DD HH:MM:SS.mmm（UTC+8）
fn format_timestamp() -> String {
    let (date, time) = timestamp_parts(SystemTime::now());
    format!("{date} {time}")
}

/// 轮转文件名用的时间戳，格式 YYYYMMDD-HHMMSS（UTC+8）
fn format_timestamp_compact(t: SystemTime) -> String {
    let (date, time) = timestamp_parts(t);
    let hms = time.split('.').next().unwrap_or("").replace(':', "");
    format!("{}-{}", date.replace('-', ""), hms)
}

/// 把 Unix 时刻换算为（UTC+8 的 YYYY-MM-DD, HH:MM:SS.mmm）
///
/// 日期部分采用 Howard Hinnant 的 civil_from_days 算法精确换算，替代旧版
/// `year=1970+days/365, month=(days%365)/31` 的近似算法（旧算法日期会漂移，
/// 如 2026-09 被记成 2026-07，直接影响按时间排查问题）。
fn timestamp_parts(t: SystemTime) -> (String, String) {
    let now = t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let total_secs = now.as_secs() as i64 + TZ_OFFSET_SECS;
    let millis = now.subsec_millis();

    let days = total_secs.div_euclid(86400);
    let day_secs = total_secs.rem_euclid(86400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;

    (
        format!("{year:04}-{month:02}-{day:02}"),
        format!("{hour:02}:{minute:02}:{second:02}.{millis:03}"),
    )
}

/// 天数（自 1970-01-01）→（年, 月, 日），Howard Hinnant civil_from_days 算法
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// AOP：记录函数调用开始，返回计时起点（DEBUG 级别：PMS_LOG_LEVEL=debug 可见）
///
/// `func_name`: 函数名
/// `desc`: 字符串描述（如参数信息）
pub fn log_call_start(func_name: &str, desc: &str) -> Instant {
    write_log(
        Level::Debug,
        &format!("[AOP:CALL] {func_name} START | {desc}"),
    );
    Instant::now()
}

/// AOP：记录函数调用结束（耗时 ≥1s 自动升为 WARN 并带 SLOW 标记）
///
/// `func_name`: 函数名
/// `start`: `log_call_start` 返回的计时起点
#[allow(dead_code)]
pub fn log_call_end(func_name: &str, start: Instant) {
    let elapsed_ms = start.elapsed().as_millis();
    if elapsed_ms >= SLOW_CALL_MS {
        STAT_SLOW.fetch_add(1, Ordering::Relaxed);
        write_log(
            Level::Warn,
            &format!("[AOP:RET] {func_name} END | SLOW | 耗时 {elapsed_ms}ms"),
        );
    } else {
        write_log(
            Level::Info,
            &format!("[AOP:RET]  {func_name} END   | 耗时 {elapsed_ms}ms"),
        );
    }
}

/// AOP：记录函数调用结束并带返回值摘要
///
/// 结果以 ERR / FAILED 开头 → ERROR 级别；耗时 ≥1s → WARN 并带 SLOW 标记。
pub fn log_call_end_with(func_name: &str, start: Instant, result_desc: &str) {
    let elapsed_ms = start.elapsed().as_millis();
    let is_err = result_desc.starts_with("ERR") || result_desc.starts_with("FAILED");
    if is_err {
        write_log(
            Level::Error,
            &format!("[AOP:RET]  {func_name} END   | 耗时 {elapsed_ms}ms | 结果 {result_desc}"),
        );
    } else if elapsed_ms >= SLOW_CALL_MS {
        STAT_SLOW.fetch_add(1, Ordering::Relaxed);
        write_log(
            Level::Warn,
            &format!("[AOP:RET] {func_name} END | SLOW | 耗时 {elapsed_ms}ms | 结果 {result_desc}"),
        );
    } else {
        write_log(
            Level::Info,
            &format!("[AOP:RET]  {func_name} END   | 耗时 {elapsed_ms}ms | 结果 {result_desc}"),
        );
    }
}

/// AOP：记录一个异常/错误
#[allow(dead_code)]
pub fn log_error(func_name: &str, err: &str) {
    write_log(Level::Error, &format!("[AOP:ERR] {func_name} ERROR | {err}"));
}

/// 记录一个警告（参数异常 / 可恢复失败 / 慢操作）——级别标签由 write_log 统一添加
pub fn log_warn(desc: &str) {
    write_log(Level::Warn, desc);
}

/// AOP：记录一个普通事件/信息——级别标签由 write_log 统一添加
pub fn log_info(desc: &str) {
    write_log(Level::Info, desc);
}

// ============================================================================
// 实时日志尾读（开发者视角窗口：前端 500ms 轮询本函数做增量读取）
// ============================================================================

/// `tail_log` 的返回：一段完整日志行 + 下次读取偏移
#[derive(Debug, serde::Serialize)]
pub struct TailResult {
    /// 本次新增的完整行（已按行切分，不含空行）
    pub lines: Vec<String>,
    /// 下次应传入的偏移（= 最后一个完整行末尾的字节位置）
    pub next_offset: u64,
    /// true = 传入的 offset 超过当前文件长度（文件被清理线程清空/轮转），前端应清屏
    pub reset: bool,
    /// true = 本次为尾部回填，文件更早内容未包含（仅首次调用可能为 true）
    pub truncated: bool,
    /// 当前日志文件字节数
    pub file_len: u64,
    /// 日志文件绝对路径（未初始化时为空串），供窗口状态栏展示
    pub path: String,
}

/// 日志文件绝对路径（未初始化时返回 None）
pub fn log_file_path() -> Option<PathBuf> {
    get_dir().map(|d| d.join(LOG_FILE))
}

/// 读取 app.log 自 `offset` 起的新增内容
///
/// - `offset` = 上次返回的 `next_offset`；首次传 0，此时只回填尾部 `max_bytes`
/// - 单次读取（含增量）都受 `max_bytes` 封顶，洪峰分帧追平
/// - 末尾不完整的行不消费（`next_offset` 不越过它），等下次写入完成后一并返回
/// - 清理线程会把 >5MB 的日志清空（`cleanup_logs`），此时 `offset > file_len`，
///   返回 `reset=true` 让前端清屏重读
pub fn tail_log(offset: u64, max_bytes: u64) -> TailResult {
    use std::io::{Read, Seek, SeekFrom};

    let path_str = log_file_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let result = |lines: Vec<String>, next_offset: u64, reset: bool, truncated: bool, file_len: u64| {
        TailResult { lines, next_offset, reset, truncated, file_len, path: path_str.clone() }
    };

    let Some(path) = log_file_path() else {
        return result(Vec::new(), 0, true, false, 0);
    };
    let Ok(meta) = fs::metadata(&path) else {
        return result(Vec::new(), 0, true, false, 0);
    };
    let file_len = meta.len();
    if file_len == 0 {
        return result(Vec::new(), 0, offset > 0, false, 0);
    }
    if offset > file_len {
        return result(Vec::new(), file_len, true, false, file_len);
    }

    let mut start = offset.min(file_len);
    let truncated = start == 0 && file_len > max_bytes;
    if truncated {
        start = file_len - max_bytes;
    }

    let Ok(mut f) = fs::File::open(&path) else {
        return result(Vec::new(), offset, false, false, file_len);
    };
    if f.seek(SeekFrom::Start(start)).is_err() {
        return result(Vec::new(), offset, false, false, file_len);
    }
    // 单次读取封顶 max_bytes：扫描等高峰期日志洪峰一次可新增数 MB，不封顶会撑大
    // 单次响应（序列化 + IPC 传输 + 前端渲染全被拖垮）；超出部分由后续轮询按
    // offset 逐步追平（BUG-2026-0910-001）。
    let mut buf = Vec::new();
    if f.take(max_bytes as u64).read_to_end(&mut buf).is_err() {
        return result(Vec::new(), offset, false, false, file_len);
    }

    // 只消费到最后一个 '\n'（含）；末尾半行留给下次写入完成后返回
    let consumed = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let mut lines: Vec<String> = Vec::new();
    if consumed > 0 {
        let text = String::from_utf8_lossy(&buf[..consumed]);
        let body = if truncated {
            // 起点落在半行中间：丢弃第一段不完整内容
            match text.find('\n') {
                Some(i) => &text[i + 1..],
                None => "",
            }
        } else {
            text.as_ref()
        };
        for l in body.lines() {
            if !l.trim().is_empty() {
                lines.push(l.to_string());
            }
        }
    }

    result(lines, start + consumed as u64, false, truncated, file_len)
}

// ============================================================================
// 单元测试：BUG-2026-0910-001 回归防护（tail 协议：回填/增量/半行/轮转/封顶）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 串行锁。
    ///
    /// 这些用例都会通过 `setup_dir` 改写**进程级全局**日志目录（`set_global_config`），
    /// 而 `tail_log()` 只认这个全局（`get_dir()`）。cargo 默认多线程并行跑测试，
    /// 于是 A 用例会读到 B 用例刚写进自己文件的内容。
    /// 历史故障：期望 `["[t1] a","[t2] b"]` 却拿到 `["[t] a"]`（另一个用例写的行）；
    /// `truncated` 期望 true 却 false（读到了别人的小文件）。加锁串行后全绿。
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        // 某个用例 assert 失败时会毒化锁：解包内部值继续，让其余用例还能正常报错
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn setup_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pms_logger_test_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(LOGS_DIR)).unwrap();
        set_global_config(dir.join(LOGS_DIR), Duration::from_secs(3600));
        dir.join(LOGS_DIR).join(LOG_FILE)
    }

    fn write(path: &Path, s: &str) {
        use std::io::Write;
        let mut f = OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
    }

    #[test]
    fn tail_small_file_full_then_incremental() {
        let _serial = serial();
        let path = setup_dir("small");
        write(&path, "[t1] a\n[t2] b\n");
        let r1 = tail_log(0, 64 * 1024);
        assert_eq!(r1.lines, vec!["[t1] a", "[t2] b"]);
        assert!(!r1.truncated && !r1.reset);
        // 半行不消费
        write(&path, "[t3] c");
        let r2 = tail_log(r1.next_offset, 64 * 1024);
        assert!(r2.lines.is_empty(), "半行应留待下次");
        write(&path, " 尾\n[t4] d\n");
        let r3 = tail_log(r2.next_offset, 64 * 1024);
        assert_eq!(r3.lines, vec!["[t3] c 尾", "[t4] d"]);
    }

    #[test]
    fn tail_backfill_only_tail_max_bytes() {
        let _serial = serial();
        let path = setup_dir("backfill");
        let big = "[t] x\n".repeat(40 * 1024); // 280KB > 64KB
        write(&path, &big);
        let r = tail_log(0, 64 * 1024);
        assert!(r.truncated);
        assert_eq!(r.lines.len(), 10922, "64KB 内的完整 6 字节行数（起点半行已丢弃）");
        assert!(r.next_offset > 0);
    }

    #[test]
    fn tail_reset_when_file_rotated() {
        let _serial = serial();
        let path = setup_dir("reset");
        write(&path, "[t] a\n");
        let r1 = tail_log(0, 64 * 1024);
        // 模拟清理线程清空
        fs::write(&path, "").unwrap();
        let r2 = tail_log(r1.next_offset, 64 * 1024);
        assert!(r2.reset);
        assert_eq!(r2.next_offset, 0);
        // 清空后重新写入，从头读
        write(&path, "[t] new\n");
        let r3 = tail_log(r2.next_offset, 64 * 1024);
        assert!(!r3.reset);
        assert_eq!(r3.lines, vec!["[t] new"]);
    }

    #[test]
    fn tail_incremental_capped_by_max_bytes() {
        let _serial = serial();
        let path = setup_dir("capped");
        write(&path, "[t] a\n");
        let r1 = tail_log(0, 64 * 1024);
        // 一次性写入 128KB，超过单次 max_bytes=64KB → 分帧追平
        let flood = "[f] line\n".repeat(128 * 1024 / 9);
        write(&path, &flood);
        let mut r = tail_log(r1.next_offset, 64 * 1024);
        let first_count = r.lines.len();
        assert!(first_count > 0 && first_count < flood.lines().count(), "单次必须封顶");
        // 逐步追平到 EOF
        let mut polls = 1;
        while r.lines.last().map(|l| !l.contains("line")).unwrap_or(true) || polls < 3 {
            r = tail_log(r.next_offset, 64 * 1024);
            polls += 1;
            if r.next_offset >= fs::metadata(&path).unwrap().len() && r.lines.is_empty() {
                break;
            }
            if polls > 20 { panic!("未能在有限轮内追平"); }
        }
    }

    /// civil_from_days 精确换算回归：旧近似算法把 2026-09 记成 2026-07，
    /// 此用例锚定已知日期，防止日期漂移回归
    #[test]
    fn civil_from_days_known_dates() {
        // 1970-01-01
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-02-29（闰年、世纪闰年，400 年历法边界）
        assert_eq!(civil_from_days(11016), (2000, 2, 29));
        // 2026-09-19（当前部署时段，旧算法会漂移成 2026-07-25）
        let days = 20_715; // 2026-09-19
        assert_eq!(civil_from_days(days), (2026, 9, 19));
        // 2024-12-31（平年年末）
        assert_eq!(civil_from_days(20_088), (2024, 12, 31));
    }

    /// timestamp_parts：UTC+8 偏移正确（epoch 正午 UTC = 当地 20:00 同日）
    #[test]
    fn timestamp_parts_applies_tz_offset() {
        // 1970-01-01 04:00:00 UTC → UTC+8 当日 12:00
        let t = UNIX_EPOCH + Duration::from_secs(4 * 3600);
        let (date, time) = timestamp_parts(t);
        assert_eq!(date, "1970-01-01");
        assert!(time.starts_with("12:00:00"), "got {time}");
    }

    /// 级别过滤 / SLOW 升级 / ERR 升级的行为回归：
    /// - 默认 INFO 级别下 AOP CALL（DEBUG）被过滤
    /// - 结果 ERR → ERROR 行
    /// - 耗时 ≥1s → WARN 且带 SLOW 标记
    #[test]
    fn level_filter_slow_and_err() {
        let _serial = serial();
        let path = setup_dir("levels");
        // 伪造一个 1.1s 前的计时起点（用 sleep 之外的确定性做法不可行，直接短睡）
        let t = log_call_start("demo_cmd", "album_id=1");
        std::thread::sleep(Duration::from_millis(1050));
        log_call_end_with("demo_cmd", t, "OK | count=3");
        log_call_end_with("demo_err", Instant::now(), "ERR | 数据库已锁定");
        log_warn("[demo] 可恢复失败");

        let text = fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(!lines.iter().any(|l| l.contains("[AOP:CALL]")), "默认级别应过滤 CALL 明细");
        assert!(lines.iter().any(|l| l.contains("[WARN]") && l.contains("SLOW") && l.contains("耗时")), "慢调用应升 WARN+SLOW: {lines:?}");
        assert!(lines.iter().any(|l| l.contains("[ERROR]") && l.contains("demo_err")), "ERR 结果应为 ERROR 级别");
        assert!(lines.iter().all(|l| !l.contains("[INFO] [INFO]")), "不应出现双级别前缀");
    }
}
