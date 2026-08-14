//! 相册全量 EXIF + 文件系统 + 图片头部信息分析
//! 用法: cargo run --example album_analyze -- "相册目录"
use std::collections::HashMap;
use std::path::Path;

fn main() {
    let dir = std::env::args().nth(1).expect("需要相册目录参数");
    let root = Path::new(&dir);
    let mut files: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            let n = p.to_string_lossy().to_lowercase();
            n.ends_with(".jpg") || n.ends_with(".jpeg") || n.ends_with(".png")
                || n.ends_with(".webp") || n.ends_with(".gif") || n.ends_with(".bmp")
        })
        .collect();
    files.sort();
    println!("相册图片总数: {}", files.len());

    let mut coverage: HashMap<String, usize> = HashMap::new();
    let total = files.len();
    let mut sizes: Vec<u64> = Vec::new();
    let mut dims: Vec<(u32, u32)> = Vec::new();
    let mut iso_vals: Vec<u32> = Vec::new();
    let mut focal_vals: Vec<f64> = Vec::new();
    let mut aperture_vals: Vec<f64> = Vec::new();
    let mut exposure_vals: Vec<f64> = Vec::new();
    let mut shoot_times: Vec<String> = Vec::new();
    let mut models: HashMap<String, usize> = HashMap::new();
    let mut lenses: HashMap<String, usize> = HashMap::new();
    let mut no_exif = 0usize;

    for f in &files {
        let meta = f.metadata().unwrap();
        sizes.push(meta.len());
        // 尺寸: jpeg-decoder 只读 SOF 头部不整图解码（注意：当前实现需 decode 一次；
        // 实际项目可用 `decoder.info()` 前的 SOF 扫描优化，此处先保证正确性）
        if let Some(dim) = read_jpeg_dim(f) {
            dims.push(dim);
        } else if let Ok(img) = image::ImageReader::open(f).and_then(|r| r.with_guessed_format()).map(|r| r.into_dimensions()) {
            if let Ok(d) = img {
                dims.push(d);
            }
        }

        let file = std::fs::File::open(f).unwrap();
        let mut buf = std::io::BufReader::new(&file);
        let reader = exif::Reader::new();
        let Ok(exif) = reader.read_from_container(&mut buf) else {
            no_exif += 1;
            continue;
        };
        for field in exif.fields() {
            let name = format!("{:?}@{:?}", field.tag, field.ifd_num);
            *coverage.entry(name).or_insert(0) += 1;
        }
        let v = |t: exif::Tag| exif.get_field(t, exif::In::PRIMARY);
        if let Some(f) = v(exif::Tag::ISOSpeed) {
            if let exif::Value::Short(s) = &f.value {
                if let Some(&x) = s.first() {
                    iso_vals.push(x as u32);
                }
            }
        }
        if let Some(f) = v(exif::Tag::FocalLength) {
            if let exif::Value::Rational(r) = &f.value {
                if let Some(r) = r.first() {
                    focal_vals.push(r.to_f64());
                }
            }
        }
        if let Some(f) = v(exif::Tag::FNumber) {
            if let exif::Value::Rational(r) = &f.value {
                if let Some(r) = r.first() {
                    aperture_vals.push(r.to_f64());
                }
            }
        }
        if let Some(f) = v(exif::Tag::ExposureTime) {
            if let exif::Value::Rational(r) = &f.value {
                if let Some(r) = r.first() {
                    exposure_vals.push(r.to_f64());
                }
            }
        }
        if let Some(f) = v(exif::Tag::DateTimeOriginal) {
            let s = f.display_value().to_string();
            let date: String = s.chars().take(10).collect();
            shoot_times.push(date);
        }
        if let Some(f) = v(exif::Tag::Model) {
            *models.entry(f.display_value().to_string()).or_insert(0) += 1;
        }
        if let Some(f) = v(exif::Tag::LensModel) {
            *lenses.entry(f.display_value().to_string()).or_insert(0) += 1;
        }
    }

    println!("\n===== 文件系统信息 (std::fs) =====");
    println!("总大小: {:.1} MB | 平均: {:.1} MB",
        sizes.iter().sum::<u64>() as f64 / 1e6,
        sizes.iter().sum::<u64>() as f64 / 1e6 / total as f64);
    println!("最小/最大: {:.1} MB / {:.1} MB", *sizes.iter().min().unwrap() as f64 / 1e6, *sizes.iter().max().unwrap() as f64 / 1e6);

    println!("\n===== 图片头部 (jpeg-decoder / image) =====");
    let widths: Vec<u32> = dims.iter().map(|d| d.0).collect();
    let heights: Vec<u32> = dims.iter().map(|d| d.1).collect();
    println!("能读到尺寸: {}/{}", dims.len(), total);
    println!("宽范围: {} ~ {} px", widths.iter().min().unwrap(), widths.iter().max().unwrap());
    println!("高范围: {} ~ {} px", heights.iter().min().unwrap(), heights.iter().max().unwrap());
    println!("横向: {} | 纵向: {} | 方形: {}", widths.iter().filter(|w| **w > heights[widths.iter().position(|x| x == *w).unwrap()]).count(),
        widths.iter().enumerate().filter(|(i, w)| **w < heights[*i]).count(),
        widths.iter().enumerate().filter(|(i, w)| **w == heights[*i]).count());

    println!("\n===== EXIF 读取 (kamadak-exif) =====");
    println!("有 EXIF: {} | 无 EXIF: {}", total - no_exif, no_exif);
    println!("\n拍摄时间范围: {} ~ {}", shoot_times.iter().min().unwrap(), shoot_times.iter().max().unwrap());
    let mut by_date: HashMap<String, usize> = HashMap::new();
    for t in &shoot_times {
        *by_date.entry(t.clone()).or_insert(0) += 1;
    }
    for (d, c) in by_date {
        println!("  {d}: {c} 张");
    }
    println!("\n相机机型:");
    for (m, c) in models { println!("  {m}: {c} 张"); }
    println!("镜头:");
    for (l, c) in lenses { println!("  {l}: {c} 张"); }
    if !iso_vals.is_empty() {
        println!("ISO 范围: {} ~ {} | 平均: {:.0}", iso_vals.iter().min().unwrap(), iso_vals.iter().max().unwrap(), iso_vals.iter().sum::<u32>() as f64 / iso_vals.len() as f64);
    }
    if !focal_vals.is_empty() {
        println!("焦距范围: {:.1} ~ {:.1} mm (35mm 等效约 {:.1} ~ {:.1} mm)",
            focal_vals.iter().fold(f64::MAX, |a, &b| a.min(b)),
            focal_vals.iter().fold(f64::MIN, |a, &b| a.max(b)),
            focal_vals.iter().fold(f64::MAX, |a, &b| a.min(b)) * 1.5,
            focal_vals.iter().fold(f64::MIN, |a, &b| a.max(b)) * 1.5);
    }
    if !aperture_vals.is_empty() {
        println!("光圈范围: f/{:.1} ~ f/{:.1}", aperture_vals.iter().fold(f64::MAX, |a, &b| a.min(b)), aperture_vals.iter().fold(f64::MIN, |a, &b| a.max(b)));
    }
    if !exposure_vals.is_empty() {
        let min = exposure_vals.iter().fold(f64::MAX, |a, &b| a.min(b));
        let max = exposure_vals.iter().fold(f64::MIN, |a, &b| a.max(b));
        println!("快门范围: 1/{:.0} s ~ 1/{:.0} s", 1.0 / max, 1.0 / min);
    }

    println!("\n===== 字段覆盖率 (前 25) =====");
    let mut cov: Vec<_> = coverage.iter().collect();
    cov.sort_by(|a, b| b.1.cmp(a.1));
    for (k, c) in cov.iter().take(25) {
        println!("  {c:>3}/{total}  {}", k);
    }
}

/// jpeg-decoder 只读 SOF 头部拿尺寸（不解码像素）
fn read_jpeg_dim(path: &Path) -> Option<(u32, u32)> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
    let _ = decoder.decode().ok()?;
    let info = decoder.info()?;
    Some((info.width as u32, info.height as u32))
}
