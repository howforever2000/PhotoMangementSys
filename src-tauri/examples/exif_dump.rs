//! EXIF 全字段读取测试（项目自用 kamadak-exif）
//! 用法: cargo run --example exif_dump -- "图片路径..."
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: cargo run --example exif_dump -- <图片路径> [...]");
        return;
    }
    for p in &args {
        dump_one(Path::new(p));
    }
}

fn dump_one(path: &Path) {
    println!("\n================ {} ================", path.display());
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            println!("  打开失败: {e}");
            return;
        }
    };
    let mut buf = std::io::BufReader::new(&file);
    let reader = exif::Reader::new();
    match reader.read_from_container(&mut buf) {
        Ok(exif) => {
            for field in exif.fields() {
                let ifd = match field.ifd_num.index() {
                    0 => "IFD0 ",
                    1 => "THUMB",
                    2 => "EXIF ",
                    3 => "GPS  ",
                    4 => "INTER",
                    _ => "???  ",
                };
                let v = field.display_value().to_string();
                let v = if v.len() > 60 { &v[..57] } else { &v };
                println!(
                    "  [{}] {:<38} = {}",
                    ifd,
                    format!("{:?}", field.tag),
                    v
                );
            }
        }
        Err(e) => println!("  无 EXIF 或读取失败: {e}"),
    }
}
