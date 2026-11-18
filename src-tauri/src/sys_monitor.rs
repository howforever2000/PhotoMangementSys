//! 资源使用情况面板（FEAT-179，迭代：仅本系统组件 + 内部启停）
//! ===================================================================
//! 职责：展示「与相册管理相关的程序」的 PID / CPU / 内存，并在面板内
//! 提供可拥有服务的启动 / 停止。组件范围固定 5 类：
//!   1. 主程序 App（本应用自身，只展示，不可停）；
//!   2. 视觉识别服务 VCR（可启停）；
//!   3. AI 创作服务 AiStudio（可启停）；
//!   4. 本地 AI 修图网关 Gateway（8000，外部手动启动，App 不拥有）；
//!   5. 图像生成服务 SdServer（1234，外部手动启动，App 不拥有）。
//!
//! 设计纪律：
//!   · 只按「已知 PID + 已知进程特征」定点识别，不列全机其它进程；
//!     VCR/AI 的 PID 来自现有只读源并验正身，网关/sd-server 靠命令行特征
//!     匹配（openai_gateway / sd-server.exe），不误认无关进程。
//!   · 启停只作用于 App 拥有的两个服务（VCR / AI 创作），复用验身强杀
//!     流程并补清内存态 base / 实例文件，保证停止后可重新拉起。
//!   · CPU% 基于全局常驻 System 的两次 refresh 间隔；采集 / 建窗 / 停止
//!     都走 spawn_blocking，不占主线程事件循环（BUG-2026-0910-001/005）。

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sysinfo::{ProcessesToUpdate, System};
use tauri::Manager;

use crate::{ai_service, logger, vision};

/// 常驻采样器：sysinfo 的进程 CPU% 基于「上一次 refresh」计算
static SAMPLER: Mutex<Option<System>> = Mutex::new(None);
/// 上次采样时刻，用于标注采样间隔（首次为 None）
static LAST_SAMPLE: Mutex<Option<Instant>> = Mutex::new(None);

/// 组件类别（对应前端 ComponentKind，snake_case）
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    /// 主程序（本应用自身）
    App,
    /// 视觉识别服务
    Vcr,
    /// AI 创作服务
    AiStudio,
    /// 本地 AI 修图网关（8000，外部）
    Gateway,
    /// 图像生成服务 sd-server（1234，外部）
    SdServer,
}

/// 单个组件的采样事实
#[derive(Debug, Clone, Serialize)]
pub struct ComponentUsage {
    /// 稳定键："app" | "vcr" | "ai_studio" | "gateway" | "sd_server"
    pub key: String,
    pub name: String,
    pub kind: ComponentKind,
    pub running: bool,
    pub pid: Option<u32>,
    /// CPU 占用百分比（间隔增量；首次为启动以来均值）
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub memory_percent: f32,
    /// 是否可由本面板启动
    pub can_start: bool,
    /// 是否可由本面板停止
    pub can_stop: bool,
    /// 状态 / 口径说明
    pub note: String,
}

/// GPU 配置态摘要（来自识别服务 /gpu，非实时利用率）
#[derive(Debug, Clone, Serialize)]
pub struct GpuUsage {
    pub enabled: bool,
    pub provider: String,
    pub devices: Vec<String>,
    pub note: String,
}

/// 一次资源采样快照
#[derive(Debug, Clone, Serialize)]
pub struct ResourceUsageSnapshot {
    pub components: Vec<ComponentUsage>,
    pub gpu: GpuUsage,
    pub network_note: String,
    pub system_cpu_percent: f32,
    pub system_memory_used: u64,
    pub system_memory_total: u64,
    pub captured_at: u64,
    pub sample_interval_ms: u64,
}

/// 本系统拥有的服务 PID（供定点匹配），不拉起、不杀死任何进程
struct KnownPids {
    app: u32,
    vcr: Option<u32>,
    ai: Option<u32>,
}

fn known_pids(app: &tauri::AppHandle) -> KnownPids {
    KnownPids {
        app: std::process::id(),
        vcr: vision::current_vcr_pid(app),
        ai: ai_service::current_ai_pid(),
    }
}

