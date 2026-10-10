//! 编辑会话：跨命令共享源图与工作区
//! ====================================================================
//! ## 为什么需要会话
//!
//! 拖动滑块时前端每 50~100ms 就要重渲一帧。若每帧都「读文件 → 解码 →
//! 处理 → 编码」，6000×4000 的图光是解码就要 340ms，完全无法交互。
//! 会话把「解码后的源图」与「可复用的工作区」缓存起来，
//! 使重渲的成本降到只剩像素运算。
//!
//! ## 生命周期与并发
//!
//! - 会话以 id 存放在 `SessionStore`（`Mutex<HashMap>`）里；
//! - 前端关掉图片 / 切换图片时显式 `close`；
//! - **兜底**：会话表设上限，超出时按「最后访问时间」淘汰最旧的，
//!   否则用户开 20 张图不关就会吃满内存（每张预览约 20MB + 工作区 40MB）。
//! - 每个会话自带**渲染序号**（`AtomicU64`）：前端拖动时可能积压多个
//!   渲染请求，若不加序号，后到的旧请求会覆盖新结果，表现为「松手后
//!   画面闪回旧状态」。渲染前后各查一次序号，不匹配就丢弃。
//!
//! ## 锁粒度
//!
//! 会话内的 `Mutex<Workspace>` 只在**渲染期间**持有，且渲染走
//! `spawn_blocking`（见 `commands.rs`），因此不会阻塞 Tauri 的异步运行时。
//! 单个会话串行渲染是刻意的：同一张图同时渲染两帧没有意义，
//! 反而会让两个任务争抢同一块工作区缓冲。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use super::mask::MaskImage;
use super::pipeline::Workspace;
use super::recipe::AdjustmentRecipe;
use super::source::SourceImage;

/// 会话内可被命令读写的可变部分。
#[derive(Debug)]
pub struct EditSession {
    /// 原始文件路径
    pub source_path: PathBuf,
    /// 解码后的预览平面（**不是**原分辨率，见 `SourceImage`）
    pub source: SourceImage,
    /// sidecar 配方（若有）
    pub recipe: AdjustmentRecipe,
    /// 工作区（渲染时借用）
    pub workspace: Mutex<Workspace>,
    /// 渲染序号（见模块头「序号」说明）
    pub render_seq: AtomicU64,
    /// 最后访问时间（毫秒时间戳，用于 LRU 淘汰）
    last_used: AtomicU64,
    /// 上一帧的直方图（避免每次渲染都重算给前端）
    pub last_histogram: Mutex<Option<HistogramData>>,
    /// 已解码的蒙版像素（跨帧复用，避免每帧重解 PNG）
    pub mask: Mutex<MaskImage>,
}

/// 一帧的直方图数据。
#[derive(Debug, Clone, Default)]
pub struct HistogramData {
    /// RGB 三通道各 256 桶（平铺，索引 = 值*3 + 通道）
    pub rgb: Vec<u32>,
    /// 亮度 256 桶
    pub luma: Vec<u32>,
    /// 暗部截断像素数
    pub clipped_low: u32,
    /// 高光截断像素数
    pub clipped_high: u32,
}

impl EditSession {
    /// 创建一个会话。
    pub fn new(source_path: PathBuf, source: SourceImage, recipe: AdjustmentRecipe) -> Self {
        Self {
            source_path,
            source,
            recipe,
            workspace: Mutex::new(Workspace::new()),
            render_seq: AtomicU64::new(0),
            last_used: AtomicU64::new(now_millis()),
            last_histogram: Mutex::new(None),
            mask: Mutex::new(MaskImage::empty()),
        }
    }

