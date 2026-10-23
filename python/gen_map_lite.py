# -*- coding: utf-8 -*-
"""生成前端地图底图（FEAT-103 · P3）

用途：`src/components/ui/LocationMap.vue` 绘制地点分布所需的省级轮廓。

为什么不直接用 `src-tauri/resources/china_geo.json`（2.89MB）：
  那份数据包含**市级**边界 + 0.002° 精度（≈220m），是给「经纬度 → 行政区
  反查」用的 —— 点在多边形内的判断要求边界精确。前端画图不需要那个精度：
  屏幕上一个省宽约 60px，220m 的边界抖动完全不可见，但会让 SVG 路径点数
  涨 10 倍，直接拖垮首帧。

简化策略（三档，见实测）：
  tolerance 0.02°（≈2.2km）→ 177KB  ← 采用。省级轮廓仍可辨识海岸线与半岛
  tolerance 0.05°（≈5.5km）→  69KB    胶州湾/雷州半岛会开始变形
  tolerance 0.1° （≈11km） →  36KB    已经不是"地图"了，是色块

输出：`public/china-provinces-lite.json`
  结构为「极简字段」：n=名称 / b=bbox / r=rings（多边形环数组）
  字段名压到一个字母 —— 这份文件只被一个组件消费，可读性让位于体积；
  真正的语义说明在本注释与 consumer 的 TS 类型里。

用法：python python/gen_map_lite.py
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(HERE, "..", "src-tauri", "resources", "china_geo.json")
OUT = os.path.join(HERE, "..", "public", "china-provinces-lite.json")

sys.path.insert(0, HERE)
# 复用生成原始数据时同一个 DP 实现，避免两套简化算法产生"边界对不上"的问题
from download_geo_data import dedupe_ring, dp_simplify  # noqa: E402

TOLERANCE = 0.02   # 度
DECIMALS = 2       # 坐标小数位：0.01° ≈ 1.1km，配合上面的容差足够

# 纬度下限（度）：排除南海诸岛的“插图框”。
#
# 阿里 DataV 的全国数据把南海诸岛作为一个独立要素（名称字段为空），
# 本意是给传统中国地图的右下角插图区用。它是“数据地图”不该有的东西：
# 插一个插图框会把纬度范围从 18°N 拉到 3.4°N，画布高度凭空多 35%，
# 主图只占上半部（实测首版就是这样）。
#
# 判定不能用 bbox：海南省与插图框**都包含三沙市岛礁**，两者的 bbox 下界
# 与中心点纬度都偏低（海南中心 12.0°N、插图框 14.0°N），任何基于 bbox
# 的阀值要么误删海南、要么误留插图框（两种都踩过）。
#
# 改用**面积最大的环的中心纬度**：海南本岛是一个面积远大于单个岛礁的环，
# 其中心 ≈ 19°N；而插图框里最大的环也只是一组岛礁，中心 < 12°N。
# 这个指标能干净地区开两者，且不依赖名称（名称是可选字段）。
MIN_MAIN_RING_LAT = 17.0

# 环面积下限（度²）：剔除三沙岛礁这类碎环。
#
# 为什么必须剔：海南省与插图框里共有 116 个环落在 17°N 以下，它们合计只占
# 550 个点（占全图 4.5%），却把纬度下界从 18°N 拉到 3.4°N —— 画布高度凭空多
# 35%，主图只占上方三分之二。而它们在地图上只有 1~2px 大，看不清、也无法点击
# （三沙市常驻人口极少，相册 GPS 命中概率接近 0）。
#
# 阀值取 0.02°²（≈240km²）：比它大的环保留（含海南本岛、崇明岛、舟山主岛），
# 比它小的一律剔除（岛礁、沙洲）。
MIN_RING_AREA = 0.09

# 环的纬度下界（度）：剔除主陆以外的离岛环（永暑礁等）。
# 与 MIN_RING_AREA 配合使用（两者捕不同对象，见用法处注释）。
# 17.5°N 是海南本岛最南端（18.1°N）与永暑礁（7.5°N）之间的大片空白，
# 且中国主陆（含海南岛、广东离岛）全部高于此值 —— 取它不会误删任何城市的照片。
MIN_RING_LAT = 17.5


def _ring_area(ring):
    """鞋带公式算环面积（度²，非真实面积，只用做相对比较）。"""
    a = 0.0
    for i in range(len(ring)):
        x1, y1 = ring[i]
        x2, y2 = ring[(i + 1) % len(ring)]
        a += x1 * y2 - x2 * y1
    return abs(a) / 2


def _main_ring_lat(prov):
    """返回该要素中面积最大的环的平均纬度；无环则返回 -90。"""
    best, best_lat = 0.0, -90.0
    for poly in prov["polygons"]:
        for ring in poly:
            if len(ring) < 3:
                continue
            area = _ring_area(ring)
            if area > best:
                best = area
                best_lat = sum(pt[1] for pt in ring) / len(ring)
    return best_lat


def build():
    with open(SRC, encoding="utf-8") as f:
        raw = json.load(f)

    provs = []
    total_pts = 0
    skipped = []
    for p in raw["provinces"]:
        # 剔除南海诸岛插图框（见 MIN_MAIN_RING_LAT 注释）
        if _main_ring_lat(p) < MIN_MAIN_RING_LAT:
            skipped.append(p["name"] or "(空名·南海诸岛)")
            continue
        rings = []
        for poly in p["polygons"]:
            for ring in poly:
                r = dedupe_ring(ring)
                r = dp_simplify(r, TOLERANCE)
                r = dedupe_ring(r)
                if len(r) < 3:      # 简化后退化成线的环直接丢弃（无法填充）
                    continue
                # 剔除碎环：①面积太小 —— 看不清也点不到；②纬度低于主陆 —— 会把画布拉长。
                #
                # 两个条件都要，因为它们捕的是不同的东西：
                #   面积阀值捕到 115 个三沙小岛礁（<0.01°²）；
                #   纬度阀值捕到剩下那个 0.11°² 的永暑礁一带（面积不小但位于 7.5°N）。
                # 只靠面积会漏掉后者，只靠纬度会把广东/广西的离岛误删。
                lats = [pt[1] for pt in r]
                if _ring_area(r) < MIN_RING_AREA or min(lats) < MIN_RING_LAT:
                    continue
                rings.append([[round(x, DECIMALS) for x in pt] for pt in r])
                total_pts += len(r)
        if not rings:
            continue
        provs.append({"n": p["name"], "b": p["bbox"], "r": rings})

    data = {
        "v": 1,
        "tolerance": TOLERANCE,
        "provs": provs,
    }
    return data, total_pts, skipped


def main():
    data, pts, skipped = build()
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8") as f:
        # separators 去空格；ensure_ascii=False 让省名直存（体积更小且便于排错）
        json.dump(data, f, separators=(",", ":"), ensure_ascii=False)

    size_kb = os.path.getsize(OUT) / 1024
    print(f"✅ {OUT}")
    print(f"   省份 {len(data['provs'])} 个 / 环 {sum(len(p['r']) for p in data['provs'])} 个 / 点 {pts:,}")
    print(f"   体积 {size_kb:.1f} KB")
    if skipped:
        print(f"   已剔除：{', '.join(skipped)}（插图框，会拉长画布）")


if __name__ == "__main__":
    main()
