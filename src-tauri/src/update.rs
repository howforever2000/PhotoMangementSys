//! 应用内更新（检查版本 → 镜像优先下载安装包 → 拉起安装程序）
//!
//! 为什么需要这个模块：安装版用户不会自己去 GitHub 翻 Release 页面，而「有没有新版本」
//! 必须在启动后安静地查一次、且不能拖慢首帧；发现更新后还要能把 177MB 的安装包下下来。
//!
//! 为什么下载要这么设计（2026-11-22 本机实测，不是拍脑袋）：
//!
//! | 通道 | 单连接 | 4 段并发 |
//! |---|---|---|
//! | GitHub 直连 | 0.03 MB/s（177MB ≈ 100 分钟） | 无改善（限速在服务端） |
//! | ghfast.top | 0.49 MB/s | **0.60 MB/s** |
//! | gh-proxy.com | 403（不可用） | — |
//! | 其它镜像 | 不可达 | — |
//!
//! 因此：**镜像优先 + 运行时测速 + 多段并发 + 直连兜底**。这四件事在模型下载器上已经
//! 验证过一轮（`model_dl_seg` 的切段/续传/拼接、`model_dl::expand_github_mirrors`
//! 的镜像表），本模块直接复用，不重写一份 —— 免得镜像表、UA、并发上限出现两套口径。
//!
//! 为什么「测速」不能省：镜像站是第三方公益服务，今天 ghfast 快、明天可能换成别人快，
//! 甚至同一天不同时段差异巨大。写死顺序等于把下载能力押在猜测上；每次下载前对每个
//! 候选拉 256KB 实测吞吐，按实测结果排序后再决定用哪几个源做分段，才是可长期运行的做法。
//!
//! 职责边界：本模块只管「查版本 / 下安装包 / 拉起安装程序」；
//! 分段传输在 `model_dl_seg`，镜像表与 UA 在 `model_dl`，进程无窗口拉起在 `proc_cmd`。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::logger;
use crate::model_dl;

/// 更新源仓库（资产命名：`PhotoManagementSys_<version>_x64-setup.exe`，tag：`v<version>`）
const REPO: &str = "howforever2000/PhotoMangementSys";

/// 自动检查周期（6 小时）
///
/// 原因：GitHub API 匿名限流是 60 次/小时/IP，且检查要走 GitHub 网络；6 小时足够
/// 让用户当天装到新版本，又不会把限流额度浪费在无意义的重复请求上。
const AUTO_CHECK_INTERVAL_MS: u64 = 6 * 60 * 60 * 1000;

/// 测速只拉 256KB
///
/// 取这个值的原因：太小（如 16KB）会被 TLS/首包开销淹没、测不出真实吞吐；
/// 太大（如 4MB）等于每次下载前先白下几 MB。256KB 在 0.5MB/s 的镜像上约 0.5s，
/// 与真实吞吐的偏差 <10%。
const PROBE_BYTES: u64 = 256 * 1024;

/// GitHub API 请求超时（不含下载）
const API_TIMEOUT: Duration = Duration::from_secs(20);

/// 单通道测速超时（超时即视为该通道不可用，换下一个）
const PROBE_TIMEOUT: Duration = Duration::from_secs(12);

/// 单流下载临时文件名（以 `.dl_` 开头，沿用模型下载器的「半成品」约定）
const SINGLE_TMP: &str = ".dl_update_single.exe";

/// 进度事件名（前端订阅）
const EVENT_PROGRESS: &str = "update-download-progress";

/// 发现新版本事件名
const EVENT_AVAILABLE: &str = "update-available";

// ---------------------------------------------------------------------------
// URL 派生
// ---------------------------------------------------------------------------

fn api_latest_url() -> String {
    format!("https://api.github.com/repos/{REPO}/releases/latest")
}

fn page_latest_url() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

/// 资产文件名（`asset_name_for("v5.0.1")` 与 `asset_name_for("5.0.1")` 结果一致）
fn asset_name_for(version: &str) -> String {
    format!(
        "PhotoManagementSys_{}_x64-setup.exe",
        normalize_tag(version)
    )
}

/// 资产下载地址（tag 统一带 `v` 前缀；version 传 `v5.0.1` 或 `5.0.1` 都能得到同一地址）
fn asset_url_for(version: &str) -> String {
    let ver = normalize_tag(version);
    format!(
        "https://github.com/{REPO}/releases/download/v{ver}/{}",
        asset_name_for(&ver)
    )
}

// ---------------------------------------------------------------------------
// 类型
// ---------------------------------------------------------------------------

/// 一个可下载的 Release 资产
#[derive(Clone, Debug, Serialize)]
pub struct UpdateAsset {
    pub name: String,
    /// 字节数；`0` 表示未知（仅回退路径「只知道 tag」时出现）
    pub size: u64,
    pub url: String,
}

/// 一次检查的结果快照（前端据此渲染「有更新 / 已是最新」）
#[derive(Clone, Debug, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub has_update: bool,
    pub published_at: String,
    pub notes: String,
    pub asset: Option<UpdateAsset>,
    pub checked_at_ms: u64,
    /// `"api"`（GitHub API 全量信息）或 `"redirect"`（Release 页面重定向兜底，拿不到 size）
    pub source: String,
}

/// 下载进度（前端进度条 + 按钮态）
#[derive(Clone, Debug, Serialize)]
pub struct UpdateDownloadStatus {
    pub running: bool,
    /// idle | probing | downloading | verifying | done | error | cancelled
    pub stage: String,
    pub received: u64,
    pub total: u64,
    pub bytes_per_sec: u64,
    pub error: Option<String>,
    pub path: Option<String>,
}

impl Default for UpdateDownloadStatus {
    fn default() -> Self {
        Self {
            running: false,
            stage: "idle".into(),
            received: 0,
            total: 0,
            bytes_per_sec: 0,
            error: None,
            path: None,
        }
    }
}

/// 前端一次性拿全的更新状态
#[derive(Clone, Debug, Serialize)]
pub struct UpdateState {
    pub current: String,
    pub info: Option<UpdateInfo>,
    pub download: UpdateDownloadStatus,
}

// ---------------------------------------------------------------------------
// 纯函数（可单测，不碰网络与 AppHandle）
// ---------------------------------------------------------------------------

/// 去掉空白与前导 `v`/`V`；含非数字/点的字符时原样返回。
///
/// 为什么「宽松处理」而不是返回 Err：GitHub 的 tag 由仓库主自己写，`v5.0.1` /
/// `5.0.1` / `V5.0.1` 都出现过。比较版本与拼资产名之前先归一化，能同时兼容这几种
/// 写法；真遇到带别的字符的怪 tag（如 `release-5`），原样返回让上层按「解析失败」
/// 处理（此时 `is_newer` 会 warn + 判无更新，而不是崩掉或误报有更新）。
fn normalize_tag(tag: &str) -> String {
    let s = tag.trim();
    if s.is_empty() {
        return String::new();
    }
    let stripped = s.strip_prefix(['v', 'V']).unwrap_or(s);
    if !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit() || c == '.') {
        stripped.to_string()
    } else {
        s.to_string()
    }
}

