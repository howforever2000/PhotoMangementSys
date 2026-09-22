//! VCR 识别服务设置与基准命令（自 lib.rs 迁入，lib.rs 瘦身第二期）
//! ===================================================================
//! 职责：GPU 开关、模型档位、CPU 线程数、模型源管理、基准测试。
//! 逻辑均在 vision.rs / model_dl.rs，本文件只是命令层。
//!
//! FEAT-064：另含「相册扫描分组工具」的**文件扫描并行度**命令
//! （`get_scan_perf` / `set_scan_perf`）—— 与 VCR 的推理线程数是两套独立设置：
//! 前者是 EXIF 文件扫描（IO 密集，受盘型约束），后者是 ONNX 推理（纯计算）。

/// FEAT-064：读取文件扫描性能画像（CPU 拓扑 + 磁盘类型 + 推荐线程数）
#[tauri::command]
pub async fn get_scan_perf(app: tauri::AppHandle) -> Result<crate::scan_perf::CpuTopology, String> {
    let _t = log_call!("get_scan_perf");
    let topo = crate::scan_perf::topology(&app);
    crate::logger::log_call_end_with(
        "get_scan_perf",
        _t,
        &format!(
            "OK | logical={} physical={} disk={} rec={} eff={}",
            topo.logical, topo.physical, topo.disk_kind, topo.recommended, topo.effective
        ),
    );
    Ok(topo)
}

/// FEAT-064：设置文件扫描线程数（None = 恢复跟随推荐）；返回更新后的画像
#[tauri::command]
pub async fn set_scan_perf(
    threads: Option<usize>,
    app: tauri::AppHandle,
) -> Result<crate::scan_perf::CpuTopology, String> {
    let _t = log_call!("set_scan_perf", &format!("threads={threads:?}"));
    let saved = crate::scan_perf::save_threads(&app, threads)?;
    let topo = crate::scan_perf::topology(&app);
    crate::logger::log_call_end_with(
        "set_scan_perf",
        _t,
        &format!("OK | 生效线程数={saved}（推荐 {}）", topo.recommended),
    );
    Ok(topo)
}

