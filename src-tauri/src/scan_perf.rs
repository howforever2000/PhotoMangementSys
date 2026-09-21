//! 扫描性能：CPU 拓扑探测 + 扫描线程数推荐（FEAT-064）
//!
//! 需求背景：「相册扫描分组工具」是多线程处理的 CPU/IO 混合负载，线程数直接决定
//! 耗时；但**不同机器最优值差异很大**（老双核本 2 线程即满、16 核本 8~12 线程收益
//! 递减、机械盘上过多线程反而因随机寻道变慢）。因此：
//!   1. 探测本机真实拓扑（物理核 / 逻辑核 / 是否大小核混合 / 磁盘类型）
//!   2. 按「实际计算」给出推荐值 —— 不是简单 = 逻辑核，而是按负载特征折算
//!   3. 允许用户覆盖（落盘持久化），并提供「实测校准」用真实扫描量出最优档
//!
//! 与 Python VCR 侧 `vcr/config.py` 的线程数逻辑**刻意分离**：
//! 那边是 ONNX Runtime 的 intra_op 线程（纯计算，推荐物理核）；这边是文件 EXIF
//! 扫描（IO 密集，推荐值更高且受磁盘类型约束），两者最优档不同，共用会互相误导。

use std::path::Path;

use tauri::Manager;

/// 线程数上下限（与 UI 档位一致）
pub const SCAN_THREADS_MIN: usize = 1;
pub const SCAN_THREADS_MAX: usize = 32;

/// CPU 拓扑 + 负载画像（前端「性能设置」展示，也是推荐值的计算依据）
#[derive(Debug, Clone, serde::Serialize)]
pub struct CpuTopology {
    /// 逻辑核数（含超线程）
    pub logical: usize,
    /// 物理核数（真实并行单元）
    pub physical: usize,
    /// 大小核混合架构（Intel 12 代+ P/E 核：并行的调度效率低于同构核）
    pub hybrid: bool,
    /// 是否为混合/未知拓扑的推测值（true 时提示「估算」）
    pub estimated: bool,
    /// 启动盘/目标盘的介质类型（scan 负载强相关）
    pub disk_kind: String,
    /// 扫描负载推荐线程数（用户未自定义时的默认值）
    pub recommended: usize,
    /// 推荐理由（一行，直接展示给用户）
    pub reason: String,
    /// 可选档位（UI 下拉）
    pub options: Vec<usize>,
    /// 用户已保存的自定义值（None = 跟随推荐）
    pub saved: Option<usize>,
    /// 实际生效值（saved 或 recommended）
    pub effective: usize,
}

/// 物理核数探测
///
/// Windows 下最可靠的口径：`GetLogicalProcessorInformation` 返回的物理核条目数。
/// 不引入 `num_cpus` 之外的依赖 —— 用 `std::thread::available_parallelism` 拿逻辑数，
/// 物理核则由「内核数 = 逻辑数 / 每核线程数」推断，并在 Windows 上用环境变量
/// `NUMBER_OF_PROCESSORS` 交叉校验。
fn logical_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or_else(|_| {
            std::env::var("NUMBER_OF_PROCESSORS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(4)
        })
}

/// 物理核数：Windows 用 WMI 口径不可行（需要依赖），改用注册表/环境交叉推断
///
/// 策略：读取 `%NUMBER_OF_PROCESSORS%`（逻辑核）与 CPU 型号特征；
/// 绝大多数现代 x86 桌面/笔记本为「物理核 × 2 = 逻辑核」，据此推断；
/// 若逻辑核为奇数或型号显示无超线程（如 i5-9400F），则逻辑核即物理核。
fn physical_cores(logical: usize) -> (usize, bool) {
    physical_cores_for(logical, &cpu_model())
}

/// 物理核推断的**纯函数**版本（可测；`model` 由调用方注入）
///
/// 拆出来的原因：`physical_cores` 会读真实注册表，测试里无法构造期望值；
/// 纯函数版本让「奇数核 / 无超线程型号 / 普通型号」三种分支都能被覆盖。
fn physical_cores_for(logical: usize, model_raw: &str) -> (usize, bool) {
    // 奇数逻辑核 → 不可能是超线程（每核 2 线程必为偶数）
    if logical % 2 != 0 {
        return (logical, false);
    }
    // 型号线索：部分 Intel 型号无超线程；AMD Ryzen 全系有 SMT，不在此列
    let model = model_raw.to_lowercase();
    let no_ht = [
        "-9400f",
        "-9100f",
        "-9600kf",
        "-9350kf",
        "pentium",
        "celeron",
        "atom",
    ]
    .iter()
    .any(|k| model.contains(k));
    if no_ht {
        return (logical, false);
    }
    ((logical / 2).max(1), true)
}

/// CPU 型号（Windows: WMI 不可用 → 读注册表 ProcessorNameString；其他平台 best-effort）
fn cpu_model() -> String {
    #[cfg(windows)]
    {
        // reg query 比 WMI 轻（无 COM 初始化），失败则返回空串走保守推断
        if let Ok(out) = std::process::Command::new("reg")
            .args([
                "query",
                r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
                "/v",
                "ProcessorNameString",
            ])
            .output()
        {
            if let Ok(s) = String::from_utf8(out.stdout) {
                if let Some(idx) = s.find("REG_SZ") {
                    return s[idx + 6..].trim().to_string();
                }
            }
        }
    }
    String::new()
}

