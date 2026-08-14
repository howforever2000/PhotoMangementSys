# -*- coding: utf-8 -*-
"""中国省/市行政区划边界数据下载与预处理（一次性离线任务，产物入库供本地反查）。

数据源：阿里 DataV.GeoAtlas（免 key、免费、民政部口径、WGS84 经纬度）
  - 全国省级边界: https://geo.datav.aliyun.com/areas_v3/bound/100000_full.json
  - 每省市级边界: https://geo.datav.aliyun.com/areas_v3/bound/{adcode}_full.json

输出：紧凑 JSON（src-tauri/resources/china_geo.json），结构：
  {
    "version": 1,
    "provinces": [ {"adcode":110000,"name":"北京市","bbox":[minLon,minLat,maxLon,maxLat],
                    "polygons":[[[lon,lat],...], ...]} ],   // polygon = 外环，洞环跟随其后
    "cities":   { "130000": [ 同上结构的市级要素 ], ... }     // 仅非直辖省市
  }

流程：下载 → 去连续重复点 → Douglas-Peucker 简化(容差 0.002°≈220m) → 计算 bbox
     → 输出并自检（11 个已知坐标 + 天长飞地 + 市级抽查）。
用法：python download_geo_data.py            # 全流程
      python download_geo_data.py --cache-only   # 只下载原始 JSON 到缓存，不产出
"""
import argparse
import gzip
import io
import json
import math
import os
import sys
import time
import urllib.request

if sys.stdout and hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

BASE = "https://geo.datav.aliyun.com/areas_v3/bound/{}_full.json"
HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_CACHE = os.path.join(HERE, ".geo_cache")
DEFAULT_OUT = os.path.join(HERE, "..", "src-tauri", "resources", "china_geo.json")

# 直辖市：市=省，无需下钻区级；港澳台：市级数据缺失/极简，直接省级
NO_CITY_DRILL = {"110000", "120000", "310000", "500000", "710000", "810000", "820000"}

UA = "Mozilla/5.0 (geo-downloader/0.1 photo-manager)"


def fetch_json(url, retries=3):
    last = None
    for i in range(retries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=30) as r:
                raw = r.read()
            return json.loads(raw.decode("utf-8"))
        except Exception as e:  # noqa: BLE001
            last = e
            time.sleep(1.5 * (i + 1))
    raise RuntimeError(f"下载失败 {url}: {last}")


def fetch_cached(url, cache_dir, name):
    os.makedirs(cache_dir, exist_ok=True)
    path = os.path.join(cache_dir, name)
    if os.path.exists(path):
        with io.open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    data = fetch_json(url)
    with io.open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False)
    print(f"  [ok] {name} ({os.path.getsize(path) / 1024:.0f} KB)")
    return data


def dedupe_ring(ring):
    """去连续重复点 + 保证闭合。"""
    out = []
    for p in ring:
        if not out or out[-1] != p:
            out.append(p)
    if len(out) > 1 and out[0] == out[-1]:
        out.pop()
    if len(out) >= 3:
        out.append(out[0])  # 闭合
    return out


def perp_dist(p, a, b):
    """点到线段 a-b 的垂直距离（经纬度平面近似，容差场景足够）。"""
    ax, ay = a
    bx, by = b
    px, py = p
    dx, dy = bx - ax, by - ay
    if dx == 0 and dy == 0:
        return math.hypot(px - ax, py - ay)
    t = ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)
    t = max(0.0, min(1.0, t))
    cx, cy = ax + t * dx, ay + t * dy
    return math.hypot(px - cx, py - cy)


def dp_simplify(ring, tol):
    """Douglas-Peucker 迭代实现。"""
    if len(ring) < 4:
        return ring
    keep = [False] * len(ring)
    keep[0] = keep[-1] = True
    stack = [(0, len(ring) - 1)]
    while stack:
        i, j = stack.pop()
        if j <= i + 1:
            continue
        dmax, idx = 0.0, i
        for k in range(i + 1, j):
            d = perp_dist(ring[k], ring[i], ring[j])
            if d > dmax:
                dmax, idx = d, k
        if dmax > tol:
            keep[idx] = True
            stack.append((i, idx))
            stack.append((idx, j))
    return [p for p, k in zip(ring, keep) if k]