/// 构造固定 5 类组件的初始占位（running=false）
fn seed_components() -> Vec<ComponentUsage> {
    vec![
        ComponentUsage {
            key: "app".into(),
            name: "本地相册搭子（主程序）".into(),
            kind: ComponentKind::App,
            running: true, // 主程序必然在跑
            pid: Some(std::process::id()),
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_percent: 0.0,
            can_start: false,
            can_stop: false,
            note: "应用自身，不可在面板内停止".into(),
        },
        ComponentUsage {
            key: "vcr".into(),
            name: "视觉识别服务（VCR）".into(),
            kind: ComponentKind::Vcr,
            running: false,
            pid: None,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_percent: 0.0,
            can_start: true,
            can_stop: true,
            note: "扫描 / 分类 / 人物识别依赖；未运行时按需拉起".into(),
        },
        ComponentUsage {
            key: "ai_studio".into(),
            name: "AI 创作服务".into(),
            kind: ComponentKind::AiStudio,
            running: false,
            pid: None,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_percent: 0.0,
            can_start: true,
            can_stop: true,
            note: "创意工坊 AI 出片依赖；未运行时按需拉起".into(),
        },
        ComponentUsage {
            key: "gateway".into(),
            name: "本地 AI 修图网关（8000）".into(),
            kind: ComponentKind::Gateway,
            running: false,
            pid: None,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_percent: 0.0,
            can_start: false,
            can_stop: false,
            note: "外部程序，由用户手动启动；App 仅作客户端，不拥有启停权".into(),
        },
        ComponentUsage {
            key: "sd_server".into(),
            name: "图像生成服务 sd-server（1234）".into(),
            kind: ComponentKind::SdServer,
            running: false,
            pid: None,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_percent: 0.0,
            can_start: false,
            can_stop: false,
            note: "网关上游，外部程序；App 仅作客户端，不拥有启停权".into(),
        },
    ]
}

/// 按已知 PID 与进程特征，把采样值写回对应组件（阻塞函数，须在 spawn_blocking 内调用）
fn collect_components(known: &KnownPids) -> (Vec<ComponentUsage>, f32, u64, u64) {
    let mut guard = SAMPLER.lock().unwrap_or_else(|e| e.into_inner());
    let sys = guard.get_or_insert_with(System::new);
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let total_mem = sys.total_memory();
    let used_mem = sys.used_memory();
    let sys_cpu = sys.global_cpu_usage();

    // 固定 5 类组件，key → 下标用纯函数映射，避免持有 comps 的借用
    fn key_index(k: &str) -> usize {
        match k {
            "app" => 0,
            "vcr" => 1,
            "ai_studio" => 2,
            "gateway" => 3,
            "sd_server" => 4,
            _ => usize::MAX,
        }
    }

    let mut comps = seed_components();

    for (pid, p) in sys.processes() {
        let pid_u = pid.as_u32();
        let name = p.name().to_string_lossy().to_string();
        let cmd = p
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");

        // 一个进程只归一类：精确 PID 优先，其次进程特征
        let key: Option<&str> = if pid_u == known.app {
            Some("app")
        } else if known.vcr == Some(pid_u) {
            Some("vcr")
        } else if known.ai == Some(pid_u) {
            Some("ai_studio")
        } else if name.eq_ignore_ascii_case("sd-server.exe") {
            Some("sd_server")
        } else if cmd.to_ascii_lowercase().contains("openai_gateway") {
            Some("gateway")
        } else {
            None
        };

        let idx = match key {
            Some(k) => key_index(k),
            None => usize::MAX,
        };
        if idx < comps.len() {
            let mem = p.memory();
            let c = &mut comps[idx];
            c.running = true;
            c.pid = Some(pid_u);
            c.cpu_percent = p.cpu_usage();
            c.memory_bytes = mem;
            c.memory_percent = if total_mem > 0 {
                mem as f32 * 100.0 / total_mem as f32
            } else {
                0.0
            };
        }
    }

    (comps, sys_cpu, used_mem, total_mem)
}

/// GPU 配置态：仅当识别服务已在跑时才读 /gpu，避免为了「查看」而拉起服务
async fn gpu_status_readonly(app: &tauri::AppHandle) -> GpuUsage {
    if vision::current_vcr_pid(app).is_none() {
        return GpuUsage {
            enabled: false,
            provider: String::new(),
            devices: vec![],
            note: "识别服务未启动（无 GPU 配置态）".to_string(),
        };
    }
    match tokio::time::timeout(Duration::from_secs(4), vision::vcr_gpu_status(app)).await {
        Ok(Ok(s)) => GpuUsage {
            enabled: s.use_gpu,
            provider: s.provider,
            devices: s.gpu,
            note: "配置态（来自识别服务 /gpu，非实时利用率）".to_string(),
        },
        Ok(Err(e)) => GpuUsage {
            enabled: false,
            provider: String::new(),
            devices: vec![],
            note: format!("读取 GPU 状态失败：{e}"),
        },
        Err(_) => GpuUsage {
            enabled: false,
            provider: String::new(),
            devices: vec![],
            note: "读取 GPU 状态超时（服务可能正忙）".to_string(),
        },
    }
}

