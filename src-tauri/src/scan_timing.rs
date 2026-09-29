//! 批量扫描耗时分段记账（细粒度耗时观测）
//
//! 目的：把「这次扫描到底慢在哪」从一句「共 N 张 / 用了 M 秒」拆到**板块级**，
//! 让每个板块（EXIF / 影调 / AI / 语义 / 落库 / 预热 / 分类重建）各自有独立读数，
//! 并额外给出批次级耗时（每批多少 ms、均摊多少 ms/张、批与批之间是否抖动）。
//!
//! 设计纪律（与项目既有约定一致，改动前请先读）：
//!   1. **纯记账**：不参与任何判断分支、不改变任何返回值；出问题最多是少一行日志。
//!   2. **逻辑下沉纯函数**：格式化 / 汇总都是纯函数（入参注入、不碰 IO、不读时钟），
//!      便于 `cargo test --lib scan_timing` 覆盖；只有 `ScanTiming::span` 这类
//!      「读时钟」的动作留在 impl 里。
//!   3. **不引入锁**：记账器是单次扫描的局部值（不是全局 static），跨线程不共享，
//!      因此不需要 Mutex —— 也就不存在「等锁影响被测对象」的经典陷阱。
//!   4. 日志前缀固定：`[scan.timing]`（相册级一条汇总）/ `[scan.batch]`（批次级，逐批一条）。
//!
//! 为什么「未计入」这一项必须有：
//!   各板块耗时相加通常**小于**总墙钟（await 调度、事件 emit、serde 序列化、
//!   spawn_blocking 的排队都要时间）。没有这一项，读者会误以为「数字对不上是记账错了」，
//!   从而不敢用这份数据下结论。显式给出残差，是对读者的诚实。
//!
//! ⚠ **并发段怎么记（BUG-2026-0928-003）**：
//!   三腿并发（影调 ∥ 语义 ∥ AI）后，`span_*` 都是 `tokio::join!` 返回之后才调用的。
//!   - 用 `span_with(name, 起点)` → 记到的是「本段起点 → join 返回」= **最慢那条腿的
//!     时长**，三条段会读出三个几乎相同的数（实测 195529 / 195529 / 195489 ms），
//!     完全无法归因，还会把 `阶段重叠` 算得虚大；
//!   - 正确做法：把「起点 + 终点」都在 future 内部读出来带出去，用 `span_ms` 记差值。
//!   顺序段（walk / EXIF / 落库 / 预热 / 重建）不受影响，继续用 `span_with` 即可。

use std::time::Instant;

/// 单个阶段（板块）的耗时
#[derive(Debug, Clone, PartialEq)]
pub struct PhaseSpan {
    /// 阶段名（写死在调用点，便于 grep）
    pub name: String,
    /// 耗时（毫秒）
    pub ms: f64,
    /// 补充口径（如线程数、张数），可为空
    pub note: String,
}

/// 相册级阶段耗时收集器（局部值，不跨线程共享 → 无需加锁）
pub struct ScanTiming {
    album_id: i64,
    started: Instant,
    spans: Vec<PhaseSpan>,
}

impl ScanTiming {
    pub fn new(album_id: i64) -> Self {
        Self { album_id, started: Instant::now(), spans: Vec::new() }
    }

    /// 记一段：从 `since` 到现在。返回本段耗时（ms），便于调用方顺手用于自己的日志。
    #[allow(dead_code)] // API 完整性保留：无口径说明的短场景用这个，当前调用点都带 note
    pub fn span(&mut self, name: &str, since: Instant) -> f64 {
        let ms = since.elapsed().as_secs_f64() * 1000.0;
        self.spans.push(PhaseSpan { name: name.to_string(), ms, note: String::new() });
        ms
    }

    /// 记一段：从 `since` 到现在，并附带口径说明（如 "8线程" / "120张"）
    pub fn span_with(&mut self, name: &str, since: Instant, note: &str) -> f64 {
        let ms = since.elapsed().as_secs_f64() * 1000.0;
        self.spans.push(PhaseSpan { name: name.to_string(), ms, note: note.to_string() });
        ms
    }

