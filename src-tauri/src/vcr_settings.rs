//! VCR 识别服务设置与基准命令（自 lib.rs 迁入，lib.rs 瘦身第二期）
//! ===================================================================
//! 职责：GPU 开关、模型档位、CPU 线程数、模型源管理、基准测试。
//! 逻辑均在 vision.rs / model_dl.rs，本文件只是命令层。

#[tauri::command]
pub async fn get_vcr_gpu_status(app: tauri::AppHandle) -> Result<crate::vision::VcrGpuStatus, String> {
    let _t = log_call!("get_vcr_gpu_status");
    let r = crate::vision::vcr_gpu_status(&app).await;
    match &r {
        Ok(s) => crate::logger::log_call_end_with(
            "get_vcr_gpu_status",
            _t,
            &format!("OK | {}", crate::vision::gpu_brief(s)),
        ),
        Err(e) => crate::logger::log_call_end_with("get_vcr_gpu_status", _t, &format!("ERR | {e}")),
    }
    r
}


/// FEAT-051：GPU 加速开关（开 = GPU 优先 / 关 = 强制 CPU），返回切换后状态
#[tauri::command]
pub async fn set_vcr_gpu(enabled: bool, app: tauri::AppHandle) -> Result<crate::vision::VcrGpuStatus, String> {
    let _t = log_call!("set_vcr_gpu", &format!("enabled={enabled}"));
    let r = crate::vision::vcr_set_gpu(&app, enabled).await;
    match &r {
        Ok(s) => crate::logger::log_call_end_with(
            "set_vcr_gpu",
            _t,
            &format!("OK | {}", crate::vision::gpu_brief(s)),
        ),
        Err(e) => crate::logger::log_call_end_with("set_vcr_gpu", _t, &format!("ERR | {e}")),
    }
    r
}


/// FEAT-051：分类模型候选清单（含是否已下载 / 当前生效）
#[tauri::command]
pub async fn list_vcr_models(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let _t = log_call!("list_vcr_models");
    let r = crate::vision::vcr_list_models(&app).await;
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "list_vcr_models",
            _t,
            &format!("OK | {}", crate::vision::models_brief(v)),
        ),
        Err(e) => crate::logger::log_call_end_with("list_vcr_models", _t, &format!("ERR | {e}")),
    }
    r
}


/// FEAT-051：切换分类模型（未下载/未知名称返回服务端错误信息）
#[tauri::command]
pub async fn set_vcr_model(model: String, app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let _t = log_call!("set_vcr_model", &format!("model={model}"));
    let r = crate::vision::vcr_set_model(&app, &model).await;
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "set_vcr_model",
            _t,
            &format!("OK | {}", crate::vision::models_brief(v)),
        ),
        Err(e) => crate::logger::log_call_end_with("set_vcr_model", _t, &format!("ERR | {e}")),
    }
    r
}


/// v6：CPU 线程数现状（性能设置展示）
#[tauri::command]
pub async fn get_vcr_threads(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let _t = log_call!("get_vcr_threads");
    let r = crate::vision::vcr_threads_status(&app).await;
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "get_vcr_threads",
            _t,
            &format!("OK | {}", crate::vision::threads_brief(v)),
        ),
        Err(e) => crate::logger::log_call_end_with("get_vcr_threads", _t, &format!("ERR | {e}")),
    }
    r
}


/// v6：设置 CPU 线程数（适配不同硬件；服务端会后台重建会话）
#[tauri::command]
pub async fn set_vcr_threads(threads: i64, app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let _t = log_call!("set_vcr_threads", &format!("threads={threads}"));
    let r = crate::vision::vcr_set_threads(&app, threads).await;
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "set_vcr_threads",
            _t,
            &format!("OK | {}", crate::vision::threads_brief(v)),
        ),
        Err(e) => crate::logger::log_call_end_with("set_vcr_threads", _t, &format!("ERR | {e}")),
    }
    r
}


/// v6：线程数扫档（UI「对比测速」一键测得最优线程数）
#[tauri::command]
pub async fn benchmark_vcr_sweep(
    channel: Option<String>,
    options: Option<Vec<i64>>,
    runs: Option<u32>,
    warmup: Option<u32>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let channel = channel.unwrap_or_else(|| "clip_vision".into());
    let options = options.unwrap_or_default();
    let runs = runs.unwrap_or(8);
    let warmup = warmup.unwrap_or(2);
    let _t = log_call!(
        "benchmark_vcr_sweep",
        &format!("channel={channel} options={options:?} runs={runs} warmup={warmup}")
    );
    let r = crate::vision::vcr_benchmark_sweep(&app, &channel, options, runs, warmup).await;
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "benchmark_vcr_sweep",
            _t,
            &format!("OK | {}", crate::vision::sweep_brief(v)),
        ),
        Err(e) => crate::logger::log_call_end_with("benchmark_vcr_sweep", _t, &format!("ERR | {e}")),
    }
    r
}


/// FEAT-053：固定张量测速（CPU/GPU 真实加速比一键对比；channel 默认 det）
#[tauri::command]
pub async fn benchmark_vcr(
    runs: Option<u32>,
    warmup: Option<u32>,
    channel: Option<String>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let runs = runs.unwrap_or(10);
    let warmup = warmup.unwrap_or(2);
    let channel = channel.unwrap_or_else(|| "det".into());
    let _t = log_call!(
        "benchmark_vcr",
        &format!("channel={channel} runs={runs} warmup={warmup}")
    );
    let r = crate::vision::vcr_benchmark(&app, runs, warmup, &channel).await;
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "benchmark_vcr",
            _t,
            &format!("OK | {}", crate::vision::bench_brief(v)),
        ),
        Err(e) => crate::logger::log_call_end_with("benchmark_vcr", _t, &format!("ERR | {e}")),
    }
    r
}