/// 解析出的版本号
struct Ver {
    /// 数字段（major.minor.patch[.build]）
    nums: Vec<u64>,
    /// 预发布后缀（`5.0.0-beta.1` 里的 `beta.1`）
    pre: Option<String>,
}

fn parse_ver(raw: &str) -> Option<Ver> {
    let s = raw.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    if s.is_empty() {
        return None;
    }
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, if p.is_empty() { None } else { Some(p.to_string()) }),
        None => (s, None),
    };
    let mut nums = Vec::new();
    for part in core.split('.') {
        if part.is_empty() {
            return None;
        }
        nums.push(part.parse::<u64>().ok()?);
    }
    if nums.is_empty() {
        return None;
    }
    Some(Ver { nums, pre })
}

/// `latest` 是否比 `current` 新（语义化比较，解析失败按「无更新」处理）
///
/// 规则：
/// - 数字段逐位比较，位数不同时缺位补 0（`5.0.0.1 > 5.0.0`）；
/// - 数字段相同则比预发布后缀：正式版 > 预发布（`5.0.0 > 5.0.0-beta.1`）；
/// - 解析失败返回 false + `log_warn`：宁可漏报一次更新，也不能因为一个怪 tag
///   把「已是最新」的用户引导去下载安装包。
fn is_newer(latest: &str, current: &str) -> bool {
    let (a, b) = match (parse_ver(latest), parse_ver(current)) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            logger::log_warn(&format!(
                "[update] 版本号无法解析，按「无更新」处理: latest={latest} current={current}"
            ));
            return false;
        }
    };
    let n = a.nums.len().max(b.nums.len());
    for i in 0..n {
        let x = *a.nums.get(i).unwrap_or(&0);
        let y = *b.nums.get(i).unwrap_or(&0);
        if x != y {
            return x > y;
        }
    }
    match (&a.pre, &b.pre) {
        (None, None) => false,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        // 两个都是预发布（同版本号下极少出现）：用字典序近似，够用于「谁更新」的判断
        (Some(x), Some(y)) => x > y,
    }
}

/// 资产名校验：只接受「文件名」形状的 `.exe`
///
/// 为什么必须校验：资产名来自网络响应，会被拼进本地落位路径（`dir.join(name)`）。
/// 不过滤就是一条远程可控的路径穿越 —— `../a.exe` 能写到下载目录之外，
/// 而 `install_update` 随后会去执行这个文件。所以只允许 ASCII 字母数字与 `._-`，
/// 不得含分隔符/`..`，必须以 `.exe` 结尾（安装包只可能是 exe）。
fn sanitize_asset_name(name: &str) -> Option<String> {
    if name.is_empty() || name.len() > 200 {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".exe") {
        return None;
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return None;
    }
    Some(name.to_string())
}

/// 资产地址白名单：只认本仓库的 `releases/download/` HTTPS 直链
///
/// 为什么：API 响应里的 `browser_download_url` 同样是远程可控输入，我们要用它去
/// 下载并**执行**一个文件。限定前缀能把「仓库被投毒后改指向别处」的风险收敛到
/// github.com 的 release 通道上，而不是任意域名。
fn is_allowed_asset_url(url: &str) -> bool {
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    if !url.starts_with(&prefix) {
        return false;
    }
    // 空白字符不可能出现在合法 URL 里，出现即说明这条输入被污染过
    if url.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    // `..` 会绕过前缀检查指向别的路径（`.../download/v5.0.1/../../x.exe`）
    if url.contains("..") {
        return false;
    }
    true
}

/// 从 `.../releases/tag/v5.0.1` 提取 tag
///
/// 这是 API 不可用时的兜底通道：请求 `releases/latest` 会被 302 到具体 tag 页，
/// 最终 URL 里就带着版本号。其它形状（未重定向、或跳到了别的页面）返回 None。
fn fallback_tag_from_url(final_url: &str) -> Option<String> {
    const MARK: &str = "/releases/tag/";
    let idx = final_url.find(MARK)?;
    let rest = &final_url[idx + MARK.len()..];
    // 去掉查询串/锚点/多余路径段，只取第一段作为 tag
    let tag = rest
        .split(['?', '#', '/'])
        .next()
        .unwrap_or_default()
        .trim();
    if tag.is_empty() {
        None
    } else {
        Some(tag.to_string())
    }
}

/// 解析 `Content-Range: bytes 0-1023/177112232` 里的总长
fn parse_content_range_total(v: &str) -> Option<u64> {
    let s = v.trim().strip_prefix("bytes ")?;
    let (range, total) = s.split_once('/')?;
    // 区间必须是 `起-止`；`bytes */123`（未知区间）这类形状直接判畸形
    if !range.trim().contains('-') {
        return None;
    }
    total.trim().parse::<u64>().ok()
}

/// 距上次检查是否已超过自动检查周期（`last_ms == 0` 表示从未检查过）
fn should_auto_check(last_ms: u64, now_ms: u64) -> bool {
    last_ms == 0 || now_ms.saturating_sub(last_ms) >= AUTO_CHECK_INTERVAL_MS
}

/// 解析 GitHub `releases/latest` 的 JSON：返回 (tag, published_at, body, assets)
///
/// `tag_name` 缺失直接 Err —— 没有 tag 就拼不出资产地址，回退通道才是正确答案；
/// 单个资产缺名字/地址只跳过该条（不能因为一个畸形资产整次检查失败）。
fn parse_api_release(v: &serde_json::Value) -> Result<(String, String, String, Vec<UpdateAsset>), String> {
    let tag = v
        .get("tag_name")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("GitHub API 响应缺少 tag_name")?;
    let published_at = v
        .get("published_at")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let body = v
        .get("body")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let mut assets = Vec::new();
    if let Some(list) = v.get("assets").and_then(|x| x.as_array()) {
        for a in list {
            let name = a.get("name").and_then(|x| x.as_str()).unwrap_or_default().trim();
            let url = a
                .get("browser_download_url")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .trim();
            if name.is_empty() || url.is_empty() {
                continue;
            }
            assets.push(UpdateAsset {
                name: name.to_string(),
                size: a.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                url: url.to_string(),
            });
        }
    }
    Ok((tag.to_string(), published_at, body, assets))
}

/// 从资产列表里挑安装包
///
/// 优先 `*_x64-setup.exe`（NSIS 安装器，我们的标准产物）；退化到「第一个名字含
/// setup 的 .exe」。`.msi` / `.zip` 一律忽略：前者是另一条安装链路（本模块不实现），
/// 后者是模型包，装不了。
fn pick_installer_asset(assets: &[UpdateAsset]) -> Option<UpdateAsset> {
    if let Some(a) = assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with("_x64-setup.exe"))
    {
        return Some(a.clone());
    }
    assets
        .iter()
        .find(|a| {
            let n = a.name.to_ascii_lowercase();
            n.ends_with(".exe") && n.contains("setup")
        })
        .cloned()
}

/// 小时制毫秒时间戳（`update-check.json` 与 `checked_at_ms` 统一用它）
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 状态与节流持久化
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Store {
    info: Option<UpdateInfo>,
    download: UpdateDownloadStatus,
    /// 当前下载任务的取消标志（`None` = 没有进行中的任务）
    cancel: Option<Arc<AtomicBool>>,
    /// 「正在检查」的互斥标记：并发调用不得重复打 GitHub API（限流 60/h）
    checking: bool,
}