    /// 直接记一段已知耗时（调用方已经算好的，如并发段「future 自己的起止差」、
    /// 或 Python 侧回报的服务端内部耗时）
    pub fn span_ms(&mut self, name: &str, ms: f64, note: &str) {
        self.spans.push(PhaseSpan { name: name.to_string(), ms, note: note.to_string() });
    }

    /// 从 startTime 到现在的总墙钟（ms）
    pub fn wall_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    /// 打一行汇总日志（前缀 `[scan.timing]`）
    pub fn emit(&self, mode: &str) {
        crate::logger::log_info(&format!(
            "[scan.timing] album={} {} | {}",
            self.album_id,
            mode,
            render_spans(&self.spans, self.wall_ms())
        ));
    }

    /// 取出全部阶段（供调用方自行处理 / 测试）
    #[allow(dead_code)]
    pub fn spans(&self) -> &[PhaseSpan] {
        &self.spans
    }
}

/// 毫秒格式化：整数位足够时用 0 位小数，小数值保留 1 位（纯函数）
pub(crate) fn fmt_ms(ms: f64) -> String {
    if !ms.is_finite() {
        return "-".to_string();
    }
    if ms >= 100.0 {
        format!("{ms:.0}")
    } else if ms >= 10.0 {
        format!("{ms:.1}")
    } else {
        format!("{ms:.2}")
    }
}

/// 渲染一行：各阶段 `名 耗时(口径)`，末尾给出总墙钟与未计入残差（纯函数）
///
/// 残差 = 墙钟 - 各阶段之和，负数（时钟精度/阶段重叠）时夹到 0，
/// 绝不输出负数让读者困惑。
pub fn render_spans(spans: &[PhaseSpan], wall_ms: f64) -> String {
    if spans.is_empty() {
        return format!("总 {wall}ms | 无阶段记录", wall = fmt_ms(wall_ms));
    }
    let mut parts: Vec<String> = Vec::with_capacity(spans.len());
    let mut sum = 0.0f64;
    for s in spans {
        sum += s.ms;
        if s.note.is_empty() {
            parts.push(format!("{} {}", s.name, fmt_ms(s.ms)));
        } else {
            parts.push(format!("{} {}({})", s.name, fmt_ms(s.ms), s.note));
        }
    }
    let rest = (wall_ms - sum).max(0.0);
    // 各阶段之和 > 墙钟 ⇒ 阶段之间存在**并发重叠**（现状只有「点 1 三腿并发」会
    // 走到这里：影调 ∥ 语义向量 ∥ AI 识别）。必须显式给出重叠量——否则读者
    // 看到「未计入 0」会以为记账漏了，而真相是三条腿同时在跑。
    let overlap = (sum - wall_ms).max(0.0);
    let tail = if overlap > 0.5 {
        format!("阶段重叠(并发) ~{}", fmt_ms(overlap))
    } else {
        format!("未计入(调度/序列化/等待) {}", fmt_ms(rest))
    };
    format!(
        "总 {}ms | {} | {}",
        fmt_ms(wall_ms),
        parts.join(" · "),
        tail
    )
}

// ---------------------------------------------------------------------------
// 批次级计时
// ---------------------------------------------------------------------------

/// 单个批次的耗时记录
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchSpan {
    /// 批号（从 1 开始）
    pub index: usize,
    /// 本批张数
    pub n: usize,
    /// 本批耗时（ms）
    pub ms: f64,
}

/// 批次耗时汇总（纯函数 `summarize` 的产出）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchSummary {
    pub batches: usize,
    pub items: usize,
    pub total_ms: f64,
    /// 均摊到每张（ms/张）
    pub per_item: f64,
    pub min_batch_ms: f64,
    pub max_batch_ms: f64,
}

/// 批次耗时收集器（同样是无锁局部值）
pub struct BatchTiming {
    label: &'static str,
    spans: Vec<BatchSpan>,
}

impl BatchTiming {
    pub fn new(label: &'static str) -> Self {
        Self { label, spans: Vec::new() }
    }