/// 采集本系统组件的资源快照（前端每 ~2s 轮询）
#[tauri::command]
pub async fn resource_usage_snapshot(app: tauri::AppHandle) -> Result<ResourceUsageSnapshot, String> {
    let _t = log_call!("resource_usage_snapshot");
    let app_bg = app.clone();
    let sampled = tauri::async_runtime::spawn_blocking(move || {
        let known = known_pids(&app_bg);
        collect_components(&known)
    })
    .await
    .map_err(|e| format!("资源采样执行失败: {e}"))?;

    let (components, system_cpu_percent, system_memory_used, system_memory_total) = sampled;

    let interval_ms = {
        let mut guard = LAST_SAMPLE.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let ms = guard.map(|t| now.duration_since(t).as_millis() as u64).unwrap_or(0);
        *guard = Some(now);
        ms
    };

    let gpu = gpu_status_readonly(&app).await;

    let snapshot = ResourceUsageSnapshot {
        components,
        gpu,
        network_note: "网络占用需按 PID 关联系统计数器（Windows PDH/NetIO），当前版本暂以占位展示"
            .to_string(),
        system_cpu_percent,
        system_memory_used,
        system_memory_total,
        captured_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        sample_interval_ms: interval_ms,
    };

    logger::log_call_end_with(
        "resource_usage_snapshot",
        _t,
        &format!(
            "OK | {} 组件 | 系统CPU {:.1}% | 内存 {}MB",
            snapshot.components.len(),
            snapshot.system_cpu_percent,
            snapshot.system_memory_used / 1024 / 1024
        ),
    );
    Ok(snapshot)
}

/// 启动 / 停止一个本系统服务（FEAT-179 内部启停）
///
/// key ∈ {"vcr", "ai_studio"}；action ∈ {"start", "stop"}。
/// 网关 / sd-server / 主程序由 Q1-A / Q3-A 决定不提供启停，这里直接拒绝。
#[tauri::command]
pub async fn service_control(
    app: tauri::AppHandle,
    key: String,
    action: String,
) -> Result<String, String> {
    let _t = log_call!("service_control", &format!("key={key} action={action}"));
    let result: Result<String, String> = match (key.as_str(), action.as_str()) {
        ("vcr", "start") => vision::start_for_monitor(&app)
            .await
            .map(|_| "视觉识别服务已启动".to_string()),
        ("vcr", "stop") => {
            let h = app.clone();
            tauri::async_runtime::spawn_blocking(move || vision::stop_for_monitor(&h))
                .await
                .map_err(|e| format!("停止识别服务失败: {e}"))?;
            Ok("视觉识别服务已停止".to_string())
        }
        ("ai_studio", "start") => ai_service::ensure_ready(&app)
            .await
            .map(|_| "AI 创作服务已启动".to_string()),
        ("ai_studio", "stop") => {
            tauri::async_runtime::spawn_blocking(ai_service::stop_for_monitor)
                .await
                .map_err(|e| format!("停止 AI 创作服务失败: {e}"))?;
            Ok("AI 创作服务已停止".to_string())
        }
        (_, a) if a != "start" && a != "stop" => Err(format!("不支持的操作: {a}")),
        _ => Err(format!("组件 {} 不支持在面板内启停", key)),
    };
    match &result {
        Ok(msg) => logger::log_call_end_with("service_control", _t, &format!("OK | {msg}")),
        Err(e) => logger::log_call_end_with("service_control", _t, &format!("ERR | {e}")),
    }
    result
}

/// 打开（或聚焦已存在的）「资源使用情况」副窗口（单例）
#[tauri::command]
pub async fn open_dev_resource_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("dev-resources") {
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || build_dev_resource_window(app))
        .await
        .map_err(|e| format!("资源窗口创建任务失败: {e}"))?
}

fn build_dev_resource_window(app: tauri::AppHandle) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let url = if tauri::is_dev() {
        tauri::WebviewUrl::External("http://localhost:1420/".parse().expect("valid dev url"))
    } else {
        tauri::WebviewUrl::App("index.html".into())
    };
    tauri::WebviewWindowBuilder::new(&app, "dev-resources", url)
        .title("开发者视角 · 资源使用情况")
        .inner_size(1080.0, 720.0)
        .min_inner_size(720.0, 460.0)
        .resizable(true)
        .center()
        .background_color(tauri::window::Color(12, 14, 20, 255))
        .build()
        .map_err(|e| format!("打开资源窗口失败: {e}"))?;
    logger::log_info(&format!(
        "打开资源使用情况窗口耗时 {}ms（后台线程建窗）",
        t0.elapsed().as_millis()
    ));
    Ok(())
}