/// 全局状态。锁只包住内存字段，**不在锁内 await**（否则整个更新模块会被一次网络
/// 请求堵死，`update_state` 也会跟着卡）。
static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

/// 检查节流记录（落盘，跨进程重启也生效：否则每次启动都查一次，纯浪费限流额度）
#[derive(Default, Serialize, Deserialize)]
struct ThrottleFile {
    last_check_ms: u64,
    latest_tag: String,
}

fn throttle_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::app_data_dir(app)?.join("update-check.json"))
}

/// 读节流记录：缺文件 / 坏文件 / 读失败一律按默认值（= 从未检查过）
///
/// 这里刻意不返回 Err：节流文件只是优化项，坏了顶多多查一次，不该阻断更新功能。
fn load_throttle(app: &AppHandle) -> ThrottleFile {
    let Ok(path) = throttle_path(app) else {
        return ThrottleFile::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return ThrottleFile::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_throttle(app: &AppHandle, t: &ThrottleFile) {
    let Ok(path) = throttle_path(app) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string(t) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                logger::log_warn(&format!("[update] 写入 update-check.json 失败: {e}"));
            }
        }
        Err(e) => logger::log_warn(&format!("[update] 序列化节流记录失败: {e}")),
    }
}

// ---------------------------------------------------------------------------
// 网络：检查版本
// ---------------------------------------------------------------------------

fn api_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(model_dl::USER_AGENT)
        .connect_timeout(Duration::from_secs(12))
        .timeout(timeout)
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

fn download_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(model_dl::USER_AGENT)
        .connect_timeout(Duration::from_secs(12))
        // 读超时按「两次数据之间的最长间隔」算：镜像偶尔卡 30s 不该直接判死
        .read_timeout(Duration::from_secs(60))
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))
}

/// 优先走 GitHub API：能一次拿到 tag / 发布时间 / 更新说明 / 资产体积
async fn api_lookup(
    client: &reqwest::Client,
) -> Result<(String, String, String, Option<UpdateAsset>), String> {
    let resp = client
        .get(api_latest_url())
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("请求 GitHub API 失败: {e}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        // 403 = 匿名限流；404 = 仓库/Release 不存在。都交给回退通道再试一次
        return Err(format!("GitHub API 返回 HTTP {status}"));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 GitHub API 响应失败: {e}"))?;
    let (tag, published_at, body, assets) = parse_api_release(&v)?;
    let asset = pick_installer_asset(&assets);
    if let Some(a) = &asset {
        // 双重校验：这两条守的是「下载并执行一个远程可控文件」的边界
        if !is_allowed_asset_url(&a.url) {
            return Err(format!("安装包地址不在白名单内: {}", a.url));
        }
        if sanitize_asset_name(&a.name).is_none() {
            return Err(format!("安装包文件名不合法: {}", a.name));
        }
    } else {
        logger::log_warn(&format!(
            "[update] Release {tag} 里没有找到 .exe 安装包（资产 {} 个），前端需引导用户手动下载",
            assets.len()
        ));
    }
    Ok((tag, published_at, body, asset))
}

/// 回退通道：请求 `releases/latest`，从 302 后的最终 URL 里取 tag
///
/// 为什么保留这条：API 匿名限流只有 60 次/小时，被限流时（HTTP 403）整个更新功能
/// 不该瞎掉。tag 页的跳转同样带着版本号，虽拿不到体积/更新说明，但足以判断
/// 「有没有新版本」并按约定拼出资产地址。
async fn redirect_lookup(client: &reqwest::Client) -> Result<String, String> {
    let resp = client
        .get(page_latest_url())
        .send()
        .await
        .map_err(|e| format!("请求 Release 页面失败: {e}"))?;
    let status = resp.status().as_u16();
    let final_url = resp.url().to_string();
    if !(200..300).contains(&status) {
        return Err(format!("Release 页面返回 HTTP {status}（{final_url}）"));
    }
    fallback_tag_from_url(&final_url)
        .ok_or_else(|| format!("Release 页面未跳转到 tag 页（{final_url}）"))
}

/// 检查最新版本（不依赖 AppHandle，便于 `#[ignore]` 的网络自测直接调用）
async fn fetch_latest(client: &reqwest::Client, current: &str) -> Result<UpdateInfo, String> {
    let checked_at_ms = now_ms();
    let api_err = match api_lookup(client).await {
        Ok((tag, published_at, notes, asset)) => {
            let latest = normalize_tag(&tag);
            return Ok(UpdateInfo {
                current: current.to_string(),
                has_update: is_newer(&latest, current),
                latest,
                published_at,
                notes,
                asset,
                checked_at_ms,
                source: "api".into(),
            });
        }
        Err(e) => {
            logger::log_warn(&format!(
                "[update] GitHub API 检查失败，回退到 Release 页面重定向: {e}"
            ));
            e
        }
    };

    match redirect_lookup(client).await {
        Ok(tag) => {
            let latest = normalize_tag(&tag);
            // 回退路径只知道 tag：按仓库约定拼资产地址，体积未知（size = 0），
            // 由下载阶段的测速探测补出总长。
            let asset = UpdateAsset {
                name: asset_name_for(&tag),
                size: 0,
                url: asset_url_for(&tag),
            };
            Ok(UpdateInfo {
                current: current.to_string(),
                has_update: is_newer(&latest, current),
                latest,
                published_at: String::new(),
                notes: String::new(),
                asset: Some(asset),
                checked_at_ms,
                source: "redirect".into(),
            })
        }
        Err(e) => Err(format!(
            "检查更新失败：GitHub API（{api_err}）；Release 页面（{e}）"
        )),
    }
}

/// 真正执行一次检查并落状态（手动检查与自动检查共用，避免两套逻辑漂移）
async fn do_check(app: &AppHandle) -> Result<UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    let client = api_client(API_TIMEOUT)?;
    let info = fetch_latest(&client, &current).await?;
    {
        let mut st = STORE.lock().unwrap_or_else(|e| e.into_inner());
        st.info = Some(info.clone());
    }
    save_throttle(
        app,
        &ThrottleFile {
            last_check_ms: now_ms(),
            latest_tag: info.latest.clone(),
        },
    );
    logger::log_info(&format!(
        "[update] 检查完成 current={} latest={} has_update={} source={}",
        info.current, info.latest, info.has_update, info.source
    ));
    Ok(info)
}

// ---------------------------------------------------------------------------
// 命令：检查 / 状态
// ---------------------------------------------------------------------------

/// 检查更新
///
/// `force = false` 且 6 小时内查过 → 直接返回内存里的结果（不联网）。
/// 前端首屏、设置页、关于页都会调这个命令，没有缓存就会变成「打开一个页面打一次 API」。
#[tauri::command]
pub async fn check_update(force: bool, app: AppHandle) -> Result<UpdateInfo, String> {
    if !force {
        let cached = STORE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .info
            .clone();
        if let Some(info) = cached {
            if now_ms().saturating_sub(info.checked_at_ms) < AUTO_CHECK_INTERVAL_MS {
                return Ok(info);
            }
        }
    }
    {
        let mut st = STORE.lock().unwrap_or_else(|e| e.into_inner());
        if st.checking {
            // 已有检查在跑：不排队、不重复打 API。有旧结果就交旧结果，
            // 没有就明确告诉前端「稍候」，让它别连点。
            return match st.info.clone() {
                Some(i) => Ok(i),
                None => Err("正在检查更新，请稍候再试".into()),
            };
        }
        st.checking = true;
    }
    let result = do_check(&app).await;
    // 无论成功失败都要放开互斥标记，否则一次网络错误会让检查功能永久卡死
    STORE.lock().unwrap_or_else(|e| e.into_inner()).checking = false;
    result
}