def simplify_ring(ring, tol):
    r = dedupe_ring(ring)
    r = dp_simplify(r, tol)
    r = dedupe_ring(r)
    # 坐标圆整到 4 位小数（≈11m，省/市级分类无感，体积降 ~30%）
    return [[round(lon, 4), round(lat, 4)] for lon, lat in r]


def process_geometry(geom, tol):
    """把 GeoJSON geometry 转成 [polygon...]，polygon = [外环, 洞环...]。
    保留所有环（洞环用于飞地判空，如安徽天长在江苏界内）。"""
    polys = geom["coordinates"] if geom["type"] == "MultiPolygon" else [geom["coordinates"]]
    out = []
    for poly in polys:
        rings = [simplify_ring(ring, tol) for ring in poly]
        rings = [r for r in rings if len(r) >= 4]
        if rings:
            out.append(rings)
    return out


def calc_bbox(polygons):
    min_lon = min_lat = float("inf")
    max_lon = max_lat = float("-inf")
    for poly in polygons:
        for ring in poly:
            for lon, lat in ring:
                min_lon = min(min_lon, lon)
                max_lon = max(max_lon, lon)
                min_lat = min(min_lat, lat)
                max_lat = max(max_lat, lat)
    return [round(min_lon, 5), round(min_lat, 5), round(max_lon, 5), round(max_lat, 5)]


def to_region(feat, tol):
    p = feat["properties"]
    polygons = process_geometry(feat["geometry"], tol)
    return {
        "adcode": int(p["adcode"]) if str(p.get("adcode", "")).isdigit() else 0,
        "name": p.get("name", ""),
        "bbox": calc_bbox(polygons),
        "polygons": polygons,
    }


# ---------------------------------------------------------------------------
# 自检：射线法验证产物（与 Rust 实现同算法）
# ---------------------------------------------------------------------------

def ray_cross(pt, ring):
    x, y = pt
    inside = False
    j = len(ring) - 1
    for i in range(len(ring)):
        xi, yi = ring[i]
        xj, yj = ring[j]
        if ((yi > y) != (yj > y)) and (x < (xj - xi) * (y - yi) / (yj - yi) + xi):
            inside = not inside
        j = i
    return inside


def in_region(lon, lat, region):
    for poly in region["polygons"]:
        # 逐 polygon 统计所有环交点奇偶：洞内 → 偶数 → 判外（正确处理飞地）
        odd = False
        for ring in poly:
            odd ^= ray_cross((lon, lat), ring)
        if odd:
            return True
    return False


def bbox_hit(lon, lat, region):
    b = region["bbox"]
    return b[0] <= lon <= b[2] and b[1] <= lat <= b[3]


def lookup(lon, lat, data, city_level=True):
    prov = None
    for r in data["provinces"]:
        if bbox_hit(lon, lat, r) and in_region(lon, lat, r):
            prov = r
            break
    if prov is None:
        return None
    city = None
    if city_level:
        cities = data["cities"].get(str(prov["adcode"]))
        if cities:
            for r in cities:
                if bbox_hit(lon, lat, r) and in_region(lon, lat, r):
                    city = r
                    break
    if city:
        return f"{prov['name']} · {city['name']}"
    return prov["name"]


