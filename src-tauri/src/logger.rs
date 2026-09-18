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
//! ```ignore
//! // crate 内函数，doctest（独立 crate）无法直接编译，标为 ignore 仅作展示
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
}
