/**
 * 中国地图投影（FEAT-103 · P3）
 *
 * 为什么投影逻辑要独立于组件（CS1）：
 *   ① 它是纯数学（经纬度 → SVG 坐标），可单测，不需要 DOM；
 *   ② 同一套投影会被底图渲染、点位定位、标签避让三处共用，
 *      写在组件里早晚会出现"点和底图对不上"的偏移 bug。
 *
 * 为什么用等距圆柱（Plate Carrée）而不是墨卡托：
 *   墨卡托在低纬度几乎不变形、高纬度剧烈放大。中国跨 18°N~54°N，
 *   用墨卡托会让黑龙江占掉近半张图 —— 但这里的地图只用来回答
 *   "哪些城市有照片"，不需要保角/等角。等距圆柱能保证
 *   「经度差 1° → 像素差恒定」，于是**所有城市点位的相对位置一眼可读**，
 *   这正是相册地点视图要的语义。
 */

/** 底图数据的精简结构（由 python/gen_map_lite.py 生成） */
export interface MapLiteProvince {
  /** 省名 */
  n: string;
  /** bbox: [minLon, minLat, maxLon, maxLat] */
  b: [number, number, number, number];
  /** 多边形环数组；每个环是点数组，点为 [lon, lat]。
   *
   * ⚠️ 层级是 3 层（环 → 点 → 坐标），**不是** GeoJSON 的 4 层
   * （MultiPolygon → Polygon → 环 → 点）。生成脚本把 MultiPolygon 完全展平了：
   *   洞环（内环）也当作普通环写入，因为本组件只用一层极淡填充 + 单色描边，
   *   不做 "evenodd" 挖洞 —— 挖洞对 4% 不透明度的轮廓视觉上零收益，
   *   却要多存一层嵌套，并把 path 计算拆成两遍。
   * 这个"层级差"曾在首版实现里写错，导致 `number 117.35 is not iterable`
   * （把 [lon,lat] 当成了环）。类型定义在这里就把它锁死。 */
  r: number[][][];
}

export interface MapLite {
  v: number;
  tolerance: number;
  provs: MapLiteProvince[];
}

export interface Projection {
  /** 经纬度 → 画布坐标 */
  project: (lon: number, lat: number) => [number, number];
  /** 画布尺寸 */
  width: number;
  height: number;
}

/**
 * 按给定画布宽度，等比映射整个中国范围。
 *
 * 用**固定地理包围盒**而不是从数据里现算：
 *   现算会让"只有南方有照片"时地图裁到南方，用户失去"这是全国"的空间参照。
 *   固定 bbox 让地图始终是同一张中国地图，点位多少只影响亮点密度。
 *
 * 数值与中国主陆的实际范围对齐（18.14°N ~ 53.56°N），**不含南海诸岛**：
 *   那份数据（33 省 35 环）已由 gen_map_lite.py 剔除三沙岛礁与插图框。
 *   如果 bbox 仍按 3.4°N 算，下方会出现 300px 的空白海域 —— 画布拉长、
 *   主图只占上方三分之二（首版实测就是这样）。
 * 纬度跨度 35.4°、经度跨度 61.6° → 宽高比 ≈ 1.74（符合中国地图的观感）。
 */
export const CHINA_BBOX = {
  minLon: 73.5,
  maxLon: 135.1,
  minLat: 18.0,
  maxLat: 53.6,
} as const;

export function createProjection(width: number, pad = 0): Projection {
  const lonSpan = CHINA_BBOX.maxLon - CHINA_BBOX.minLon;
  const latSpan = CHINA_BBOX.maxLat - CHINA_BBOX.minLat;
  // 等比：按经度定比例尺，纬度方向的像素高度随之确定（等距圆柱的特性）
  const scale = (width - pad * 2) / lonSpan;
  const height = latSpan * scale + pad * 2;

  const project = (lon: number, lat: number): [number, number] => [
    pad + (lon - CHINA_BBOX.minLon) * scale,
    // Y 轴翻转：纬度越大越靠上
    pad + (CHINA_BBOX.maxLat - lat) * scale,
  ];

  return { project, width, height };
}

/** 经纬度是否落在中国地图范围内（用于过滤境外的 GPS 点，不在地图上瞎打点） */
export function inChina(lon: number, lat: number): boolean {
  return (
    lon >= CHINA_BBOX.minLon &&
    lon <= CHINA_BBOX.maxLon &&
    lat >= CHINA_BBOX.minLat &&
    lat <= CHINA_BBOX.maxLat
  );
}

/**
 * 环 → SVG path 的 d 字符串。
 * 用直折线（L）而不是曲线：轮廓已由 DP 简化到 2.2km，
 * 再上贝塞尔只会让"海岸线"变成一段段软塌塌的弧，失去地图的可信度。
 */
export function ringToPath(ring: number[][], proj: Projection): string {
  if (ring.length === 0) return "";
  const parts: string[] = [];
  for (let i = 0; i < ring.length; i++) {
    const [lon, lat] = ring[i];
    const [x, y] = proj.project(lon, lat);
    parts.push(`${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`);
  }
  return parts.join("") + "Z";
}

/**
 * 光点半径：按张数取**对数**而不是线性。
 *   线性：653 张的城市会是 1 张城市的 653 倍，直接糊成一团；
 *   对数：1→1.0、10→1.44、100→1.88、653→2.23（倍率），
 *   既保住"大城更亮"的直觉，又让稀疏点位仍然可见。
 * 下限 2.6px 保证"只有 1 张照片"也能被看见（否则会被当成没数据）。
 */
export function pointRadius(count: number): number {
  const c = Math.max(1, count);
  return Math.min(13, 2.6 + 2.2 * Math.log10(c) * 1.6);
}
