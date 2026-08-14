//! 相册照片 EXIF 扫描模块（测试功能，独立组件）
//!
//! 职责：遍历相册目录内所有图片，提取 EXIF 拍摄参数
//! （ISO / 焦段 / 光圈 / 快门速度 / 拍摄时间），供前端表格展示。
//!
//! 解耦原则：
//! - 不依赖 `db` / `thumbnail` / `folder` 模块（图片扩展名列表本地定义，独立可测）
//! - 不修改任何数据库 schema，扫描结果不落库
//! - `lib.rs` 仅保留一个薄命令壳 `scan_album_photos`，避免命令层变重

use std::path::Path;

use serde::Serialize;

/// 支持的图片扩展名（与 thumbnail 模块保持一致；为解耦本地复制一份）
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

/// 单张照片的 EXIF 扫描结果
///
/// 所有拍摄参数均为 `Option`：图片无 EXIF / 字段缺失 / 读取失败时置 `None`，
/// 由前端显示占位符，保证照片名照常出现在表格中。
#[derive(Debug, Clone, Serialize)]
pub struct PhotoExif {
    /// 文件名（不含路径）
    pub file_name: String,
    /// 完整路径（前端 tooltip 显示用）
    pub path: String,
    /// ISO 感光度，如 "100"
    pub iso: Option<String>,
    /// 焦段，如 "50mm"
    pub focal_length: Option<String>,
    /// 光圈，如 "f/2.8"
    pub aperture: Option<String>,
    /// 快门速度，如 "1/200s"
    pub shutter_speed: Option<String>,
    /// 拍摄时间，如 "2023-01-15 10:30:00"
    pub shoot_time: Option<String>,
    /// 纬度（十进制度，WGS84），如 31.921282
    pub lat: Option<f64>,
    /// 经度（十进制度，WGS84）
    pub lon: Option<f64>,
    /// 纬度原始度分秒字符串，如 "31°55'16.61\"N"
    pub lat_raw: Option<String>,
    /// 经度原始度分秒字符串
    pub lon_raw: Option<String>,
    /// 海拔（米）
    pub alt_m: Option<f64>,
    /// 地图链接（离线可用，点开即定位）
    pub map_url: Option<String>,
    /// 反向地理编码地名（仅 with_place 扫描时填充）
    pub place: Option<String>,
}