    /// 取一份蒙版快照供渲染用（跨帧复用解码结果）。
    pub fn mask_snapshot(&self) -> MaskImage {
        self.mask.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// 替换会话内的蒙版像素。
    pub fn set_mask(&self, mask: MaskImage) {
        if let Ok(mut g) = self.mask.lock() {
            *g = mask;
        }
    }

    /// 取下一个渲染序号（返回**本次新签发**的票号，不是自增前的旧值）。
    ///
    /// 返回 `fetch_add(..) + 1` 而非 `fetch_add(..)`：后者的返回值
    /// 永远比计数器当前值小 1，`is_latest` 拿它去比会恒为 false ——
    /// 于是每一帧渲染结果都被判为过期丢弃，表现为「界面永远不出图」。
    pub fn next_render_seq(&self) -> u64 {
        self.render_seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 校验序号仍然是最新的（否则说明渲染期间有更新的请求到达，本次结果应丢弃）。
    pub fn is_latest(&self, seq: u64) -> bool {
        self.render_seq.load(Ordering::SeqCst) == seq
    }

    /// 触碰最后访问时间。
    pub fn touch(&self) {
        self.last_used.store(now_millis(), Ordering::SeqCst);
    }

    /// 距上次访问的毫秒数。
    pub fn idle_millis(&self) -> u64 {
        now_millis().saturating_sub(self.last_used.load(Ordering::SeqCst))
    }

    /// 原图的真实宽高。
    pub fn original_size(&self) -> (u32, u32) {
        (self.source.original_width, self.source.original_height)
    }

    /// 预览平面的宽高。
    pub fn preview_size(&self) -> (u32, u32) {
        (self.source.decoded_width(), self.source.decoded_height())
    }

    /// 清空缓存的直方图（配方变更后调用）。
    pub fn invalidate_histogram(&self) {
        if let Ok(mut g) = self.last_histogram.lock() {
            *g = None;
        }
    }

    /// 估算该会话占用的内存字节数。
    pub fn memory_bytes(&self) -> usize {
        let px = self.source.decoded_width() as usize * self.source.decoded_height() as usize;
        let source = px * 3;
        let ws = self
            .workspace
            .lock()
            .map(|g| g.allocated_bytes())
            .unwrap_or(0);
        source + ws
    }
}

/// 会话表：id → 会话。
#[derive(Debug)]
pub struct SessionStore {
    inner: Mutex<HashMap<u64, Arc<EditSession>>>,
    next_id: AtomicU64,
}

/// `next_id` 从 **1** 起，而不是 `#[derive(Default)]` 给的 0。
///
/// 前端用 `sessionId !== 0` 表示「当前没有打开图片」（见
/// `stores/edit.ts` 的 `hasImage`），因此 0 必须是保留的「无会话」值。
///
/// 若这里从 0 起，**第一张打开的图会拿到 id 0**，`hasImage` 判定为
/// 「没打开图」→ 编辑器停在空状态：不显示照片、工具条与算子控件全部禁用。
/// 症状是「打开照片后界面没反应」，但后端日志里 `edit_open` 明明成功、
/// 会话也确实建好了 —— 排查起来极其误导。
///
/// 顺带说明：这不是前端该改的地方。前端拿 0 当哨兵是合理约定，
/// 让后端避开保留值比让每个调用点都记得「0 是假的」更可靠。
impl Default for SessionStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }
}

/// 最多同时保留的会话数。
///
/// 每张 24MP 图的预览解码约 4.5MB（1/4 DCT 档）+ 工作区约 40MB ≈ 45MB。
/// 8 张 ≈ 360MB —— 超过这个量就该开始淘汰了。
pub const MAX_SESSIONS: usize = 8;