/// 当前更新状态（不联网）
#[tauri::command]
pub fn update_state(app: AppHandle) -> UpdateState {
    let current = app.package_info().version.to_string();
    let st = STORE.lock().unwrap_or_else(|e| e.into_inner());
    UpdateState {
        current,
        info: st.info.clone(),
        download: st.download.clone(),
    }
}

// ---------------------------------------------------------------------------
// 下载
// ---------------------------------------------------------------------------

/// 下载目录：`app_local_data_dir/updates`
///
/// 用 `app_local_data_dir`（不是 app_data_dir）的原因：这是可再生的下载缓存，
/// 不该混进被备份/漫游的用户数据（安装成功后它就是个可删的 177MB 临时物）。
fn download_dir(app: &AppHandle) -> PathBuf {
    match app.path().app_local_data_dir() {
        Ok(d) => d.join("updates"),
        // 取不到本地数据目录时退回临时目录：宁可下到临时目录，也别让更新功能整个失效
        Err(_) => std::env::temp_dir().join("LumaPerch-updates"),
    }
}

/// 下载目录（命令版，前端复用它 + 已有的 `open_folder` 打开）
#[tauri::command]
pub fn update_download_dir(app: AppHandle) -> String {
    download_dir(&app).to_string_lossy().to_string()
}

/// 在锁内修改下载状态并 emit（所有状态变化都走这里，避免有的路径忘了发事件）
fn patch_download(app: &AppHandle, f: impl FnOnce(&mut UpdateDownloadStatus)) {
    let status = {
        let mut st = STORE.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut st.download);
        st.download.clone()
    };
    let _ = app.emit(EVENT_PROGRESS, &status);
}

/// 进度上报器：200ms 节流 + 用「本次 emit 的增量 / 间隔」算 bps
///
/// 为什么要按 emit 间隔算而不是累计量/总耗时：分段并发下累计曲线会来回抖
/// （某几段先完成、某一段卡住），用户看到的剩余时间会疯狂跳。用最近一个窗口的
/// 增量比时间更稳，也更接近「当下网速」的直觉。
struct Progress {
    app: AppHandle,
    received: Arc<AtomicU64>,
    /// (上次 emit 时刻, 上次 emit 时的累计字节)
    last: Mutex<(Instant, u64)>,
}

impl Progress {
    fn new(app: AppHandle) -> Arc<Self> {
        Arc::new(Self {
            app,
            received: Arc::new(AtomicU64::new(0)),
            // 初值取「200ms 前」，保证第一次回调立即上报（点下去界面马上有反应）
            last: Mutex::new((Instant::now() - Duration::from_millis(200), 0)),
        })
    }

    fn add(&self, delta: u64) {
        let n = self.received.fetch_add(delta, Ordering::Relaxed) + delta;
        let bps = {
            let Ok(mut last) = self.last.lock() else {
                return;
            };
            let now = Instant::now();
            let dt = now.duration_since(last.0);
            if dt < Duration::from_millis(200) {
                return;
            }
            let db = n.saturating_sub(last.1);
            let bps = (db as f64 / dt.as_secs_f64().max(0.001)) as u64;
            *last = (now, n);
            bps
        };
        self.emit(n, bps);
    }

    /// 收尾补报（避免最后一小段没赶上节流窗口，进度条停在 99%）
    fn finish(&self) {
        let n = self.received.load(Ordering::Relaxed);
        self.emit(n, 0);
    }

    fn emit(&self, received: u64, bps: u64) {
        patch_download(&self.app, |d| {
            d.received = received;
            d.bytes_per_sec = bps;
            d.stage = "downloading".into();
        });
    }
}

/// 单个候选通道的测速结果
struct Probe {
    url: String,
    /// 实测吞吐（字节/秒）
    bps: f64,
    /// 从 Content-Range 读到的总长（不支持 Range 时为 None）
    total: Option<u64>,
    /// 是否老实返回 206（决定它能否参与分段）
    range_ok: bool,
}

/// 对一个候选通道测速：拉 `PROBE_BYTES` 并计实际吞吐
///
/// 注意「读完 body 才算数」：只看响应头到达时间会把「TLS 握手快但之后没带宽」
/// 的镜像判成最快。同时读满就停 —— 镜像若无视 Range 返回整个文件（HTTP 200），
/// 也不会把 177MB 全拉进内存。
async fn probe_one(client: &reqwest::Client, url: &str) -> Result<Probe, String> {
    let started = Instant::now();
    let fut = async {
        let resp = client
            .get(url)
            .header("Range", format!("bytes=0-{}", PROBE_BYTES - 1))
            .send()
            .await
            .map_err(|e| format!("请求失败: {e}"))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}"));
        }
        let total = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range_total);
        let mut got = 0usize;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let c = chunk.map_err(|e| format!("读取失败: {e}"))?;
            got += c.len();
            if got as u64 >= PROBE_BYTES {
                break;
            }
        }
        if got == 0 {
            return Err("没有收到数据".into());
        }
        Ok((status, total, got))
    };
    let (status, total, got) = tokio::time::timeout(PROBE_TIMEOUT, fut)
        .await
        .map_err(|_| format!("测速超时（{}s）", PROBE_TIMEOUT.as_secs()))??;
    let secs = started.elapsed().as_secs_f64().max(0.001);
    Ok(Probe {
        url: url.to_string(),
        bps: got as f64 / secs,
        total,
        range_ok: status == 206,
    })
}