    /// 记一批：自动编号（从 1 开始），返回本批批号
    pub fn push(&mut self, n: usize, ms: f64) -> usize {
        let index = self.spans.len() + 1;
        self.spans.push(BatchSpan { index, n, ms });
        index
    }

    /// 逐批日志是否全量输出：批数过多时只打首末 + 每 `stride` 批，避免刷屏
    /// （细腻度与可读性的折中：批数 ≤ 30 全打，便于看批间抖动）
    pub fn should_log(&self, index: usize) -> bool {
        const FULL_LOG_MAX: usize = 30;
        let total = self.spans.len();
        if total <= FULL_LOG_MAX {
            return true;
        }
        let stride = total.div_ceil(FULL_LOG_MAX).max(1);
        index == 1 || index == total || index % stride == 0
    }

    /// 最近一批的耗时（供逐批日志复用，避免调用方自己再传一次 ms）
    pub fn spans_last_ms(&self) -> Option<f64> {
        self.spans.last().map(|s| s.ms)
    }

    pub fn label(&self) -> &'static str {
        self.label
    }

    pub fn summarize(&self) -> BatchSummary {
        summarize(&self.spans)
    }

    /// 批次耗时合计（预留给调用方做自定义汇总；库内主用 `summarize()`）
    #[allow(dead_code)]
    pub fn total_ms(&self) -> f64 {
        self.spans.iter().map(|s| s.ms).sum()
    }
}

/// 批次汇总（纯函数：不读时钟、不碰 IO，可单测）
pub fn summarize(spans: &[BatchSpan]) -> BatchSummary {
    if spans.is_empty() {
        return BatchSummary { batches: 0, items: 0, total_ms: 0.0, per_item: 0.0,
                              min_batch_ms: 0.0, max_batch_ms: 0.0 };
    }
    let mut total = 0.0f64;
    let mut items = 0usize;
    let mut min_b = f64::MAX;
    let mut max_b = 0.0f64;
    for s in spans {
        total += s.ms;
        items += s.n;
        min_b = min_b.min(s.ms);
        max_b = max_b.max(s.ms);
    }
    BatchSummary {
        batches: spans.len(),
        items,
        total_ms: total,
        per_item: if items > 0 { total / items as f64 } else { 0.0 },
        min_batch_ms: if min_b == f64::MAX { 0.0 } else { min_b },
        max_batch_ms: max_b,
    }
}

/// 渲染单行批次日志（纯函数）
pub fn render_batch_line(label: &str, index: usize, total_batches: usize, n: usize, ms: f64) -> String {
    let per = if n > 0 { ms / n as f64 } else { 0.0 };
    format!(
        "[scan.batch] {} 批 {}/{} · {}张 · {}ms · 均摊 {}/张",
        label,
        index,
        total_batches,
        n,
        fmt_ms(ms),
        fmt_ms(per)
    )
}