/// 反向地理编码：坐标 → 中文地名。
///
/// 1) BigDataCloud（免 key、国内直连、localityLanguage=zh 返回中文行政区划）
/// 2) Nominatim / OpenStreetMap（标准但国内可能超时，作为备选）
pub(crate) fn reverse_geocode(client: &reqwest::blocking::Client, lat: f64, lon: f64) -> Option<String> {
    let url = format!(
        "https://api.bigdatacloud.net/data/reverse-geocode-client?latitude={lat:.6}&longitude={lon:.6}&localityLanguage=zh"
    );
    if let Ok(resp) = client.get(&url).send() {
        if let Ok(json) = resp.json::<serde_json::Value>() {
            let parts = [
                json["countryName"].as_str().map(str::to_string),
                json["principalSubdivision"].as_str().map(str::to_string),
                json["city"].as_str().map(str::to_string),
                json["locality"].as_str().map(str::to_string),
            ];
            let joined: Vec<String> = parts.into_iter().flatten().collect();
            if !joined.is_empty() {
                return Some(joined.join(" · "));
            }
        }
    }
    let url = format!(
        "https://nominatim.openstreetmap.org/reverse?format=jsonv2&lat={lat:.6}&lon={lon:.6}&zoom=16&accept-language=zh-CN"
    );
    if let Ok(resp) = client.get(&url).send() {
        if let Ok(json) = resp.json::<serde_json::Value>() {
            if let Some(name) = json["display_name"].as_str() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// 读取单张图片的 EXIF（每个字段独立容错，失败置 None）
pub(crate) fn read_photo_exif(path: &Path, name: &str) -> PhotoExif {
    let mut photo = PhotoExif {
        file_name: name.to_string(),
        path: path.to_string_lossy().into_owned(),
        iso: None,
        focal_length: None,
        aperture: None,
        shutter_speed: None,
        shoot_time: None,
        lat: None,
        lon: None,
        lat_raw: None,
        lon_raw: None,
        alt_m: None,
        map_url: None,
        place: None,
    };

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return photo,
    };
    let mut buf = std::io::BufReader::new(&file);
    let exif = match exif::Reader::new().read_from_container(&mut buf) {
        Ok(exif) => exif,
        Err(_) => return photo,
    };

    // ISO：0x8827（PhotographicSensitivity / ISOSpeedRatings）为主，0x8833（ISOSpeed）兜底
    photo.iso = field_by_number(&exif, 0x8827)
        .or_else(|| field_by_number(&exif, 0x8833))
        .and_then(|f| match &f.value {
            exif::Value::Short(v) => v.first().map(|x| format_iso(*x as u32)),
            exif::Value::Long(v) => v.first().map(|x| format_iso(*x)),
            _ => None,
        });

    // 焦段：FocalLength（0x920A）
    if let Some(f) = field_by_number(&exif, 0x920a) {
        if let Some(v) = field_value_f64(f) {
            photo.focal_length = Some(format_focal(v));
        }
    }

    // 光圈：FNumber（0x829D）
    if let Some(f) = field_by_number(&exif, 0x829d) {
        if let Some(v) = field_value_f64(f) {
            photo.aperture = Some(format_aperture(v));
        }
    }

    // 快门速度：ExposureTime（0x829A）
    if let Some(f) = field_by_number(&exif, 0x829a) {
        if let Some(v) = field_value_f64(f) {
            photo.shutter_speed = Some(format_shutter(v));
        }
    }

    // 拍摄时间：DateTimeOriginal（0x9003）→ DateTime（0x0132）→ GPS-UTC+8 三级兜底
    // （扫描图/后期处理图常缺 Original 但有 DateTime）
    photo.shoot_time = field_by_number(&exif, 0x9003)
        .or_else(|| field_by_number(&exif, 0x0132))
        .and_then(|f| Some(format_shoot_time(&f.display_value().to_string())))
        .or_else(|| gps_utc_time(&exif));

    // ---------- 地点（GPS）----------
    read_gps_location(&exif, &mut photo);

    photo
}

/// GPS 时间（UTC +8h）→ "YYYY-MM-DD HH:MM:SS"，相机时间漂移/无本地时间时兜底
///
/// GPSDateStamp（ASCII "YYYY:MM:DD"）+ GPSTimeStamp（Rational 时分秒）。
fn gps_utc_time(exif: &exif::Exif) -> Option<String> {
    let ds = exif.get_field(exif::Tag::GPSDateStamp, exif::In::PRIMARY)?;
    let date = ascii_str(&ds.value);
    let parts: Vec<&str> = date.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i64 = parts[0].parse().ok()?;
    let mo: i64 = parts[1].parse().ok()?;
    let d: i64 = parts[2].parse().ok()?;

    let ts = exif.get_field(exif::Tag::GPSTimeStamp, exif::In::PRIMARY)?;
    let t = value_to_f64s(&ts.value);
    if t.len() < 3 {
        return None;
    }
    let secs = civil_to_unix(y, mo, d, t[0] as i64, t[1] as i64, t[2] as i64)? + 8 * 3600;
    let (yy, mo2, dd, hh, mi, ss) = unix_to_civil(secs);
    Some(format!("{yy:04}-{mo2:02}-{dd:02} {hh:02}:{mi:02}:{ss:02}"))
}

/// GPS 地点提取：度分秒 → 十进制度 + 海拔 + 地图链接
///
/// GPSLatitude/GPSLongitude 为 Rational 度分秒数组，
/// 配合 GPSLatitudeRef/GPSLongitudeRef（N/S/E/W）判定正负。
fn read_gps_location(exif: &exif::Exif, photo: &mut PhotoExif) {
    let lat_ref = exif.get_field(exif::Tag::GPSLatitudeRef, exif::In::PRIMARY).map(|f| ascii_str(&f.value));
    let lon_ref = exif.get_field(exif::Tag::GPSLongitudeRef, exif::In::PRIMARY).map(|f| ascii_str(&f.value));
    let lat_f = exif.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY);
    let lon_f = exif.get_field(exif::Tag::GPSLongitude, exif::In::PRIMARY);

    let (Some(lat_f), Some(lat_r), Some(lon_f), Some(lon_r)) = (lat_f, &lat_ref, lon_f, &lon_ref) else {
        return;
    };
    let lat_dms = value_to_f64s(&lat_f.value);
    let lon_dms = value_to_f64s(&lon_f.value);
    if lat_dms.len() < 3 || lon_dms.len() < 3 {
        return;
    }
    let lat = dms_to_decimal(&lat_dms, lat_r);
    let lon = dms_to_decimal(&lon_dms, lon_r);
    photo.lat = Some(lat);
    photo.lon = Some(lon);
    photo.lat_raw = Some(format!(
        "{}°{}'{:.2}\"{}",
        lat_dms[0] as u32, lat_dms[1] as u32, lat_dms[2], lat_r
    ));
    photo.lon_raw = Some(format!(
        "{}°{}'{:.2}\"{}",
        lon_dms[0] as u32, lon_dms[1] as u32, lon_dms[2], lon_r
    ));
    photo.map_url = Some(format!(
        "https://www.google.com/maps?q={lat:.6},{lon:.6}"
    ));
    // 海拔（GPSAltitude，Rational 米）
    if let Some(alt) = exif.get_field(exif::Tag::GPSAltitude, exif::In::PRIMARY) {
        if let Some(v) = value_to_f64s(&alt.value).first() {
            photo.alt_m = Some(*v);
        }
    }
}

/// 度分秒数组 + N/S/E/W → 十进制度
fn dms_to_decimal(dms: &[f64], refr: &str) -> f64 {
    let mut dec = dms[0] + dms[1] / 60.0 + dms[2] / 3600.0;
    if matches!(refr, "S" | "W") {
        dec = -dec;
    }
    dec
}

/// EXIF Value → f64 数组（Rational/Float/Double 统一）
fn value_to_f64s(value: &exif::Value) -> Vec<f64> {
    match value {
        exif::Value::Rational(v) => v.iter().map(|r| {
            let den = r.denom;
            if den != 0 {
                r.num as f64 / den as f64
            } else {
                0.0
            }
        }).collect(),
        exif::Value::SRational(v) => v.iter().map(|r| {
            let den = r.denom;
            if den != 0 {
                r.num as f64 / den as f64
            } else {
                0.0
            }
        }).collect(),
        exif::Value::Float(v) => v.iter().map(|x| *x as f64).collect(),
        exif::Value::Double(v) => v.iter().copied().collect(),
        _ => vec![],
    }
}

/// ASCII 值拼接（EXIF 字符串字段）
fn ascii_str(value: &exif::Value) -> String {
    match value {
        exif::Value::Ascii(v) => v
            .iter()
            .map(|b| String::from_utf8_lossy(b).trim_matches('\0').to_string())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// (年,月,日,时,分,秒) → unix 秒（Howard Hinnant days_from_civil）
fn civil_to_unix(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(0..=23).contains(&h)
        || !(0..=59).contains(&mi) || !(0..=59).contains(&s) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + s)
}

/// unix 秒 → (年,月,日,时,分,秒)
fn unix_to_civil(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i64, m as i64, d as i64, rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// 按 tag 数字匹配字段（跨上下文）
///
/// 真实相机将拍摄参数写在 Exif 子 IFD（上下文 Exif），而部分工具
/// （如 PIL）会写入 IFD0（上下文 Tiff）。`Tag` 枚举包含上下文信息，
/// 直接比较枚举会漏读，这里按数字匹配保证两种来源都能读到。
fn field_by_number<'a>(exif: &'a exif::Exif, num: u16) -> Option<&'a exif::Field> {
    exif.fields().find(|f| f.tag.number() == num)
}

/// 将字段值转为 f64
///
/// 标准相机照片以 Rational（分子/分母）写入；部分工具（如 PIL 直接赋值元组）
/// 会以 Short/Long 二元组 [num, denom] 写入。这里统一兼容两种形态。
fn field_value_f64(f: &exif::Field) -> Option<f64> {
    match &f.value {
        exif::Value::Rational(r) => r.first().map(|r| r.to_f64()),
        exif::Value::SRational(r) => r.first().map(|r| r.to_f64()),
        exif::Value::Float(v) => v.first().map(|x| *x as f64),
        exif::Value::Double(v) => v.first().copied(),
        exif::Value::Short(v) => pair_to_f64(v),
        exif::Value::Long(v) => pair_to_f64(v),
        exif::Value::SShort(v) => pair_to_f64(v),
        exif::Value::SLong(v) => pair_to_f64(v),
        _ => None,
    }
}

/// 整数数组转 f64：二元组视为 [num, denom]，单值视为数值本身
fn pair_to_f64<T: Copy + Into<f64>>(v: &[T]) -> Option<f64> {
    match (v.first(), v.get(1)) {
        (Some(&n), Some(&d)) => Some(n.into() / d.into()),
        (Some(&n), None) => Some(n.into()),
        _ => None,
    }
}

pub(crate) fn is_image_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    IMAGE_EXTS.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
}