/// 大小核混合架构检测（Intel 12 代+ 的 P-core/E-core）
///
/// 混合架构下，任务被调度到 E 核时单线程性能骤降，且 E 核数量多但吞吐低；
/// 对 EXIF 这种短任务反而合适（E 核够用且省电），故不减少线程数，只做提示。
fn is_hybrid(model: &str) -> bool {
    let m = model.to_lowercase();
    // Intel 12/13/14 代桌面/移动：型号形如 "i7-12700H" / "i5-1340P" / "i9-14900K"
    let intel_hybrid = m.contains("intel")
        && (m.contains("-12") || m.contains("-13") || m.contains("-14"))
        && (m.contains('h') || m.contains('p') || m.contains('k') || m.contains('u'));
    // Core Ultra（Meteor Lake 及以后）
    intel_hybrid || m.contains("core ultra") || m.contains("core(tm) ultra")
}

/// 磁盘介质类型（SSD / HDD / 未知）
///
/// EXIF 扫描是随机小读：SSD 上线程越多吞吐越高（并行度直接换速度）；
/// HDD 上过多线程导致磁头频繁寻道，反而**变慢** —— 这是推荐值必须区分盘型的根本原因。
/// Windows 用 PowerShell 的 `Get-PhysicalDisk` 取 MediaType（一次 ~80ms，可接受）。
fn disk_kind() -> String {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-PhysicalDisk | Select-Object -First 1 -ExpandProperty MediaType)",
            ])
            .output();
        if let Ok(o) = out {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() && s != "Unspecified" {
                return s; // "SSD" / "HDD" / "SCM"
            }
        }
    }
    "未知".into()
}

/// 计算推荐线程数（「根据实际计算推荐」的核心）
///
/// 依据（按优先级）：
///   1. **盘型**：HDD → 上限 4（避免寻道抖动）；SSD/未知 → 放开
///   2. **物理核数**：真实并行单元，是吞吐上限的基准
///   3. **超线程折扣**：EXIF 解析 CPU 占比不高，逻辑核可贡献约 30% 额外吞吐，
///      故推荐 = 物理核 + (逻辑核 - 物理核) / 3，而非直接用逻辑核数
///   4. **经验上限**：8 线程后收益明显递减（调度/内存带宽/文件句柄争用），封顶 12
///   5. **下限**：至少 2（即使是单核机，IO 等待也能从 2 线程获益）
fn compute_recommended(logical: usize, physical: usize, disk: &str) -> (usize, String) {
    let (ht_bonus, ht_note) = if logical > physical {
        ((logical - physical) * 2 / 5, "含超线程加成")
    } else {
        (0, "无超线程")
    };
    let mut n = physical + ht_bonus;
    let mut notes: Vec<String> = vec![format!("物理核 {physical}（{ht_note}）")];

    // 上限：经验递减点
    let cap = 12usize;
    if n > cap {
        n = cap;
        notes.push(format!("线程数封顶 {cap}（更多线程收益递减）"));
    }

    // 盘型约束（HDD 随机读的并行会互相拖慢）
    let disk_upper = disk.to_lowercase();
    if disk_upper.contains("hdd") {
        if n > 4 {
            n = 4;
        }
        notes.push("机械盘（HDD）随机读寻道敏感，建议不超过 4 线程".into());
    }

    let n = n.clamp(2.min(logical).max(1), SCAN_THREADS_MAX).max(1);
    let final_n = n.min(logical.max(1));
    if final_n != physical + ht_bonus {
        notes.push(format!("最终按负载特征折算为 {final_n}"));
    }
    (final_n, notes.join("；"))
}

/// 可选档位：常见档 + 推荐值 + 物理/逻辑核（去重升序）
fn thread_options(logical: usize, physical: usize, recommended: usize) -> Vec<usize> {
    let mut base: Vec<usize> = vec![1, 2, 4, 6, 8, 12, 16, 24, 32]
        .into_iter()
        .filter(|n| *n <= logical.min(SCAN_THREADS_MAX))
        .collect();
    base.extend([physical, recommended, logical.min(SCAN_THREADS_MAX)]);
    base.retain(|n| *n >= SCAN_THREADS_MIN && *n <= logical.min(SCAN_THREADS_MAX));
    base.sort_unstable();
    base.dedup();
    base
}

/// 线程数持久化文件（与 Python VCR 的 current_threads.json 分开，互不影响）
///
/// 落 app_data_dir 根（与日志、数据库同级），便于「开发者视角」一并查看。
fn threads_file(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("scan_threads.json"))
}

/// 用户保存的线程数（None = 跟随推荐）
pub fn saved_threads(app: &tauri::AppHandle) -> Option<usize> {
    let path = threads_file(app)?;
    let text = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let n = v.get("threads")?.as_u64()? as usize;
    (SCAN_THREADS_MIN..=SCAN_THREADS_MAX).contains(&n).then_some(n)
}

