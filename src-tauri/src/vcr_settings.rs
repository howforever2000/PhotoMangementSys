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

/// P23：批次大小标定 —— 实测「一次请求带几张图」在本机最快。
///
/// 为什么需要实测（而不是沿用旧结论「批次无影响」）：
/// 日志里分批耗时呈清晰的 `A + B/n` 曲线 —— A ≈ 43ms/张（单图解码+推理）
/// 加 B ≈ 127ms/次请求（HTTP 往返 + Python 侧调用开销，**与张数无关**）。用这组参数
/// 回验已有样本全部对上：n=4 预测 74.8 vs 实测 76.8、n=6 预测 64.2 vs 65.2、
/// n=8 预测 58.9 vs 59.1。按曲线推算批 32约 47ms/张，比默认批 8 省约 20%。
///
/// 但那些小批样本（n<8）每档只有 1~6 条，且全部来自**每轮扫描末尾的余数批**
/// （412 ÷ 8 = 52 批还剩 4 张），可能是尾部效应而非真实规律 —— 相关性不足以定论。
/// 本命令用**同一批样本 + 交错轮转**把曲线真测一遍。
///
/// 方法学与 `calibrate_scan_threads` 对齐：
/// - 样本按文件大小 stride 抽样，避免「同一次连拍、大小雷同」的偏样本；
/// - 每档先预热 1 轮（页缓存 + 模型热身），再正式跑 2 轮**取最小值**
///   （干扰最少的那个样本最能代表真实速度）；
/// - 档位间顺序每轮**轮转**，抵消热漂移/后台负载随时间变化的偏差。
///
/// 「最优」取**达峰 95% 的最小批次**（与线程同口径）：批越大越省时间是曲线趋势，
/// 但批次同时是**内存上限**，取拐点而非峰值，多出来的那点吞吐不值得多占内存。
#[tauri::command]
pub async fn calibrate_scan_batch(
    path: Option<String>,
    recurse: bool,
    force: bool,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    use std::time::Instant;
    use tauri::Emitter; // app.emit 需要该 trait 在作用域内

    const LEVELS: [usize; 5] = [4, 8, 16, 32, 64];
    const DEFAULT_BATCH: usize = 8;
    /// 样本量取 64：让最大档（64）也能得到一个满批请求，否则最大档只有零散尾批
    const SAMPLE_TARGET: usize = 64;
    const ROUNDS: usize = 2;
    /// 达峰 95% 即视为「够快」，在够快的档里取最小的
    const GOOD_ENOUGH: f64 = 0.95;

    let _t = log_call!(
        "calibrate_scan_batch",
        &format!("path={path:?} recurse={recurse} force={force}")
    );

    // 未强制 → 有新鲜结论就直接复用（与线程标定的 `ensure_scan_calibration` 同语义：
    // 30 天内不重复占 CPU，换环境/要看新数据时才用 force）
    if !force {
        if let Some(cached) = load_batch_calibration(&app) {
            let age = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .saturating_sub(cached.get("updated_at").and_then(|v| v.as_u64()).unwrap_or(0));
            if age < 30 * 24 * 3600 {
                crate::logger::log_info(&format!(
                    "[scan-calib] 批次标定复用 {age}s 前的结论（未传 force），最优批 {}",
                    cached.get("optimal_batch").and_then(|v| v.as_u64()).unwrap_or(0)
                ));
                let mut v = cached;
                v["cached"] = serde_json::json!(true);
                return Ok(v);
            }
        }
    }

    // 1. 取样：与线程标定同口径（分层，不取前 N 张）
    //
    // **优先用缩略图缓存**：真实扫描送进 /embed_batch 的就是缩略图
    //（`scan_album_embeddings` 的 `thumb_paths`），用它标定才测得准真实负载，
    // 且不需要用户先选文件夹（高级选项区拿不到 dir prop）。
    let dir = match path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => {
            let d = crate::thumbs_dir(&app)
                .map_err(|e| format!("无法定位缩略图目录：{e}"))?
                .join("grid");
            if !d.is_dir() {
                return Err("缩略图缓存为空（尚未扫描过任何相册），无法取样。请先扫描一次，或指定 path。".to_string());
            }
            d.to_string_lossy().into_owned()
        }
    };
    let files = super::test_scan::collect_image_files(&dir, recurse)?;
    if files.is_empty() {
        return Err(format!("目录里没有可编码的图片，无法实测：{dir}"));
    }
    // 按文件大小排序后等距抽样 —— 跨度覆盖大/中/小，又不引入连拍偏样本
    let by_size: Vec<(u64, &std::path::PathBuf)> = {
        let mut v: Vec<(u64, &std::path::PathBuf)> = files
            .iter()
            .filter_map(|(p, _)| std::fs::metadata(p).ok().map(|m| (m.len(), p)))
            .collect();
        v.sort_by_key(|(sz, _)| *sz);
        v
    };
    let stride = (by_size.len() / SAMPLE_TARGET).max(1);
    let sample: Vec<&std::path::PathBuf> =
        by_size.iter().step_by(stride).take(SAMPLE_TARGET).map(|(_, p)| *p).collect();
    let paths: Vec<String> = sample
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let n_img = paths.len();
    let avg_bytes = if by_size.is_empty() {
        0
    } else {
        by_size.iter().map(|(s, _)| *s as usize).sum::<usize>() / by_size.len()
    };

    // 档位：只测「样本量容得下」的档（否则最大档只有 1 个请求，测不出东西）
    let levels: Vec<usize> = LEVELS.iter().copied().filter(|l| *l <= n_img).collect();
    if levels.len() < 2 {
        return Err(format!("样本只有 {n_img} 张，不足 2 个可测档位（需 ≥8 张）"));
    }

    crate::logger::log_info(&format!(
        "[scan-calib] 批次标定开始 | 样本 {n_img} 张（全库 {} 张，平均 {:.1}MB）· 档位 {:?} · 每档预热1+正式{ROUNDS}轮",
        files.len(),
        avg_bytes as f64 / 1e6,
        levels,
    ));
    // 估算总编码次数，供前端展示「约需 N 秒」
    let est_rounds = levels.len() as u64 * (ROUNDS as u64 + 1);
    let est_secs = (est_rounds * n_img as u64 * 60 / 1000).max(1);

    let mut measured: Vec<(usize, f64)> = Vec::new(); // (批次, ms/张)
    let mut rows: Vec<serde_json::Value> = Vec::new();
    // 轮转偏移**只取一次**：若每轮重算，同一轮内各次取值不同，置换语义就碎了
    let rot = level_rotation();

    for li in 0..levels.len() {
        // 档位顺序每轮轮转 —— 抵消「越跑越热/后台负载变化」带来的系统性偏差
        let level = levels[(li + rot) % levels.len()];
        if measured.iter().any(|(l, _)| *l == level) {
            continue;
        }
        let t_all = Instant::now();

        // 预热 1 轮：首跑要把源文件读进页缓存、模型首次前向，不预热会把该档测得虚高
        if let Err(e) = crate::vision::embed_images_batch(&paths, level, &app, None).await {
            rows.push(serde_json::json!({ "batch": level, "error": e }));
            crate::logger::log_info(&format!("[scan-calib] 批{level} 预热失败: {e}"));
            continue;
        }

        let mut best_secs = f64::MAX;
        let mut ok_best = 0usize;
        let mut err_last = String::new();
        for _ in 0..ROUNDS {
            let t = Instant::now();
            match crate::vision::embed_images_batch(&paths, level, &app, None).await {
                Ok(rs) => {
                    let el = t.elapsed().as_secs_f64();
                    let ok = rs.iter().filter(|r| r.embedding.is_some()).count();
                    if ok == 0 {
                        err_last = "本档 0 张编码成功（服务或模型异常）".into();
                        break;
                    }
                    if el < best_secs {
                        best_secs = el;
                        ok_best = ok;
                    }
                }
                Err(e) => {
                    err_last = e;
                    break;
                }
            }
        }
        if best_secs.is_finite() && best_secs > 0.0 && ok_best > 0 {
            let ms = best_secs * 1000.0 / n_img as f64;
            measured.push((level, ms));
            rows.push(serde_json::json!({
                "batch": level,
                "requests": n_img.div_ceil(level),
                "ms_per_photo": (ms * 100.0).round() / 100.0,
                "total_ms": (best_secs * 1000.0).round() as u64,
                "encoded": ok_best,
            }));
            crate::logger::log_info(&format!(
                "[scan-calib] 批{level} → {n_img} 张 / {:.0}ms（{ms:.1}ms/张 · 每次请求 {} 张）",
                best_secs * 1000.0,
                n_img.div_ceil(level)
            ));
        } else if !err_last.is_empty() {
            rows.push(serde_json::json!({ "batch": level, "error": err_last }));
        }

        let _ = app.emit(
            "scan-calib-progress",
            serde_json::json!({
                "kind": "batch",
                "done": li + 1,
                "total": levels.len(),
                "batch": level,
                "elapsed_ms": t_all.elapsed().as_millis(),
                "est_secs": est_secs,
            }),
        );
    }

    if measured.is_empty() {
        return Err("所有批次档位实测均失败".to_string());
    }

    // 2. 拟合：A + B/n —— 单图成本 + 每请求固定开销。仅用于给用户看「规律」，不参与选档。
    let (fit_a, fit_b) = fit_ab_curve(&measured);

    // 3. 选档：**达峰 95% 的最小批次**（峰值 = ms/张 最小的那档）
    let peak_ms = measured.iter().map(|(_, ms)| *ms).fold(f64::MAX, f64::min);
    let threshold = peak_ms / GOOD_ENOUGH;
    let optimal = match pick_optimal_batch(&measured) {
        Some(n) => n,
        None => return Err("实测数据无效，无法选档".to_string()),
    };
    let peak = measured
        .iter()
        .cloned()
        .fold((0usize, f64::MAX), |a, b| if b.1 < a.1 { b } else { a });

    // 4. 相对默认档（8）的收益 —— 前端据此判「值不值得换」
    let base_ms = measured
        .iter()
        .find(|(l, _)| *l == DEFAULT_BATCH)
        .map(|(_, ms)| *ms);
    let best_ms = peak.1;
    let gain = match base_ms {
        Some(b) if b > 0.0 && best_ms > 0.0 && b > best_ms => (b - best_ms) / b,
        _ => 0.0,
    };
    let insignificant = gain < 0.05; // 不到 5% 视作「无显著差异」，不建议折腾

    let mut notes: Vec<String> = Vec::new();
    notes.push(format!(
        "样本 {n_img} 张（全库 {} 张按大小等距抽样，平均 {:.1}MB）",
        files.len(),
        avg_bytes as f64 / 1e6
    ));
    notes.push(format!(
        "拟合 cost ≈ {fit_a:.0} + {fit_b:.0}/n ms（单图成本 + 每请求固定开销）"
    ));
    if match base_ms {
        Some(b) => optimal == DEFAULT_BATCH && b <= threshold,
        None => false,
    } {
        notes.push(format!("实测确认默认档 {DEFAULT_BATCH} 已在达峰区间，无需调整"));
    } else if gain < 0.05 {
        notes.push(format!(
            "各档差异不足 5%（最省 {} 与默认 {DEFAULT_BATCH} 相差 {:.1}%）—— 批次对本机无显著影响",
            optimal,
            base_ms.map(|b| (b - best_ms) / b * 100.0).unwrap_or(0.0)
        ));
    } else {
        notes.push(format!(
            "批 {optimal} 比默认 {DEFAULT_BATCH} 快 {gain:.0}%（{} → {best_ms:.1} ms/张）",
            base_ms.unwrap_or(best_ms)
        ));
    }
    if optimal != peak.0 {
        notes.push(format!(
            "峰值在批 {}（{peak_ms:.1} ms/张），但批 {optimal} 已达其 {:.0}%，省内存",
            peak.0,
            peak_ms / threshold * 100.0 * GOOD_ENOUGH
        ));
    }
    notes.push("批次只作用于语义向量通道；人物/文档识别是逐张处理，不受影响".into());

    let report = serde_json::json!({
        "sample": n_img,
        "total": files.len(),
        "avg_bytes": avg_bytes,
        "levels": levels,
        "results": rows,
        "optimal_batch": optimal,
        "peak_batch": peak.0,
        "peak_ms": (peak_ms * 100.0).round() / 100.0,
        "default_batch": DEFAULT_BATCH,
        "default_ms": base_ms.map(|m| (m * 100.0).round() / 100.0),
        "gain": (gain * 100.0).round() / 100.0,
        "insignificant": insignificant,
        "fit_a": (fit_a * 100.0).round() / 100.0,
        "fit_b": (fit_b * 100.0).round() / 100.0,
        "est_secs": est_secs,
        "reason": notes.join("；"),
    });

    // 5. 落盘 —— 让「批次推荐」不再是空壳（UI 徽标可从「默认值」变「本机实测」）
    if let Err(e) = save_batch_calibration(&app, &report, &dir) {
        crate::logger::log_info(&format!("[scan-calib] 批次实测结果落盘失败（不影响本次结果）：{e}"));
    }

    crate::logger::log_call_end_with(
        "calibrate_scan_batch",
        _t,
        &format!(
            "OK | 最优批 {optimal}（峰值批 {} · {peak_ms:.1}ms/张 · 相对默认 {DEFAULT_BATCH} 省 {gain:.0}%）",
            peak.0
        ),
    );
    Ok(report)
}