/// 单流下载（不支持分段 / 体积不足 16MB 时走这里）
///
/// 续传规则（与模型下载器一致）：磁盘上已有半包就带 `Range: bytes=<len>-`，
/// **只有 206 才追加**；服务器回 200（忽略 Range）必须清空重下 —— 追加会得到
/// 「前 N 字节重复一遍」的坏文件，而且大小看着还挺像真的。
async fn download_single(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    total: u64,
    on_bytes: Arc<dyn Fn(u64) + Send + Sync>,
    cancel: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<(), String> {
    let have = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let mut file: Option<std::fs::File> = None;
    let mut written = 0u64;
    let mut pending: Option<reqwest::Response> = None;

    if have > 0 && have < total {
        let resp = client
            .get(url)
            .header("Range", format!("bytes={have}-"))
            .send()
            .await
            .map_err(|e| format!("续传请求失败: {e}"))?;
        match resp.status().as_u16() {
            206 => {
                file = Some(
                    std::fs::OpenOptions::new()
                        .append(true)
                        .open(dest)
                        .map_err(|e| format!("打开续传文件失败: {e}"))?,
                );
                written = have;
                // 断点前的字节也计入进度，避免续传时进度条从 0 又跳一遍
                on_bytes(have);
            }
            200 => pending = Some(resp),
            s => return Err(format!("下载失败（HTTP {s}）: {url}")),
        }
    }

    let resp = match pending {
        Some(r) => r,
        None => {
            if written == 0 {
                let _ = std::fs::remove_file(dest);
            }
            client
                .get(url)
                .send()
                .await
                .map_err(|e| format!("下载请求失败: {e}"))?
        }
    };
    let status = resp.status().as_u16();
    if status != 200 && status != 206 {
        return Err(format!("下载失败（HTTP {status}）: {url}"));
    }

    let mut f = match file {
        Some(f) => f,
        None => std::fs::File::create(dest).map_err(|e| format!("创建安装包失败: {e}"))?,
    };
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel() {
            let _ = f.sync_all();
            return Err("已取消（安装包下载已停止）".into());
        }
        let c = chunk.map_err(|e| format!("下载中断: {e}"))?;
        // 防御：镜像超发时不写坏后续字节（截断到 total）
        let remain = total.saturating_sub(written);
        let take: &[u8] = if c.len() as u64 > remain {
            &c[..remain as usize]
        } else {
            &c[..]
        };
        if take.is_empty() {
            break;
        }
        f.write_all(take)
            .map_err(|e| format!("写入安装包失败: {e}"))?;
        written += take.len() as u64;
        on_bytes(take.len() as u64);
        if written >= total {
            break;
        }
    }
    let _ = f.sync_all();
    if written != total {
        return Err(format!(
            "下载不完整（得到 {written} 字节，期望 {total}）——可能是镜像截断，请重试"
        ));
    }
    Ok(())
}

/// 把段键里的非法字符洗掉（分片文件名要用它，tag 是远程输入）
fn keyify(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    if cleaned.is_empty() {
        "unknown".into()
    } else {
        cleaned
    }
}

/// 完整下载流程：测速 → 选路（分段 / 单流）→ 落位 → 校验
async fn download_flow(
    app: &AppHandle,
    asset: &UpdateAsset,
    latest: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let dir = download_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新下载目录失败: {e}"))?;
    let name = sanitize_asset_name(&asset.name)
        .ok_or_else(|| format!("安装包文件名不合法: {}", asset.name))?;
    let dest = dir.join(&name);

    // 幂等：已经下好（字节数对得上）就直接复用，别让用户重复点一次又白跑 177MB
    if asset.size > 0 && dest.is_file() {
        let have = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if have == asset.size {
            logger::log_info(&format!(
                "[update] 安装包已存在且完整，跳过下载: {}",
                dest.display()
            ));
            return Ok(dest);
        }
        // 体积不符（上次被截断 / 版本换了但同名）：删掉重下，免得装到半个文件
        let _ = std::fs::remove_file(&dest);
    }

    let client = download_client()?;

    // 候选：镜像优先 + 直连兜底（与模型下载器同一张镜像表）
    let mut candidates: Vec<String> = Vec::new();
    for u in model_dl::expand_github_mirrors(&asset.url) {
        if !candidates.contains(&u) {
            candidates.push(u);
        }
    }

    // 逐个测速（串行：并发测速会互相抢带宽，测出来的排序没意义）
    let mut probes: Vec<Probe> = Vec::new();
    for url in &candidates {
        if cancel.load(Ordering::SeqCst) {
            return Err("已取消（安装包下载已停止）".into());
        }
        match probe_one(&client, url).await {
            Ok(p) => {
                logger::log_info(&format!(
                    "[update] 通道测速 {:.2} MB/s range={} total={} {}",
                    p.bps / (1024.0 * 1024.0),
                    p.range_ok,
                    p.total.map(|t| t.to_string()).unwrap_or_else(|| "-".into()),
                    p.url
                ));
                probes.push(p);
            }
            Err(e) => logger::log_warn(&format!("[update] 通道不可用（{e}）: {url}")),
        }
    }
    if probes.is_empty() {
        return Err("所有下载通道都不可用（镜像与直连均失败）".into());
    }
    probes.sort_by(|a, b| b.bps.partial_cmp(&a.bps).unwrap_or(std::cmp::Ordering::Equal));

    // 总长：优先用 API 给的资产体积（权威口径），否则用探测到的 Content-Range
    let probed_total = probes.iter().find_map(|p| p.total).unwrap_or(0);
    let total = if asset.size > 0 { asset.size } else { probed_total };
    if total == 0 {
        return Err("无法确定安装包大小".into());
    }

    patch_download(app, |d| {
        d.running = true;
        d.stage = "downloading".into();
        d.received = 0;
        d.total = total;
        d.bytes_per_sec = 0;
        d.error = None;
        d.path = None;
    });

    let progress = Progress::new(app.clone());
    let counter = progress.clone();
    let on_bytes: Arc<dyn Fn(u64) + Send + Sync> = Arc::new(move |d| counter.add(d));
    let flag = cancel.clone();
    let is_cancelled: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || flag.load(Ordering::SeqCst));

    let best_bps = probes[0].bps;
    // 轮转源：只用实测吞吐不低于最优 25% 的通道（更慢的加进来只会拖后腿），
    // 最多 3 个（再多会触发镜像 429），且必须支持 206。
    let mut rotation: Vec<String> = probes
        .iter()
        .filter(|p| p.range_ok && p.bps >= best_bps * 0.25)
        .map(|p| p.url.clone())
        .take(3)
        .collect();
    let range_any = probes.iter().any(|p| p.range_ok);

    if total >= crate::model_dl_seg::SEG_MIN_BYTES && range_any {
        if rotation.is_empty() {
            rotation = probes
                .iter()
                .filter(|p| p.range_ok)
                .map(|p| p.url.clone())
                .collect();
        }
        // 段键带版本号：版本一变分片就不再复用，避免把新旧两个版本的字节拼成一个文件
        let key = format!("update-{}", keyify(latest));
        let plan = crate::model_dl_seg::plan_segments(total, &rotation);
        if !plan.is_empty() {
            logger::log_info(&format!(
                "[update] 分段下载：{} 段 / {} 个源 / 共 {:.1} MB",
                plan.len(),
                rotation.len(),
                total as f64 / (1024.0 * 1024.0)
            ));
            let merged = crate::model_dl_seg::fetch_and_concat(
                &client,
                plan,
                &dir,
                &key,
                "exe",
                is_cancelled.clone(),
                on_bytes.clone(),
            )
            .await?;
            progress.finish();
            finish_file(&merged, &dest, total)?;
            let _ = std::fs::remove_file(dir.join(SINGLE_TMP));
            return Ok(dest);
        }
        logger::log_warn("[update] 分段计划为空，回落单流下载");
    }

    // 单流：用实测最快的通道
    let url = probes[0].url.clone();
    logger::log_info(&format!(
        "[update] 单流下载（{:.2} MB/s）: {url}",
        probes[0].bps / (1024.0 * 1024.0)
    ));
    let tmp = dir.join(SINGLE_TMP);
    download_single(&client, &url, &tmp, total, on_bytes, is_cancelled).await?;
    progress.finish();
    finish_file(&tmp, &dest, total)?;
    Ok(dest)
}