/// 保存用户线程数（越界夹紧）；`None` = 清除自定义、回到推荐
pub fn save_threads(app: &tauri::AppHandle, n: Option<usize>) -> Result<usize, String> {
    let topo = topology(app);
    let path = threads_file(app).ok_or_else(|| "无法定位应用数据目录".to_string())?;
    match n {
        None => {
            let _ = std::fs::remove_file(&path);
            Ok(topo.recommended)
        }
        Some(v) => {
            let v = v.clamp(SCAN_THREADS_MIN, SCAN_THREADS_MAX.min(topo.logical.max(1)));
            std::fs::write(&path, serde_json::json!({ "threads": v }).to_string())
                .map_err(|e| format!("保存线程数失败: {e}"))?;
            Ok(v)
        }
    }
}

/// 完整拓扑 + 推荐值（UI 一次拉取）
pub fn topology(app: &tauri::AppHandle) -> CpuTopology {
    let logical = logical_cores();
    let (physical, estimated) = physical_cores(logical);
    let model = cpu_model();
    let hybrid = is_hybrid(&model);
    let disk = disk_kind();
    let (recommended, reason) = compute_recommended(logical, physical, &disk);
    let saved = saved_threads(app);
    CpuTopology {
        logical,
        physical,
        hybrid,
        estimated,
        disk_kind: disk,
        recommended,
        reason,
        options: thread_options(logical, physical, recommended),
        effective: saved.unwrap_or(recommended),
        saved,
    }
}

/// 生效线程数（扫描命令用）
pub fn effective_threads(app: &tauri::AppHandle) -> usize {
    let t = topology(app);
    t.effective.clamp(1, SCAN_THREADS_MAX)
}

/// 目标目录所在盘（仅诊断/日志用）
#[allow(dead_code)] // 预留：日志中标注扫描发生在哪个盘（多盘用户排查性能用）
pub fn disk_of(dir: &str) -> String {
    Path::new(dir)
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 推荐值必须落在合理区间，且是「物理核 + 超线程折扣」的量级
    #[test]
    fn recommended_is_sane_for_ssd() {
        // 8 核 16 线程 + SSD：物理 8 + (16-8)*2/5=3 → 11，封顶 12 → 11
        let (n, why) = compute_recommended(16, 8, "SSD");
        assert!((2..=12).contains(&n), "推荐值应在 [2,12]，实际 {n}");
        assert!(n >= 8, "不应低于物理核数（IO 密集负载可从超线程获益）");
        assert!(why.contains("物理核"), "理由应说明物理核依据：{why}");
    }

    /// HDD 必须被压到 ≤4 线程（寻道抖动会让多线程反而变慢）
    #[test]
    fn hdd_capped_to_four() {
        let (n, why) = compute_recommended(16, 8, "HDD");
        assert!(n <= 4, "机械盘应限制在 4 线程内，实际 {n}");
        assert!(why.contains("机械盘"), "理由应提示盘型约束：{why}");
    }

    /// 无超线程（逻辑核 == 物理核）时不应凭空加成
    #[test]
    fn no_ht_no_bonus() {
        let (n, _) = compute_recommended(4, 4, "SSD");
        assert_eq!(n, 4, "4 物理核无超线程 → 4 线程");
    }

    /// 单核机下限保护：至少 1，且不因折算变成 0
    #[test]
    fn single_core_lower_bound() {
        let (n, _) = compute_recommended(1, 1, "SSD");
        assert!(n >= 1, "单核机也应有可用线程数，实际 {n}");
    }

    /// 档位列表必须含推荐值、物理核，且严格升序去重
    #[test]
    fn options_include_key_points() {
        let opts = thread_options(16, 8, 11);
        assert!(opts.contains(&11), "档位应含推荐值 11：{opts:?}");
        assert!(opts.contains(&8), "档位应含物理核 8：{opts:?}");
        assert!(opts.windows(2).all(|w| w[0] < w[1]), "档位应严格升序：{opts:?}");
        assert!(opts.iter().all(|n| *n <= 16), "档位不应超过逻辑核：{opts:?}");
    }

    /// 物理核推断：奇数逻辑核不可能是超线程；无超线程型号不应被折半
    #[test]
    fn physical_cores_inference() {
        // 奇数逻辑核 → 物理核 = 逻辑核（超线程必为偶数）
        let (p, est) = physical_cores_for(7, "Intel(R) Core(TM) i7-12700H");
        assert_eq!(p, 7, "奇数逻辑核不折半");
        assert!(!est);
        // 普通有超线程型号 → 折半
        let (p, est) = physical_cores_for(16, "AMD Ryzen 7 7840HS");
        assert_eq!(p, 8, "16 逻辑核应折半为 8 物理核");
        assert!(est, "推断值应标记 estimated");
        // 明确无超线程型号 → 不折半
        let (p, _) = physical_cores_for(6, "Intel(R) Core(TM) i5-9400F CPU @ 2.90GHz");
        assert_eq!(p, 6, "9400F 无超线程，逻辑核即物理核");
    }
}