/// 档位轮转偏移：让不同次标定从不同档位起步，进一步消掉顺序偏差
fn level_rotation() -> usize {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as usize) % 5)
        .unwrap_or(0)
}

/// 选批次最优档：**达峰值 95% 的最小批次**（与线程标定同口径）。
///
/// 为什么取最小而不是峰值档：批次对吞吐是「越大越省」（每请求固定开销被摊薄），
/// 但对内存是「越大越贵」 —— 峰值档常比次优档快不到 5%，却要多吃一倍内存。
/// 「够用即可」的拐点才是性价比。
///
/// 入参 `(批次, ms/张)`（**越小越快**），返回其中达到最优值 95% 的最小批次。
/// 全空 / 全无效 → None。
fn pick_optimal_batch(measured: &[(usize, f64)]) -> Option<usize> {
    let valid: Vec<(usize, f64)> = measured
        .iter()
        .copied()
        .filter(|(n, ms)| *n > 0 && ms.is_finite() && *ms > 0.0)
        .collect();
    let best = valid.iter().map(|(_, ms)| *ms).fold(f64::MAX, f64::min);
    if !best.is_finite() {
        return None;
    }
    // ms 越小越快 → 「达峰 95%」即 ms ≤ best / 0.95（≈ best × 1.053）
    let threshold = best / 0.95;
    valid
        .iter()
        .filter(|(_, ms)| *ms <= threshold)
        .min_by_key(|(n, _)| *n)
        .map(|(n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拟合函数用**日志里的真实样本**验证 —— 这组数是 2026-09-28 album=63 等扫描
    /// 的 `[scan.batch] embed` 均摊耗时（批 1~8，其中 8 有 413 条样本）。
    /// 若拟合逻辑被改动，残差会立刻变大。
    #[test]
    fn fit_ab_curve_matches_real_log_samples() {
        let pts: Vec<(usize, f64)> = vec![
            (1, 170.5),
            (2, 111.0),
            (3, 83.0),
            (4, 76.8),
            (5, 69.2),
            (6, 65.2),
            (7, 61.9),
            (8, 59.1),
        ];
        let (a, b) = fit_ab_curve(&pts);
        // 独立最小二乘复算：A=43.7 / B=127.7
        assert!(a > 38.0 && a < 50.0, "单图成本 A 应落在 ~43.7，实际 {a:.1}");
        assert!(b > 110.0 && b < 145.0, "每请求固定开销 B 应落在 ~127.7，实际 {b:.1}");
        for (n, real) in &pts {
            let pred = a + b / *n as f64;
            let err = ((pred - real) / real).abs();
            assert!(err < 0.06, "批 {n} 预测 {pred:.1} 与实测 {real} 偏差 {:.1}% 超 6%", err * 100.0);
        }
        // 外推验证：批 32 应比批 8 省约 20%
        let gain_32 = 1.0 - (a + b / 32.0) / (a + b / 8.0);
        assert!(gain_32 > 0.15 && gain_32 < 0.25, "批32 相对批8 应省 ~20%，实际 {:.0}%", gain_32 * 100.0);
    }

    /// 选档规则：达峰 95% 的**最小**档，绝不选最大档（批次同时是内存上限）
    #[test]
    fn pick_optimal_batch_picks_knee_not_peak() {
        // 实测曲线（A=43.7, B=127.7）：批64 最快 45.7，门槛 = 45.7/0.95 ≈ 48.1
        // 批32 = 47.7 达标、批16 = 51.7 不达标 → 应选 32 而非 64
        let m: Vec<(usize, f64)> = vec![(4, 75.6), (8, 59.7), (16, 51.7), (32, 47.7), (64, 45.7)];
        assert_eq!(pick_optimal_batch(&m), Some(32), "应在达标档里取最小批次");
    }

    /// 曲线平坦（各档差异 <5%）→ 取最小档，与「无显著差异时保持默认」不矛盾
    #[test]
    fn pick_optimal_batch_on_flat_curve_picks_smallest() {
        let m: Vec<(usize, f64)> = vec![(4, 60.0), (8, 59.8), (16, 59.5), (32, 59.2), (64, 59.0)];
        assert_eq!(pick_optimal_batch(&m), Some(4), "平坦曲线取最小档");
    }

    /// 边界：空输入 / 全非法 / 非单调都不 panic
    #[test]
    fn pick_optimal_batch_handles_degenerate_input() {
        assert_eq!(pick_optimal_batch(&[]), None);
        assert_eq!(pick_optimal_batch(&[(8, 0.0), (16, f64::NAN)]), None);
        // 部分非法：只看合法项
        assert_eq!(pick_optimal_batch(&[(8, f64::NAN), (16, 50.0)]), Some(16));
        // 批次为 0 的项直接剔除（除零保护）
        assert_eq!(pick_optimal_batch(&[(0, 10.0), (4, 20.0)]), Some(4));
    }

    /// 轮转偏移必须稳定在 [0, 5)，否则越界会 panic
    #[test]
    fn level_rotation_stays_in_range() {
        for _ in 0..50 {
            let r = level_rotation();
            assert!((0..5).contains(&r), "轮转偏移越界: {r}");
        }
    }
}

/// 最小二乘拟合 `ms = A + B/n`（A=单图成本，B=每请求固定开销）。
/// 输入至少 2 档；只用于报告拟合规律，不参与选档。
fn fit_ab_curve(pts: &[(usize, f64)]) -> (f64, f64) {
    if pts.len() < 2 {
        return (pts.first().map(|(_, m)| *m).unwrap_or(0.0), 0.0);
    }
    // 以 x = 1/n 做线性拟合：ms = A + B·x
    let n = pts.len() as f64;
    let xs: Vec<f64> = pts.iter().map(|(l, _)| 1.0 / (*l as f64)).collect();
    let ys: Vec<f64> = pts.iter().map(|(_, m)| *m).collect();
    let sx: f64 = xs.iter().sum();
    let sy: f64 = ys.iter().sum();
    let sxx: f64 = xs.iter().map(|x| x * x).sum();
    let sxy: f64 = xs.iter().zip(ys.iter()).map(|(x, y)| x * y).sum();
    let denom = n * sxx - sx * sx;
    if denom.abs() < 1e-12 {
        return (sy / n, 0.0);
    }
    let b = (n * sxy - sx * sy) / denom;
    let a = (sy - b * sx) / n;
    (a.max(0.0), b.max(0.0))
}

/// 读回批次实测结果（无文件/解析失败 → None，调用方回退到「现场实测」）
fn load_batch_calibration(app: &tauri::AppHandle) -> Option<serde_json::Value> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().ok()?;
    let text = std::fs::read_to_string(dir.join("batch_calibration.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// 批次实测结果落盘（app_data_dir/batch_calibration.json，与线程设置分开存）
fn save_batch_calibration(
    app: &tauri::AppHandle,
    report: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录: {e}"))?;
    let file = dir.join("batch_calibration.json");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = serde_json::json!({
        "version": 1,
        "updated_at": now,
        "path": path,
        "optimal_batch": report.get("optimal_batch").cloned().unwrap_or(serde_json::json!(0)),
        "peak_batch": report.get("peak_batch").cloned().unwrap_or(serde_json::json!(0)),
        "gain": report.get("gain").cloned().unwrap_or(serde_json::json!(0.0)),
        "insignificant": report.get("insignificant").cloned().unwrap_or(serde_json::json!(false)),
        "sample": report.get("sample").cloned().unwrap_or(serde_json::json!(0)),
        "results": report.get("results").cloned().unwrap_or_else(|| serde_json::json!([])),
        "reason": report.get("reason").cloned().unwrap_or(serde_json::json!("")),
    });
    std::fs::write(&file, serde_json::to_string_pretty(&body).unwrap_or_default())
        .map_err(|e| format!("写入 {file:?} 失败: {e}"))?;
    Ok(())
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


/// 人脸模型档位清单（高精度 / 轻量）：供「性能设置」展示与切换
///
/// 与语义档位（list_vcr_models）同模式：调用失败即代表服务不可用，由前端提示。
#[tauri::command]
pub async fn get_face_tier_info(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let _t = log_call!("get_face_tier_info");
    let r = crate::vision::vcr_face_tier_info(&app).await;
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "get_face_tier_info",
            _t,
            &format!(
                "OK | current={}",
                v.get("current").and_then(|x| x.as_str()).unwrap_or("?")
            ),
        ),
        Err(e) => crate::logger::log_call_end_with("get_face_tier_info", _t, &format!("ERR | {e}")),
    }
    r
}


/// 切换人脸模型档位（precise / light）；选择由服务端持久化，重启后仍生效
#[tauri::command]
pub async fn set_face_tier(tier: String, app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let _t = log_call!("set_face_tier", &format!("tier={tier}"));
    let r = crate::vision::vcr_set_face_tier(&app, &tier).await;
    match &r {
        Ok(v) => crate::logger::log_call_end_with(
            "set_face_tier",
            _t,
            &format!(
                "OK | current={}",
                v.get("current").and_then(|x| x.as_str()).unwrap_or("?")
            ),
        ),
        Err(e) => crate::logger::log_call_end_with("set_face_tier", _t, &format!("ERR | {e}")),
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