/// 淘汰阈值：会话空闲超过此时长也会被回收（前端异常关闭时的兜底）。
pub const IDLE_EVICT_MILLIS: u64 = 30 * 60 * 1000;

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入会话，返回新 id。
    pub fn insert(&self, session: EditSession) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut g) = self.inner.lock() {
            g.insert(id, Arc::new(session));
            evict_locked(&mut g);
        }
        id
    }

    /// 取出会话的共享引用。
    ///
    /// 返回 `Arc` 而不是 guard：`commands.rs` 的渲染路径要把会话带进
    /// `spawn_blocking` 的任务里，guard 无法跨线程传递，而 `Arc` 可以。
    pub fn get(&self, id: u64) -> Option<Arc<EditSession>> {
        let g = self.inner.lock().ok()?;
        let s = g.get(&id)?.clone();
        s.touch();
        Some(s)
    }

    /// 便利方法：在会话上执行闭包。
    ///
    /// 适合「不需要把会话带出锁」的只读查询（如取尺寸、读配方）。
    pub fn with<T>(&self, id: u64, f: impl FnOnce(&EditSession) -> T) -> Option<T> {
        let s = self.get(id)?;
        Some(f(&s))
    }

    /// 关闭（移除）一个会话，返回是否确实存在。
    pub fn close(&self, id: u64) -> bool {
        self.inner
            .lock()
            .map(|mut g| g.remove(&id).is_some())
            .unwrap_or(false)
    }

    /// 关闭全部会话。
    pub fn close_all(&self) -> usize {
        self.inner
            .lock()
            .map(|mut g| g.drain().count())
            .unwrap_or(0)
    }

    /// 当前会话数。
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 总占用内存字节数（供 devtools 查看）。
    pub fn memory_bytes(&self) -> usize {
        self.inner
            .lock()
            .map(|g| g.values().map(|s| s.memory_bytes()).sum())
            .unwrap_or(0)
    }
}

/// 淘汰：先按空闲时长，再按数量上限。
///
/// 顺序很重要 —— 只按容量淘汰的话，一个用户开 8 张图然后切回去改
/// 第 1 张，会被无声踢掉且前端还持有已失效的 id。
fn evict_locked(map: &mut HashMap<u64, Arc<EditSession>>) {
    // 1) 空闲过久的无条件回收
    let idle_cutoff = IDLE_EVICT_MILLIS;
    map.retain(|_, s| s.idle_millis() < idle_cutoff);

    // 2) 仍超量则淘汰最久未访问的
    while map.len() > MAX_SESSIONS {
        let oldest = map
            .iter()
            .min_by_key(|(_, s)| s.last_used.load(Ordering::SeqCst))
            .map(|(k, _)| *k);
        match oldest {
            Some(k) => {
                map.remove(&k);
            }
            None => break,
        }
    }
}