/// 渲染批次汇总行（纯函数）
pub fn render_batch_summary(label: &str, s: &BatchSummary) -> String {
    format!(
        "[scan.batch] {} 汇总 · {}批 {}张 · 合计 {}ms · 均摊 {}/张 · 批最快 {} · 批最慢 {}",
        label,
        s.batches,
        s.items,
        fmt_ms(s.total_ms),
        fmt_ms(s.per_item),
        fmt_ms(s.min_batch_ms),
        fmt_ms(s.max_batch_ms)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_ms_keeps_precision_only_when_small() {
        assert_eq!(fmt_ms(1234.5), "1234");       // ≥100 → 0 位小数
        assert_eq!(fmt_ms(56.78), "56.8");        // ≥10  → 1 位
        assert_eq!(fmt_ms(3.14159), "3.14");      // <10  → 2 位
        assert_eq!(fmt_ms(f64::NAN), "-");        // 非有限值不 panic
        assert_eq!(fmt_ms(0.0), "0.00");
    }

    #[test]
    fn render_spans_shows_note_and_residual() {
        let spans = vec![
            PhaseSpan { name: "EXIF".into(), ms: 210.0, note: "8线程".into() },
            PhaseSpan { name: "影调".into(), ms: 356.0, note: String::new() },
        ];
        let line = render_spans(&spans, 600.0);
        assert!(line.contains("总 600ms"), "{line}");
        assert!(line.contains("EXIF 210(8线程)"), "{line}");
        assert!(line.contains("影调 356"), "{line}");
        // 残差 = 600 - (210+356) = 34
        assert!(line.contains("未计入(调度/序列化/等待) 34"), "{line}");
    }

    /// 并发段专用记法：span_ms 原样记下传入的段耗时（BUG-2026-0928-003 的正确路径）
    #[test]
    fn span_ms_records_given_duration_verbatim() {
        let mut t = ScanTiming::new(1);
        t.span_ms("语义向量", 195529.0, "批次8（与AI腿并发）");
        t.span_ms("AI识别", 41581.0, "批次8·412张（与语义腿并发）");
        let spans = t.spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].name, "语义向量");
        assert_eq!(spans[0].ms, 195529.0, "必须原样记下传入的段耗时");
        assert_eq!(spans[1].ms, 41581.0);
    }

    #[test]
    fn render_spans_reports_overlap_when_phases_run_concurrently() {
        // 各阶段之和大于墙钟 = 阶段并发重叠（点 1 三腿并发），必须显式标出，
        // 而不是输出「未计入 0」让读者以为记账漏了。
        let spans = vec![PhaseSpan { name: "AI".into(), ms: 500.0, note: String::new() }];
        let line = render_spans(&spans, 480.0);
        assert!(line.contains("阶段重叠(并发) ~20"), "{line}");
        assert!(!line.contains("未计入"), "{line}");
    }

    #[test]
    fn render_spans_residual_never_negative() {
        // 残差为负但落在亚毫秒内（时钟精度）：仍按「未计入 0」输出，不误报重叠
        let spans = vec![PhaseSpan { name: "AI".into(), ms: 480.2, note: String::new() }];
        let line = render_spans(&spans, 480.0);
        assert!(line.contains("未计入(调度/序列化/等待) 0"), "{line}");
    }

    #[test]
    fn render_spans_empty() {
        assert!(render_spans(&[], 100.0).contains("无阶段记录"));
    }

    #[test]
    fn summarize_batches() {
        let spans = vec![
            BatchSpan { index: 1, n: 8, ms: 640.0 },
            BatchSpan { index: 2, n: 8, ms: 800.0 },
        ];
        let s = summarize(&spans);
        assert_eq!(s.batches, 2);
        assert_eq!(s.items, 16);
        assert_eq!(s.total_ms, 1440.0);
        assert_eq!(s.per_item, 90.0);
        assert_eq!(s.min_batch_ms, 640.0);
        assert_eq!(s.max_batch_ms, 800.0);
    }

    #[test]
    fn summarize_empty_is_zero_not_nan() {
        let s = summarize(&[]);
        assert_eq!(s.batches, 0);
        assert_eq!(s.per_item, 0.0);   // 不能是 NaN（除零保护）
        assert_eq!(s.min_batch_ms, 0.0);
    }

    #[test]
    fn batch_timing_numbers_from_one_and_throttles() {
        let mut t = BatchTiming::new("classify");
        assert_eq!(t.push(8, 100.0), 1);
        assert_eq!(t.push(8, 120.0), 2);
        assert_eq!(t.total_ms(), 220.0);
        // ≤30 批：全打
        assert!(t.should_log(1) && t.should_log(2));
        assert_eq!(t.label(), "classify");
    }

    #[test]
    fn batch_timing_throttles_when_many_batches() {
        let mut t = BatchTiming::new("embed");
        // 120 批：只打首末 + 每 stride 批（stride = ceil(120/30) = 4）
        for _ in 0..120 {
            t.push(8, 100.0);
        }
        assert!(t.should_log(1), "首批必打");
        assert!(t.should_log(120), "末批必打");
        assert!(t.should_log(4), "stride 整数倍必打");
        assert!(!t.should_log(5), "非采样点不打（避免刷屏）");
    }
}