/// 校验并落位：字节数不等于 `total` 一律删文件报错
///
/// 为什么必须校验：镜像截断不会报错（HTTP 层看起来是正常结束），装的时候才炸
/// （NSIS 报「安装包已损坏」），而且用户已经等了半小时。在这里拦住并给出
/// 「得到 N 字节，期望 M」的数字，重试一次通常就好了。
fn finish_file(src: &Path, dest: &Path, total: u64) -> Result<(), String> {
    let got = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    if got != total {
        let _ = std::fs::remove_file(src);
        return Err(format!(
            "下载不完整（得到 {got} 字节，期望 {total}）——可能是镜像截断，请重试"
        ));
    }
    std::fs::rename(src, dest).map_err(|e| format!("落位安装包失败: {e}"))?;
    Ok(())
}

/// 后台下载任务：**无论 Ok/Err 都必须落终态并把 `running` 置回 false**
///
/// 这条是硬约束：漏掉任何一个分支，前端按钮会永久停在「下载中」，用户只能重启应用。
async fn run_download(app: AppHandle, asset: UpdateAsset, latest: String, cancel: Arc<AtomicBool>) {
    patch_download(&app, |d| {
        d.stage = "probing".into();
        d.total = asset.size;
    });

    let res = download_flow(&app, &asset, &latest, &cancel).await;
    match res {
        Ok(path) => {
            logger::log_info(&format!("[update] 安装包就绪: {}", path.display()));
            let p = path.to_string_lossy().to_string();
            patch_download(&app, |d| {
                d.received = d.total;
                d.bytes_per_sec = 0;
                d.stage = "done".into();
                d.error = None;
                d.path = Some(p.clone());
                d.running = false;
            });
        }
        Err(e) => {
            // 取消与出错必须区分：取消要保留分片（下次点下载直接续传），
            // 出错要让用户看到原因（白名单/截断/通道全挂各有各的处理方式）
            let cancelled = cancel.load(Ordering::SeqCst);
            let stage = if cancelled { "cancelled" } else { "error" };
            logger::log_warn(&format!("[update] 下载结束 stage={stage}（{e}）"));
            patch_download(&app, |d| {
                d.bytes_per_sec = 0;
                d.stage = stage.into();
                d.error = if cancelled { None } else { Some(e.clone()) };
                d.running = false;
            });
        }
    }
}

/// 开始下载更新安装包
#[tauri::command]
pub async fn start_update_download(app: AppHandle) -> Result<(), String> {
    let (asset, latest) = {
        let st = STORE.lock().unwrap_or_else(|e| e.into_inner());
        let info = st
            .info
            .clone()
            .ok_or("尚未检查到更新信息，请先检查更新")?;
        if st.download.running {
            return Err("下载已在进行".into());
        }
        let asset = info
            .asset
            .clone()
            .ok_or("该版本未提供可自动安装的 .exe 安装包，请到 Release 页面手动下载")?;
        (asset, info.latest)
    };
    if sanitize_asset_name(&asset.name).is_none() {
        return Err(format!("安装包文件名不合法: {}", asset.name));
    }

    let cancel = Arc::new(AtomicBool::new(false));
    {
        // 置位与检查在同一把锁内完成：两次快速点击只会有一次真正开跑（TOCTOU）
        let mut st = STORE.lock().unwrap_or_else(|e| e.into_inner());
        if st.download.running {
            return Err("下载已在进行".into());
        }
        st.download = UpdateDownloadStatus {
            running: true,
            stage: "probing".into(),
            received: 0,
            total: asset.size,
            bytes_per_sec: 0,
            error: None,
            path: None,
        };
        st.cancel = Some(cancel.clone());
    }
    patch_download(&app, |_| {});
    logger::log_info(&format!(
        "[update] 开始下载 {}（{} 字节）",
        asset.name, asset.size
    ));

    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        run_download(h, asset, latest, cancel).await;
    });
    Ok(())
}

/// 取消下载（分片保留，再点下载可续传）
#[tauri::command]
pub fn cancel_update_download() {
    let st = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = &st.cancel {
        c.store(true, Ordering::SeqCst);
        logger::log_info("[update] 收到取消下载请求");
    }
}

// ---------------------------------------------------------------------------
// 安装
// ---------------------------------------------------------------------------

/// 体积校验：`size > 0` 才比对字节数
///
/// `size == 0` 表示体积未知（回退通道只拿得到 tag），此时只要求「文件存在」——
/// 调用方已经保证过这一点。不能因为拿不到体积就把一个已下好的安装包判为不可用。
fn check_installer_size(path: &Path, size: u64) -> Result<(), String> {
    if size == 0 {
        return Ok(());
    }
    let have = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if have != size {
        return Err(format!(
            "安装包不完整（{have} 字节，期望 {size} 字节），请重新下载"
        ));
    }
    Ok(())
}

/// 拉起安装程序并退出本进程
///
/// 为什么要 `ping` 等 2 秒：NSIS 安装器会检测「应用是否在运行」，本进程不退它就弹
/// 「请先关闭应用」。所以先起一个延迟启动的命令（本进程退出后它还在跑），再让
/// 本进程自己退出。`ping -n 3` 是 Windows 上不依赖外部程序的「睡 2 秒」惯用法
/// （`timeout` 在无控制台时需要交互，`sleep` 不存在）。
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    // 安装包路径**优先取下载完成时落盘的 `download.path`**，不再只按 `asset.name` 现拼。
    //
    // 原因（边界）：用户「已下载完成」后又点了一次「检查更新」，`info` 会被换成更新的
    // 版本，`asset.name` 跟着变成新文件名 —— 此时按名字现拼路径，就会找不到那个
    // 已经下好、并且用户正等着装的安装包（表现为「安装包不存在，请先下载更新」，
    // 而按钮明明显示「安装包已就绪」）。`download.path` 是下载真正落盘的路径，
    // 不受后续 `info` 变化影响。
    //
    // 回落到 `download_dir(app).join(asset.name)` 仍然保留：老版本/进程内刚恢复状态时
    // `download.path` 可能为空，而目录里确实躺着那个文件。两者都没有才报错。
    let (cached_path, asset) = {
        let st = STORE.lock().unwrap_or_else(|e| e.into_inner());
        let cached = st
            .download
            .path
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| p.is_file());
        let asset = st.info.clone().and_then(|i| i.asset);
        (cached, asset)
    };

    let path = match cached_path {
        Some(p) => {
            // 体积未知（`size == 0`，回退通道只知道 tag）时不做体积校验，
            // 但「是个文件」这条已经由上面的 filter 保证过了
            let size = asset.as_ref().map(|a| a.size).unwrap_or(0);
            check_installer_size(&p, size)?;
            p
        }
        None => {
            let asset = asset.ok_or("没有可安装的更新包，请先检查更新并下载")?;
            let name = sanitize_asset_name(&asset.name)
                .ok_or_else(|| format!("安装包文件名不合法: {}", asset.name))?;
            let p = download_dir(&app).join(&name);
            if !p.is_file() {
                return Err("安装包不存在，请先下载更新".into());
            }
            check_installer_size(&p, asset.size)?;
            p
        }
    };
    let display = path.to_string_lossy().to_string();
    // 路径要嵌进 shell 命令：含引号说明这条路径不可能是我们拼出来的，直接拒绝
    if display.contains('"') {
        return Err(format!("安装包路径包含非法字符（引号）: {display}"));
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Stdio;
        // 走 proc_cmd：发布版是 GUI 子系统（无控制台），裸 Command 会闪一个黑窗
        let mut cmd = crate::proc_cmd::command("cmd");
        cmd.args([
            "/C",
            &format!("ping -n 3 127.0.0.1 >nul & start \"\" \"{display}\""),
        ]);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        cmd.spawn().map_err(|e| format!("启动安装程序失败: {e}"))?;
        logger::log_info(&format!("[update] 启动安装包 {display}"));
        // 命令先返回 Ok：前端才有机会提示「正在退出并启动安装器」。
        // 600ms 后退出，给 IPC 响应与 WebView 一帧的时间。
        let h = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            h.exit(0);
        });
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = &app;
        Err("自动安装仅支持 Windows，请手动运行下载好的安装包".into())
    }
}

