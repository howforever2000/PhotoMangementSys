//! 扫描线程数实测基准（检验 scan_perf 推荐公式是否真的最快）
//!
//! 用法: cargo run --release --example scan_threads_bench -- "相册目录" [递归1/0] [轮数] [预热] [档位]
//!   例: cargo run --release --example scan_threads_bench -- "D:/YUAN HAO/Pictures/2026/毕业照" 1 5 1 1,2,4,6,8,11,12,16
//!
//! 为什么单独写基准而不是用 App 内的 calibrate_scan_threads：
//!   - calibrate_scan_threads 需一次完整 Tauri 会话才能调用；
//!   - 这里能自由指定档位 / 轮数 / 样本量，做更严格的对照实验。
//!
//! 复刻的 IO 模式与 photo_scan::read_photo_exif 一致：
//!   File::open → BufReader → exif::Reader::read_from_container
//! 换算到「每张」时按生产的字段提取量（全字段 display_value）估算 CPU 侧开销，
//! 使测得的形状与 App 内实测可互相印证。

use std::path::{Path, PathBuf};
use std::time::Instant;

const EXTS: [&str; 9] = [
    ".jpg", ".jpeg", ".png", ".bmp", ".webp", ".tif", ".tiff", ".heic", ".gif",
];

fn is_image(name: &str) -> bool {
    let l = name.to_lowercase();
    EXTS.iter().any(|e| l.ends_with(e))
}

/// 收集图片（与 test_scan::collect_image_files 同语义：按名排序）
fn collect(dir: &str, recurse: bool) -> Vec<(PathBuf, String)> {
    let root = Path::new(dir);
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    if !recurse {
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() {
                    let n = p.file_name().unwrap().to_string_lossy().into_owned();
                    if is_image(&n) {
                        files.push((p, n));
                    }
                }
            }
        }
    } else {
        for e in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
        {
            let Ok(e) = e else { continue };
            if e.file_type().is_file() {
                let n = e.file_name().to_string_lossy().into_owned();
                if is_image(&n) {
                    files.push((e.into_path(), n));
                }
            }
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    files
}

/// 单张读取：复刻 read_photo_exif 的 IO + 解析路径
fn read_one(p: &Path) -> bool {
    let Ok(file) = std::fs::File::open(p) else {
        return false;
    };
    let mut buf = std::io::BufReader::new(&file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut buf) else {
        return false;
    };
    // 生产侧会逐字段取值 + display_value()（含 rational→字符串格式化），
    // 这里按同量级遍历一遍，避免把 CPU 侧成本测没了。
    let mut acc = 0usize;
    for f in exif.fields() {
        acc = acc.wrapping_add(f.display_value().to_string().len());
    }
    std::hint::black_box(acc);
    true
}

/// 跑一轮，返回 (耗时秒, 张数)
fn run(files: &[(PathBuf, String)], threads: usize) -> (f64, usize) {
    let t0 = Instant::now();
    let n = if threads <= 1 {
        let mut c = 0usize;
        for (p, _) in files {
            c += read_one(p) as usize;
        }
        c
    } else {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("线程池创建失败");
        pool.install(|| {
            use rayon::prelude::*;
            files.par_iter().map(|(p, _)| read_one(p) as usize).sum()
        })
    };
    (t0.elapsed().as_secs_f64(), n)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().cloned().unwrap_or_else(|| {
        eprintln!("用法: cargo run --release --example scan_threads_bench -- <相册目录> [递归1/0] [轮数] [预热] [档位]");
        std::process::exit(2);
    });
    let recurse = args.get(1).map(|s| s != "0").unwrap_or(true);
    let rounds: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let warmup: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let thread_list: Vec<usize> = args
        .get(4)
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1, 2, 4, 6, 8, 11, 12, 16]);

    let all = collect(&dir, recurse);
    if all.is_empty() {
        eprintln!("目录内没有找到图片: {dir}");
        std::process::exit(1);
    }
    let total_bytes: u64 = all
        .iter()
        .filter_map(|(p, _)| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();

    println!("=== 生产 EXIF 扫描路径 · 线程数实测 ===");
    println!("目录      : {dir}  (recurse={recurse})");
    println!(
        "图片总数  : {} 张 / {:.1} MB（均 {:.2} MB）",
        all.len(),
        total_bytes as f64 / 1e6,
        total_bytes as f64 / all.len() as f64 / 1e6
    );
    println!(
        "CPU 逻辑核: {}",
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0)
    );
    println!("测量口径  : 预热 {warmup} 轮 + 正式 {rounds} 轮交错测量（每轮轮转所有档位），取最快值");
    println!("{}", "-".repeat(92));

    // 预热：每档位各跑一遍
    for _ in 0..warmup {
        for &n in &thread_list {
            let _ = run(&all, n);
        }
    }

    // 正式测量：**交错轮转** —— 每轮把所有档位都测一遍，再进入下一轮。
    // 这样任何随时间漂移的因素（热降频、后台进程、页缓存淘汰）会等量作用到
    // 所有档位，而不是集中惩罚「排在后面测的档位」。
    let mut samples: Vec<(usize, Vec<f64>)> =
        thread_list.iter().map(|&n| (n, Vec::new())).collect();
    for _ in 0..rounds {
        for slot in samples.iter_mut() {
            let (dt, _) = run(&all, slot.0);
            slot.1.push(dt);
        }
    }

    let mut results: Vec<(usize, f64, f64, f64)> = Vec::new(); // (threads, best, median, rate)
    for (n, mut times) in samples {
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let best = times[0];
        let median = times[times.len() / 2];
        let rate = all.len() as f64 / best;
        results.push((n, best, median, rate));
        println!(
            "{n:>3} 线程 | 最快 {:8.1} ms | 中位 {:8.1} ms | 吞吐 {:8.0} 张/秒 | 每张 {:5.2} ms | 整盘预估 {:5.1}s",
            best * 1000.0,
            median * 1000.0,
            rate,
            best / all.len() as f64 * 1000.0,
            all.len() as f64 / rate
        );
    }

    println!("{}", "-".repeat(92));
    let peak = results
        .iter()
        .cloned()
        .fold((0usize, 0.0f64, 0.0f64, 0.0f64), |a, b| if b.3 > a.3 { b } else { a });
    println!("实测峰值         : {} 线程（{:.0} 张/秒）", peak.0, peak.3);

    let thr95 = peak.3 * 0.95;
    let opt = results
        .iter()
        .filter(|r| r.3 >= thr95)
        .min_by_key(|r| r.0)
        .cloned()
        .expect("至少有一个档位");
    println!(
        "达峰值95%最小档  : {} 线程（{:.0} 张/秒，峰值的 {:.1}%）",
        opt.0,
        opt.3,
        opt.3 / peak.3 * 100.0
    );
    if let Some(b) = results.iter().find(|r| r.0 == 1) {
        println!(
            "相对单线程提速   : {:.2}×（最优档） / {:.2}×（峰值档）",
            opt.3 / b.3,
            peak.3 / b.3
        );
    }
    if let Some(r) = results.iter().find(|r| r.0 == 11) {
        println!(
            "11 线程(公式推荐): {:.0} 张/秒 = 峰值的 {:.1}%（比峰值档 {} 线程慢 {:.1}%）",
            r.3,
            r.3 / peak.3 * 100.0,
            peak.0,
            (peak.3 - r.3) / peak.3 * 100.0
        );
    }
}