/// 毫秒时间戳（够用即可，不需要高精度）。
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::super::source::SourceImage;
    use super::*;
    use image::RgbImage;

    fn fake_source(w: u32, h: u32) -> SourceImage {
        SourceImage {
            pixels: RgbImage::new(w, h),
            original_width: w,
            original_height: h,
            subsampled: false,
        }
    }

    fn session(w: u32, h: u32) -> EditSession {
        EditSession::new(
            PathBuf::from("/tmp/x.jpg"),
            fake_source(w, h),
            AdjustmentRecipe::default(),
        )
    }

    #[test]
    fn store_insert_and_close() {
        let s = SessionStore::new();
        let id = s.insert(session(16, 16));
        assert_eq!(s.len(), 1);
        assert!(s.close(id));
        assert_eq!(s.len(), 0);
        assert!(!s.close(id), "重复关闭应返回 false");
    }

    #[test]
    fn store_ids_are_unique() {
        let s = SessionStore::new();
        let a = s.insert(session(8, 8));
        let b = s.insert(session(8, 8));
        assert_ne!(a, b);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn store_with_gives_access() {
        let s = SessionStore::new();
        let id = s.insert(session(64, 32));
        let size = s.with(id, |sess| sess.original_size()).expect("会话应存在");
        assert_eq!(size, (64, 32));
        assert!(
            s.with(id + 999, |_| ()).is_none(),
            "不存在的 id 应返回 None"
        );
    }

    #[test]
    fn store_get_returns_arc_handle() {
        let s = SessionStore::new();
        let id = s.insert(session(8, 8));
        let arc = s.get(id).expect("刚插入的会话应能取到");
        assert_eq!(arc.original_size(), (8, 8));
        assert!(s.get(id + 999).is_none(), "不存在的 id 应返回 None");
    }

    #[test]
    fn store_evicts_beyond_capacity() {
        let s = SessionStore::new();
        for _ in 0..(MAX_SESSIONS + 3) {
            s.insert(session(64, 64));
        }
        assert!(s.len() <= MAX_SESSIONS, "会话数 {} 超过上限", s.len());
    }

    #[test]
    fn close_all_drains() {
        let s = SessionStore::new();
        for _ in 0..4 {
            s.insert(session(8, 8));
        }
        assert_eq!(s.close_all(), 4);
        assert!(s.is_empty());
    }

    #[test]
    fn render_seq_detects_stale_requests() {
        let s = session(16, 16);
        let a = s.next_render_seq();
        let b = s.next_render_seq();
        assert!(s.is_latest(b), "最新序号应通过校验");
        assert!(!s.is_latest(a), "旧序号必须被判定为过期");
    }

    #[test]
    fn memory_accounting_is_positive() {
        let s = session(100, 100);
        // 源图 100*100*3 = 30000，工作区按需增长
        assert!(
            s.memory_bytes() >= 30_000,
            "内存统计 {} 偏小",
            s.memory_bytes()
        );
    }

    #[test]
    fn invalidating_histogram_clears_cache() {
        let s = session(8, 8);
        {
            let mut g = s.last_histogram.lock().unwrap();
            *g = Some(HistogramData {
                rgb: vec![1; 768],
                luma: vec![1; 256],
                clipped_low: 0,
                clipped_high: 0,
            });
        }
        s.invalidate_histogram();
        assert!(s.last_histogram.lock().unwrap().is_none());
    }

    #[test]
    fn session_is_send_and_sync() {
        fn assert_ss<T: Send + Sync>() {}
        assert_ss::<EditSession>();
        assert_ss::<SessionStore>();
    }

    /// 第一个会话的 id 必须**非 0**。
    ///
    /// 前端用 `sessionId !== 0` 当「没有打开图片」的哨兵
    /// （`stores/edit.ts` 的 `hasImage`）。若后端从 0 开始发号，
    /// 第一张打开的图会拿到 id 0 → `hasImage` 为假 → 编辑器停在空状态：
    /// 照片不显示、工具条与算子下拉全部禁用，而后端日志里 `edit_open`
    /// 却是成功的。这类症状与真正的「没打开图」无法从界面上区分。
    #[test]
    fn first_session_id_is_reserved_zero() {
        let store = SessionStore::new();
        let id = store.insert(make_session("a.jpg"));
        assert_ne!(id, 0, "id 0 是保留的「无会话」值，第一个会话不能占用它");
        assert!(store.get(id).is_some());
    }

    /// 连开发号必须唯一且递增 —— 前端按 id 取会话，重复会导致串图。
    #[test]
    fn session_ids_are_unique_and_increasing() {
        let store = SessionStore::new();
        let a = store.insert(make_session("a.jpg"));
        let b = store.insert(make_session("b.jpg"));
        let c = store.insert(make_session("c.jpg"));
        assert!(a < b && b < c, "id 应递增：{a}, {b}, {c}");
        assert_eq!(store.len(), 3);
        // 每个 id 取回的都必须是当初放进去的那张
        assert_eq!(
            store.with(a, |s| s.source_path.clone()),
            Some(std::path::PathBuf::from("a.jpg"))
        );
        assert_eq!(
            store.with(c, |s| s.source_path.clone()),
            Some(std::path::PathBuf::from("c.jpg"))
        );
    }

    /// 造一个最小可用会话（不碰磁盘）。
    fn make_session(name: &str) -> EditSession {
        let img = RgbImage::new(2, 2);
        let decoded = SourceImage {
            pixels: img,
            original_width: 2,
            original_height: 2,
            subsampled: false,
        };
        EditSession::new(
            std::path::PathBuf::from(name),
            decoded,
            super::super::recipe::AdjustmentRecipe::default(),
        )
    }
}