fn format_iso(v: u32) -> String {
    v.to_string()
}

/// 焦段格式化：50mm；有小数保留 1 位（如 4.3mm）
fn format_focal(v: f64) -> String {
    if v == v.round() {
        format!("{v:.0}mm")
    } else {
        format!("{v:.1}mm")
    }
}

/// 光圈格式化：f/2.8
fn format_aperture(v: f64) -> String {
    format!("f/{v:.1}")
}

/// 快门速度格式化：
/// - 曝光时间 >= 1s → "30s" / "1.5s"
/// - 曝光时间 < 1s  → "1/200s"（分母四舍五入）
fn format_shutter(v: f64) -> String {
    if v >= 1.0 {
        if v == v.round() {
            format!("{v:.0}s")
        } else {
            format!("{v:.1}s")
        }
    } else {
        let denom = (1.0 / v).round();
        format!("1/{denom:.0}s")
    }
}

/// 拍摄时间格式化：
/// - "2023:01:15 10:30:00"（PIL 写入）→ "2023-01-15 10:30:00"
/// - "2020-02-07 17:37:42"（相机原始，kamadak display_value 已横线日期）→ 原样
/// - "2023-01-15"（仅日期）→ 原样
/// （display_value 对 ASCII 值会带引号，需一并去除）
fn format_shoot_time(s: &str) -> String {
    let s = s.trim().trim_matches('"');
    if s.len() >= 19 {
        // 前 10 字符为日期（'/' ':' 或 '-' 分隔统一为 '-'），后 8 字符为时间（保留 ':'）
        let date = s[..10].replace(':', "-");
        let time = &s[11..19];
        format!("{date} {time}")
    } else {
        s.replace(':', "-")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_shutter() {
        assert_eq!(format_shutter(0.005), "1/200s");
        assert_eq!(format_shutter(0.5), "1/2s");
        assert_eq!(format_shutter(1.0), "1s");
        assert_eq!(format_shutter(30.0), "30s");
    }

    #[test]
    fn test_format_shoot_time() {
        assert_eq!(format_shoot_time("2023:01:15 10:30:00"), "2023-01-15 10:30:00");
        // display_value 的 ASCII 值带引号
        assert_eq!(format_shoot_time("\"2023:01:15 10:30:00\""), "2023-01-15 10:30:00");
        // 已无冒号（如 mtime 等来源）保持原样
        assert_eq!(format_shoot_time("2023-01-15"), "2023-01-15");
        // kamadak 0.6 对相机照片输出日期已横线（时间部分冒号），不应再改时间
        assert_eq!(format_shoot_time("2020-02-07 17:37:42"), "2020-02-07 17:37:42");
    }

    #[test]
    fn test_format_focal_aperture() {
        assert_eq!(format_focal(50.0), "50mm");
        assert_eq!(format_focal(4.3), "4.3mm");
        assert_eq!(format_aperture(2.8), "f/2.8");
    }
}