def self_check(data):
    cases = [
        ("北京", 116.4074, 39.9042, "北京市"),
        ("上海", 121.4737, 31.2304, "上海市"),
        ("成都(省)", 104.0657, 30.6593, "四川省 · 成都市"),
        ("拉萨", 91.1721, 29.6520, "西藏自治区 · 拉萨市"),
        ("哈尔滨", 126.5349, 45.8038, "黑龙江省 · 哈尔滨市"),
        ("广州", 113.2644, 23.1291, "广东省 · 广州市"),
        ("乌鲁木齐", 87.6168, 43.8256, "新疆维吾尔自治区 · 乌鲁木齐市"),
        ("台北", 121.5654, 25.0330, "台湾省"),
        ("香港", 114.1694, 22.3193, "香港特别行政区"),
        ("三亚", 109.5119, 18.2528, "海南省 · 三亚市"),
        ("绵阳", 104.68, 31.47, "四川省 · 绵阳市"),
        ("万源(达州)", 108.035, 32.067, "四川省 · 达州市"),
        ("天长(安徽飞地)", 118.998, 32.688, "安徽省 · 滁州市"),  # 洞/飞地测试
        ("南海中部(海里)", 113.0, 17.0, None),
    ]
    print("\n[自检] 已知坐标点：")
    ok = 0
    for name, lon, lat, expect in cases:
        got = lookup(lon, lat, data)
        mark = "✓" if got == expect else "✗"
        if got == expect:
            ok += 1
        print(f"  {mark} {name:14s} 期望[{expect}] 实际[{got}]")
    print(f"[自检] {ok}/{len(cases)} 通过")
    return ok == len(cases)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cache", default=DEFAULT_CACHE)
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--tolerance", type=float, default=0.002, help="DP 简化容差(度)，0.002≈220m")
    ap.add_argument("--cache-only", action="store_true", help="只下载原始数据到缓存")
    args = ap.parse_args()

    t0 = time.time()
    print("[1/4] 下载全国省级边界…")
    prov_data = fetch_cached(f"{BASE.format(100000)}", args.cache, "100000_full.json")

    print(f"[2/4] 下载市级边界（跳过直辖市/港澳台: {sorted(NO_CITY_DRILL)}）…")
    adcodes = set()
    for f in prov_data["features"]:
        a = str(f["properties"].get("adcode", ""))
        if a.isdigit() and len(a) == 6 and a not in NO_CITY_DRILL:
            adcodes.add(a)
    raw_city = {}
    for a in sorted(adcodes):
        raw_city[a] = fetch_cached(BASE.format(a), args.cache, f"{a}_full.json")
    if args.cache_only:
        print("[cache-only] 原始数据已缓存，跳过处理。")
        return

    print(f"[3/4] 预处理（去重 + DP 简化容差 {args.tolerance}°）…")
    tol = args.tolerance
    provinces = []
    for f in prov_data["features"]:
        r = to_region(f, tol)
        if r["polygons"]:
            provinces.append(r)
    cities = {}
    for a, d in raw_city.items():
        cs = []
        for f in d["features"]:
            r = to_region(f, tol)
            if r["polygons"]:
                cs.append(r)
        if cs:
            cities[a] = cs

    data = {"version": 1, "tolerance": tol, "provinces": provinces, "cities": cities}
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with io.open(args.out, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, separators=(",", ":"))

    n_pts = sum(len(r) for p in provinces for poly in p["polygons"] for r in poly)
    n_city_pts = sum(
        len(r) for cs in cities.values() for c in cs for poly in c["polygons"] for r in poly
    )
    raw = os.path.getsize(args.out)
    with io.open(args.out, "rb") as f:
        gz = len(gzip.compress(f.read()))
    print(f"\n[4/4] 产物: {args.out}")
    print(f"  省级 {len(provinces)} 个 / 市级 {sum(len(v) for v in cities.values())} 个（{len(cities)} 省）")
    print(f"  顶点数: 省级 {n_pts:,} / 市级 {n_city_pts:,}")
    print(f"  体积: {raw / 1024:.0f} KB 原始 → {gz / 1024:.0f} KB gzip（{(time.time() - t0):.0f}s）")

    if not self_check(data):
        print("\n[自检失败] 请检查数据/容差参数。")
        sys.exit(1)
    print("\n全部通过 ✅")


if __name__ == "__main__":
    main()