// ---------------------------------------------------------------------------
// 自动检查（lib.rs setup 调用）
// ---------------------------------------------------------------------------

/// 启动后的自动检查（失败只该记日志，不打扰用户）
///
/// - 开发态不联网：`cargo tauri dev` 下每次热重启都查一次 GitHub 毫无意义，
///   还会把匿名限流额度吃掉。需要验证自动检查时用 `PMS_UPDATE_CHECK=1`。
/// - 6 小时节流：节流记录落盘，重启也能生效。
pub async fn auto_check(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    if cfg!(debug_assertions) && std::env::var("PMS_UPDATE_CHECK").as_deref() != Ok("1") {
        return Ok(None);
    }
    let persisted = load_throttle(&app);
    if !should_auto_check(persisted.last_check_ms, now_ms()) {
        return Ok(None);
    }
    {
        let mut st = STORE.lock().unwrap_or_else(|e| e.into_inner());
        if st.checking {
            // 用户已经手动点过检查：让给手动那条路，别重复打 API
            return Ok(None);
        }
        st.checking = true;
    }
    let result = do_check(&app).await;
    STORE.lock().unwrap_or_else(|e| e.into_inner()).checking = false;
    let info = result?;
    if info.has_update {
        let _ = app.emit(EVENT_AVAILABLE, &info);
    }
    Ok(Some(info))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------- is_newer ----------------

    /// 版本比较表：覆盖跨段进位、预发布、多段、非语义化输入
    #[test]
    fn is_newer_table() {
        let cases: &[(&str, &str, bool)] = &[
            ("5.0.1", "5.0.0", true),
            ("5.0.0", "5.0.0", false),
            ("4.5.9", "5.0.0", false),
            ("v5.10.0", "v5.9.9", true),
            // 预发布 < 同版本正式版
            ("5.0.0-beta.1", "5.0.0", false),
            // 多出一段且更大
            ("5.0.0.1", "5.0.0", true),
            // 解析不了的输入：不 panic，一律按「无更新」
            ("", "5.0.0", false),
            ("abc", "5.0.0", false),
            ("5.0.0", "", false),
        ];
        for (latest, current, want) in cases {
            assert_eq!(
                is_newer(latest, current),
                *want,
                "is_newer({latest:?}, {current:?}) 应为 {want}"
            );
        }
    }

    // ---------------- parse_api_release ----------------

    /// 真实形状的 `releases/latest` 响应（4 个资产，tag v5.0.0）
    const API_RELEASE_JSON: &str = r#"{
        "tag_name": "v5.0.0",
        "published_at": "2026-11-22T10:31:00Z",
        "body": "5.0.0：应用内更新 + 人像形变",
        "assets": [
            {
                "name": "model.zip",
                "size": 1548203311,
                "browser_download_url": "https://github.com/howforever2000/PhotoMangementSys/releases/download/v5.0.0/model.zip"
            },
            {
                "name": "openvino-npu-win-x64.zip",
                "size": 42881996,
                "browser_download_url": "https://github.com/howforever2000/PhotoMangementSys/releases/download/v5.0.0/openvino-npu-win-x64.zip"
            },
            {
                "name": "PhotoManagementSys_5.0.0_x64-setup.exe",
                "size": 177112232,
                "browser_download_url": "https://github.com/howforever2000/PhotoMangementSys/releases/download/v5.0.0/PhotoManagementSys_5.0.0_x64-setup.exe"
            },
            {
                "name": "PhotoManagementSys_5.0.0_x64_en-US.msi",
                "size": 178000000,
                "browser_download_url": "https://github.com/howforever2000/PhotoMangementSys/releases/download/v5.0.0/PhotoManagementSys_5.0.0_x64_en-US.msi"
            }
        ]
    }"#;

    #[test]
    fn parse_api_release_picks_setup_asset() {
        let v: serde_json::Value = serde_json::from_str(API_RELEASE_JSON).unwrap();
        let (tag, published_at, body, assets) = parse_api_release(&v).unwrap();
        assert_eq!(tag, "v5.0.0");
        assert_eq!(published_at, "2026-11-22T10:31:00Z");
        assert!(body.contains("应用内更新"), "更新说明应被原样带出");
        assert_eq!(assets.len(), 4, "四个资产都应被解析出来");
        let picked = pick_installer_asset(&assets).expect("应挑出 setup.exe");
        assert_eq!(picked.name, "PhotoManagementSys_5.0.0_x64-setup.exe");
        assert_eq!(picked.size, 177112232);
        assert!(is_allowed_asset_url(&picked.url));
        // tag 缺失必须 Err（没有 tag 就拼不出下载地址，应回退重定向通道）
        let bad = serde_json::json!({ "assets": [] });
        assert!(parse_api_release(&bad).is_err());
    }

    // ---------------- pick_installer_asset ----------------

    fn asset(name: &str) -> UpdateAsset {
        UpdateAsset {
            name: name.to_string(),
            size: 1,
            url: format!("https://github.com/{REPO}/releases/download/v5.0.1/{name}"),
        }
    }

    #[test]
    fn pick_installer_asset_rules() {
        // 只有 msi / zip → 装不了，返回 None
        assert!(pick_installer_asset(&[asset("a.msi"), asset("model.zip")]).is_none());
        assert!(pick_installer_asset(&[]).is_none());
        // setup.exe 优先于同批次的其它 exe
        let picked = pick_installer_asset(&[
            asset("PhotoManagementSys_5.0.1_x64-setup.exe"),
            asset("other_setup.exe"),
            asset("a.msi"),
        ])
        .expect("应挑出 _x64-setup.exe");
        assert_eq!(picked.name, "PhotoManagementSys_5.0.1_x64-setup.exe");
        // 退化规则：没有标准名时取第一个名字含 setup 的 .exe
        let picked = pick_installer_asset(&[asset("tool.exe"), asset("mysetup_arm.exe")])
            .expect("应退化到含 setup 的 exe");
        assert_eq!(picked.name, "mysetup_arm.exe");
    }

    // ---------------- asset_url_for ----------------

    #[test]
    fn asset_url_for_accepts_both_tag_forms() {
        let want = format!(
            "https://github.com/{REPO}/releases/download/v5.0.1/PhotoManagementSys_5.0.1_x64-setup.exe"
        );
        assert_eq!(asset_url_for("v5.0.1"), want);
        assert_eq!(asset_url_for("5.0.1"), want);
        // 生成的地址必须过白名单校验（两条口径要自洽）
        assert!(is_allowed_asset_url(&want));
    }

    // ---------------- sanitize_asset_name ----------------

    #[test]
    fn sanitize_asset_name_rules() {
        assert_eq!(
            sanitize_asset_name("PhotoManagementSys_5.0.1_x64-setup.exe").as_deref(),
            Some("PhotoManagementSys_5.0.1_x64-setup.exe")
        );
        for bad in [
            "../a.exe",
            "a/b.exe",
            r"a\b.exe",
            "a.msi",
            "",
            "a.exe.txt",
            "a b.exe",
            &"a".repeat(300),
        ] {
            assert!(
                sanitize_asset_name(bad).is_none(),
                "应拒绝非法资产名: {bad:?}"
            );
        }
    }

    // ---------------- is_allowed_asset_url ----------------

    #[test]
    fn asset_url_whitelist() {
        assert!(is_allowed_asset_url(&format!(
            "https://github.com/{REPO}/releases/download/v5.0.1/PhotoManagementSys_5.0.1_x64-setup.exe"
        )));
        for bad in [
            format!("http://github.com/{REPO}/releases/download/v5.0.1/a.exe"),
            "https://github.com/someone/other/releases/download/v5.0.1/a.exe".to_string(),
            "https://evil.example.com/releases/download/v5.0.1/a.exe".to_string(),
            format!("https://github.com/{REPO}/releases/download/v5.0.1/../../../a.exe"),
        ] {
            assert!(!is_allowed_asset_url(&bad), "应拒绝: {bad}");
        }
    }

    // ---------------- fallback_tag_from_url ----------------

    #[test]
    fn fallback_tag_extraction() {
        assert_eq!(
            fallback_tag_from_url(&format!("https://github.com/{REPO}/releases/tag/v5.0.1")).as_deref(),
            Some("v5.0.1")
        );
        // 未重定向（页面仍是 /latest）→ 拿不到 tag
        assert!(fallback_tag_from_url(&page_latest_url()).is_none());
        assert!(fallback_tag_from_url("https://github.com/x/y").is_none());
        // 多余路径段/查询串要被裁掉
        assert_eq!(
            fallback_tag_from_url("https://github.com/x/y/releases/tag/v5.0.1/extra?tab=readme")
                .as_deref(),
            Some("v5.0.1")
        );
    }

    // ---------------- parse_content_range_total ----------------

    #[test]
    fn content_range_total_parsing() {
        assert_eq!(
            parse_content_range_total("bytes 0-1023/177112232"),
            Some(177112232)
        );
        for bad in [
            "",
            "bytes 0-1023",
            "bytes /177112232",
            "bytes 0-1023/abc",
            "items 0-1023/100",
        ] {
            assert!(
                parse_content_range_total(bad).is_none(),
                "畸形 Content-Range 应返回 None: {bad:?}"
            );
        }
    }

    // ---------------- should_auto_check ----------------

    #[test]
    fn auto_check_throttle() {
        const H: u64 = 60 * 60 * 1000;
        let now = 1_700_000_000_000u64;
        assert!(should_auto_check(0, now), "从未检查过 → 应检查");
        assert!(!should_auto_check(now - H, now), "1 小时前 → 不检查");
        assert!(should_auto_check(now - 6 * H, now), "正好 6 小时 → 检查");
        assert!(should_auto_check(now - 7 * H, now), "7 小时前 → 检查");
        assert!(!should_auto_check(now + H, now), "上次检查在未来（时钟回拨）→ 不检查");
    }

    // ---------------- 网络自测（默认忽略） ----------------

    /// 真实网络自测：`cargo test --lib update:: -- --ignored --nocapture`
    ///
    /// 它守的是三件「只能联网才验证得到」的事：
    /// 1) GitHub API 的响应形状没有变（资产名/字段还能解析出 setup.exe）；
    /// 2) 镜像当前可用、且 `plan_segments` + `fetch_and_concat` 在真实 CDN 上拉的
    ///    字节与官方直链 `Range` 拿到的字节**逐字节一致**（拼接没串段、没丢块）；
    /// 3) 各通道当前吞吐（打印出来供下次调整镜像顺序时对照）。
    #[tokio::test]
    #[ignore]
    async fn live_check_and_segmented_range() {
        let client = download_client().expect("构建 HTTP 客户端");

        // 1) 检查版本
        let info = fetch_latest(&client, "5.0.0").await.expect("应能查到最新版本");
        println!(
            "latest={} source={} published_at={} has_update={}",
            info.latest, info.source, info.published_at, info.has_update
        );
        assert!(info.latest.starts_with("5."), "最新 tag 形如 5.x.y：{}", info.latest);
        let asset = info.asset.expect("应能取到安装包资产");
        assert!(
            asset.name.to_ascii_lowercase().ends_with("_x64-setup.exe"),
            "应取到 setup.exe：{}",
            asset.name
        );
        println!("asset={} size={}", asset.name, asset.size);

        // 2) 逐通道测速，挑最快的
        let candidates = model_dl::expand_github_mirrors(&asset.url);
        let mut best: Option<(String, f64)> = None;
        for url in &candidates {
            let started = Instant::now();
            match probe_one(&client, url).await {
                Ok(p) => {
                    println!(
                        "{:.2} MB/s  range={}  {}  ({:.1}s)",
                        p.bps / (1024.0 * 1024.0),
                        p.range_ok,
                        p.url,
                        started.elapsed().as_secs_f64()
                    );
                    if best.as_ref().map(|(_, b)| p.bps > *b).unwrap_or(true) {
                        best = Some((p.url.clone(), p.bps));
                    }
                }
                Err(e) => println!("不可用（{e}）：{url}"),
            }
        }
        let (url, bps) = best.expect("至少应有一个通道可用");
        println!("选中通道 {:.2} MB/s: {url}", bps / (1024.0 * 1024.0));

        // 3) 用分段并发拉前 8MB，并与直链 Range 的结果做字节级比对
        const N: u64 = 8 * 1024 * 1024;
        let plan = crate::model_dl_seg::plan_segments(N, std::slice::from_ref(&url));
        assert!(!plan.is_empty(), "应产出分段计划");
        let dir = std::env::temp_dir().join(format!(
            "pms-update-live-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let started = Instant::now();
        let merged = crate::model_dl_seg::fetch_and_concat(
            &client,
            plan,
            &dir,
            "live",
            "bin",
            Arc::new(|| false),
            Arc::new(|_| {}),
        )
        .await
        .expect("分段下载应成功");
        println!(
            "分段下载 {N} 字节耗时 {:.1}s",
            started.elapsed().as_secs_f64()
        );
        let seg_bytes = std::fs::read(&merged).expect("读取分段结果");
        assert_eq!(seg_bytes.len() as u64, N, "分段结果应为 8MiB");

        let resp = client
            .get(&url)
            .header("Range", format!("bytes=0-{}", N - 1))
            .send()
            .await
            .expect("参考请求应成功");
        assert_eq!(resp.status().as_u16(), 206, "参考请求应得到 206");
        let ref_bytes = resp.bytes().await.expect("读取参考字节");
        assert_eq!(ref_bytes.len() as u64, N, "参考字节应为 8MiB");
        assert!(seg_bytes == ref_bytes.to_vec(), "分段结果与直链 Range 必须逐字节一致");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