/// FEAT-064：扫描性能实测校准 —— 用同一批真实文件在若干线程档位下实测，
/// 返回「最快档」，供 UI 一键采用
///
/// 为什么需要实测：推荐值来自拓扑推断（物理核 + 超线程折扣 + 盘型约束），
/// 但真实吞吐还受 SSD/HDD 具体型号、CPU 大小核调度、杀软实时扫描等影响，
/// 只有实际跑一遍才准。取前 N 个文件重复跑，避免整套相册扫多遍（太慢）。
#[tauri::command]
pub async fn calibrate_scan_threads(
    path: String,
    recurse: bool,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    use std::time::Instant;
    let _t = log_call!("calibrate_scan_threads", &format!("path={path} recurse={recurse}"));

    let topo = crate::scan_perf::topology(&app);

    let r: Result<serde_json::Value, String> = tauri::async_runtime::spawn_blocking(move || {
        let files = super::test_scan::collect_image_files(&path, recurse)?;
        if files.is_empty() {
            return Err("该目录没有可扫描的图片，无法实测".to_string());
        }

        // ---- 相册画像：样本选择要能代表「这个相册」的真实负载 ----
        // 只按文件名取前 200 张会撞上「同一次连拍、大小雷同」的偏样本，
        // 按文件大小分层抽样（大/中/小各取一段）更能反映真实吞吐。
        let mut by_size: Vec<(usize, usize, &std::path::PathBuf, &String)> = files
            .iter()
            .enumerate()
            .filter_map(|(i, (p, n))| {
                std::fs::metadata(p).ok().map(|m| (m.len() as usize, i, p, n))
            })
            .collect();
        by_size.sort_by_key(|(sz, _, _, _)| *sz);
        // 平均文件大小（决定单文件耗时量级：大图解码久、小图重在打开开销）
        let avg_bytes = if by_size.is_empty() {
            0
        } else {
            by_size.iter().map(|(s, _, _, _)| *s).sum::<usize>() / by_size.len()
        };
        let big = by_size.iter().rev().take(70); // 最大的 70 张
        let mid_start = by_size.len().saturating_sub(70) / 2;
        let mid = by_size.iter().skip(mid_start).take(70); // 中段 70 张
        let small = by_size.iter().take(70); // 最小的 70 张
        let sample: Vec<(usize, &std::path::PathBuf, &String)> = big
            .chain(mid)
            .chain(small)
            .map(|(_, i, p, n)| (*i, *p, *n))
            .collect();
        let sample: Vec<(usize, &std::path::PathBuf, &String)> =
            if sample.is_empty() {
                files.iter().enumerate().map(|(i, (p, n))| (i, p, n)).collect()
            } else {
                sample
            };

        // ---- 档位设计：覆盖「太少 / 推荐 / 过多」三个区间，才能看出拐点 ----
        // 关键点：并发上限受逻辑核约束（超出的线程只会互相抢），但 IO 等待
        // 允许一定超配，故上限取 logical + logical/2。
        let hard_cap = (topo.logical + topo.logical / 2).min(crate::scan_perf::SCAN_THREADS_MAX);
        let mut opts: Vec<usize> = vec![
            1,
            2,
            (topo.physical / 2).max(1),
            topo.physical,
            topo.recommended,
        ];
        // 在推荐值以上再探 2 档（找拐点：超过它吞吐开始下降）
        if topo.recommended < hard_cap {
            opts.push((topo.recommended + topo.recommended / 2).min(hard_cap));
            opts.push(hard_cap);
        }
        opts.retain(|n| *n >= 1 && *n <= hard_cap);
        opts.sort_unstable();
        opts.dedup();

        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut measured: Vec<(usize, f64)> = Vec::new(); // (threads, 张/秒)
        for n in &opts {
            let pool = match rayon::ThreadPoolBuilder::new().num_threads(*n).build() {
                Ok(p) => p,
                Err(e) => {
                    results.push(serde_json::json!({
                        "threads": n, "error": format!("线程池创建失败: {e}")
                    }));
                    continue;
                }
            };
            // 预热 1 轮（首次要落磁盘页缓存，不预热会把 1 线程档测得虚低）
            let _ = pool.install(|| {
                use rayon::prelude::*;
                sample
                    .par_iter()
                    .map(|(_, p, name)| super::test_scan::read_photo_exif_pub(p, name))
                    .count()
            });
            // 正式测 2 轮取**最小值**（干扰最少的那个样本最能代表真实速度）
            let mut best_secs = f64::MAX;
            let mut count = 0usize;
            for _ in 0..2 {
                let t0 = Instant::now();
                count = pool.install(|| {
                    use rayon::prelude::*;
                    sample
                        .par_iter()
                        .map(|(_, p, name)| {
                            let _ = super::test_scan::read_photo_exif_pub(p, name);
                            1usize
                        })
                        .sum::<usize>()
                });
                best_secs = best_secs.min(t0.elapsed().as_secs_f64());
            }
            let rate = if best_secs > 0.0 { count as f64 / best_secs } else { 0.0 };
            measured.push((*n, rate));
            results.push(serde_json::json!({
                "threads": n,
                "count": count,
                "ms": (best_secs * 1000.0).round() as u64,
                "per_sec": (rate * 10.0).round() / 10.0,
                "per_file_ms": if count > 0 { ((best_secs * 1000.0) / count as f64 * 100.0).round() / 100.0 } else { 0.0 },
            }));
            crate::logger::log_info(&format!(
                "[scan-calib] {n} 线程 → {} 张 / {:.0}ms（{:.0} 张/秒）",
                count,
                best_secs * 1000.0,
                rate
            ));
        }
        if measured.is_empty() {
            return Err("所有档位实测均失败".to_string());
        }

        // ---- 推算最优档：不取「峰值」而取「达到峰值 95% 的最小线程数」----
        // 理由：峰值档常比次优档快不到 5%，但线程数高一大截 —— 多占 CPU 换来的
        // 边际收益很低，还会拖慢用户同时做的其他事。取「够用即可」的档更实用。
        let peak = measured.iter().cloned().fold((0usize, 0.0), |a, b| {
            if b.1 > a.1 { b } else { a }
        });
        let threshold = peak.1 * 0.95;
        let optimal = measured
            .iter()
            .filter(|(_, r)| *r >= threshold)
            .min_by_key(|(n, _)| *n)
            .map(|(n, _)| *n)
            .unwrap_or(peak.0);
        let opt_rate = measured
            .iter()
            .find(|(n, _)| *n == optimal)
            .map(|(_, r)| *r)
            .unwrap_or(peak.1);

        // 相对单线程的提速比（用户最直观的收益指标）
        let base_rate = measured
            .iter()
            .find(|(n, _)| *n == 1)
            .map(|(_, r)| *r)
            .unwrap_or(0.0);
        let speedup = if base_rate > 0.0 { opt_rate / base_rate } else { 0.0 };

        // ---- 推荐理由（前端直接展示，让用户知道"为什么是这个数"）----
        let mut notes: Vec<String> = Vec::new();
        notes.push(format!(
            "样本 {} 张（按文件大小分层抽样，平均 {:.1}MB）",
            sample.len(),
            avg_bytes as f64 / 1e6
        ));
        notes.push(format!("机检推荐 {} 线程", topo.recommended));
        if optimal == topo.recommended {
            notes.push("实测与机检推荐一致".into());
        } else if optimal > topo.recommended {
            notes.push(format!("实测在本相册上更多线程仍有效（+{}）", optimal - topo.recommended));
        } else {
            notes.push(format!(
                "实测比机检推荐更少线程即达最优（{} < {}），本相册文件偏{}",
                optimal,
                topo.recommended,
                if avg_bytes > 4_000_000 { "大" } else { "小" }
            ));
        }
        if peak.0 != optimal {
            notes.push(format!(
                "峰值在 {} 线程（{} 张/秒），但 {} 线程已达其 {:.0}%，性价比更高",
                peak.0,
                (peak.1 * 10.0).round() / 10.0,
                optimal,
                if peak.1 > 0.0 { opt_rate / peak.1 * 100.0 } else { 100.0 }
            ));
        }
        if speedup > 1.05 {
            notes.push(format!("比单线程快 {speedup:.1}×"));
        }

        Ok(serde_json::json!({
            "sample": sample.len(),
            "total": files.len(),
            "avg_bytes": avg_bytes,
            "best_threads": optimal,        // 「最优」= 达峰值 95% 的最小档
            "peak_threads": peak.0,         // 实测绝对峰值档
            "best_per_sec": (opt_rate * 10.0).round() / 10.0,
            "peak_per_sec": (peak.1 * 10.0).round() / 10.0,
            "speedup": (speedup * 100.0).round() / 100.0,
            "recommended": topo.recommended,
            "logical": topo.logical,
            "physical": topo.physical,
            "disk_kind": topo.disk_kind,
            "reason": notes.join("；"),
            "results": results,
        }))
    })
    .await
    .map_err(|e| format!("实测线程失败: {e}"))?;

    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "calibrate_scan_threads",
            _t,
            &format!(
                "OK | 最优 {} 线程（峰值 {} 线程 · {} 张/秒）",
                v.get("best_threads").and_then(|x| x.as_u64()).unwrap_or(0),
                v.get("peak_threads").and_then(|x| x.as_u64()).unwrap_or(0),
                v.get("best_per_sec").and_then(|x| x.as_f64()).unwrap_or(0.0)
            ),
        ),
        Err(e) => crate::logger::log_call_end_with("calibrate_scan_threads", _t, &format!("ERR | {e}")),
    }
    r
}

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
