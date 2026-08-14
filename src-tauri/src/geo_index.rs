//! 本地行政区划反查（离线 · 省/市）
//!
//! 用民政部口径的中国省/市边界数据（阿里 DataV.GeoAtlas，WGS84），
//! 以「bbox 预筛 + 射线法点面判断」把 GPS 坐标解析为 省/市，全程无网络。
//!
//! - 数据：`include_str!` 内嵌 `resources/china_geo.json`（由
//!   `python/download_geo_data.py` 一次性生成，含去重/DP 简化/坐标圆整）
//! - 查询：先在全国 35 个省级面命中省份，再下钻该省市级面（直辖市/港澳台无市级数据，
//!   直接返回省名）；MultiPolygon 逐 polygon 统计全部环交点奇偶，正确处理岛屿与飞地
//! - 缓存：按 0.01°（≈1.1km）网格缓存结果，同地照片批量扫描 O(1) 命中
//! - 性能：~0.05ms/点（万张照片 <1s），相对在线反编码（~2s/张）快 4 个数量级

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

/// 单个省/市边界要素
///
/// `polygons[i]` = 一个多边形（外环 + 洞环），`polygons[i][j]` = 一个环，
/// 环内点为 `[lon, lat]`（GeoJSON 顺序，注意与照片 EXIF 的 (lat, lon) 相反）。
#[derive(Debug, Clone, Deserialize)]
struct Region {
    /// 行政区划码（省级 6 位，市级 6 位）
    #[allow(dead_code)]
    adcode: u32,
    /// 名称，如 "四川省" / "成都市"
    name: String,
    /// [min_lon, min_lat, max_lon, max_lat]
    #[serde(default)]
    bbox: [f64; 4],
    /// 多边形 → 环 → [lon, lat]
    polygons: Vec<Vec<Vec<[f64; 2]>>>,
}

#[derive(Deserialize)]
struct GeoData {
    provinces: Vec<Region>,
    /// 省级 adcode → 市级列表（直辖市/港澳台无条目）
    #[serde(default)]
    cities: HashMap<u32, Vec<Region>>,
}

/// 全局索引：解析一次，之后只读 + 缓存写锁
struct GeoIndex {
    provinces: Vec<Region>,
    cities: HashMap<u32, Vec<Region>>,
    /// 0.01° 网格 key → 结果（None 表示已查过但未命中，避免重复扫描）
    cache: Mutex<HashMap<(i32, i32), Option<String>>>,
}

const GEO_JSON: &str = include_str!("../resources/china_geo.json");

static INDEX: OnceLock<GeoIndex> = OnceLock::new();

fn index() -> &'static GeoIndex {
    INDEX.get_or_init(|| {
        let data: GeoData =
            serde_json::from_str(GEO_JSON).expect("内嵌行政区划数据解析失败（请重跑 python/download_geo_data.py）");
        GeoIndex {
            provinces: data.provinces,
            cities: data.cities,
            cache: Mutex::new(HashMap::new()),
        }
    })
}

/// 网格 key：0.01° ≈ 1.1km
fn grid_key(lat: f64, lon: f64) -> (i32, i32) {
    ((lat * 100.0).round() as i32, (lon * 100.0).round() as i32)
}

/// 把坐标解析为「省」或「省 · 市」地名（本地离线，无网络请求）
///
/// - 命中省级面但该省无市级数据（直辖市/港澳台）→ 返回省名
/// - 未命中任何面（国外 / 南海公海）→ None
/// - 结果按 0.01° 网格缓存
pub fn find_region(lat: f64, lon: f64) -> Option<String> {
    let idx = index();
    let key = grid_key(lat, lon);
    if let Some(hit) = idx.cache.lock().unwrap().get(&key) {
        return hit.clone();
    }
    let result = lookup(idx, lat, lon);
    idx.cache.lock().unwrap().insert(key, result.clone());
    result
}

/// 无缓存查询
fn lookup(idx: &GeoIndex, lat: f64, lon: f64) -> Option<String> {
    let province = idx
        .provinces
        .iter()
        .find(|r| bbox_hit(lon, lat, r) && point_in_region(lon, lat, r))?;
    let city = idx
        .cities
        .get(&province.adcode)
        .and_then(|cs| cs.iter().find(|r| bbox_hit(lon, lat, r) && point_in_region(lon, lat, r)));
    match city {
        Some(c) => Some(format!("{} · {}", province.name, c.name)),
        None => Some(province.name.clone()),
    }
}

fn bbox_hit(lon: f64, lat: f64, r: &Region) -> bool {
    let b = &r.bbox;
    lon >= b[0] && lon <= b[2] && lat >= b[1] && lat <= b[3]
}

/// 点在环内（射线法，even-odd）
fn point_in_ring(lon: f64, lat: f64, ring: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = ring.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (ring[i][0], ring[i][1]);
        let (xj, yj) = (ring[j][0], ring[j][1]);
        if ((yi > lat) != (yj > lat)) && (lon < (xj - xi) * (lat - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 点在省/市区域内（逐 polygon 统计全部环交点奇偶：
/// 洞内交点数为偶 → 判外，正确处理「安徽天长在江苏界内」这类飞地）
fn point_in_region(lon: f64, lat: f64, r: &Region) -> bool {
    r.polygons.iter().any(|poly| {
        let mut odd = false;
        for ring in poly {
            odd ^= point_in_ring(lon, lat, ring);
        }
        odd
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(lat: f64, lon: f64) -> Option<String> {
        find_region(lat, lon)
    }

    #[test]
    fn test_known_cities() {
        assert_eq!(check(39.9042, 116.4074).as_deref(), Some("北京市"));
        assert_eq!(check(31.2304, 121.4737).as_deref(), Some("上海市"));
        assert_eq!(check(30.6593, 104.0657).as_deref(), Some("四川省 · 成都市"));
        assert_eq!(check(29.6520, 91.1721).as_deref(), Some("西藏自治区 · 拉萨市"));
        assert_eq!(check(45.8038, 126.5349).as_deref(), Some("黑龙江省 · 哈尔滨市"));
        assert_eq!(check(23.1291, 113.2644).as_deref(), Some("广东省 · 广州市"));
        assert_eq!(check(43.8256, 87.6168).as_deref(), Some("新疆维吾尔自治区 · 乌鲁木齐市"));
        assert_eq!(check(25.0330, 121.5654).as_deref(), Some("台湾省"));
        assert_eq!(check(22.3193, 114.1694).as_deref(), Some("香港特别行政区"));
        assert_eq!(check(18.2528, 109.5119).as_deref(), Some("海南省 · 三亚市"));
    }

    #[test]
    fn test_enclave_and_miss() {
        // 安徽天长市位于江苏界内（洞处理必须正确）
        assert_eq!(check(32.688, 118.998).as_deref(), Some("安徽省 · 滁州市"));
        // 公海 / 国外
        assert_eq!(check(17.0, 113.0), None);
        assert_eq!(check(48.8566, 2.3522), None); // 巴黎
    }

    #[test]
    fn test_cache_consistency() {
        let a = check(30.66, 104.06).unwrap();
        let b = check(30.66, 104.06).unwrap();
        assert_eq!(a, b);
        assert!(a.contains("成都"));
    }
}
