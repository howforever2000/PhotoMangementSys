<script setup lang="ts">
/**
 * LocationMap —— 中国地图地点分布（FEAT-103 · P3）
 *
 * 这个组件的核心主张：**空即信息**。
 *   UX 评审里有一条："地点功能全线空转，653 张照片 location 全为 NULL，
 *   界面只是安静地展示空态，用户不知道是没 GPS、解析失败、还是没扫描。"
 *   地图正好能把这个"空"表达成**有意义的信息**：地图是完整的，
 *   但上面没有点 —— 用户立刻理解"我的照片没有定位数据"，
 *   而不是"这个功能坏了"。所以：
 *     · 不画占位骨架、不画"暂无数据"图标、不画半透明假点
 *     · 地图本身就是那块"应当有点但没有"的画布
 *
 * 底图为什么是 177KB 的本地 JSON：
 *   `public/china-provinces-lite.json` 由 python/gen_map_lite.py 从
 *   src-tauri/resources/china_geo.json 简化而来（DP 0.02°）。**零网络**，
 *   与项目"照片不出本机"的定位一致，也不受"地图服务要 API key"的限制。
 *
 * 懒加载：底图 177KB + 261 个环的 path 计算，首屏不该承担。
 *   用 IntersectionObserver 在进入视口时才开始解析与绘制。
 */
import { computed, onBeforeUnmount, onMounted, ref, shallowRef } from "vue";
import {
  createProjection,
  inChina,
  pointRadius,
  ringToPath,
  type MapLite,
} from "../../utils/mapProjection.ts";

/** 地点聚合（与后端 LocationGroupRow 对应；P3 已补 lat/lon） */
export interface LocationPoint {
  location: string | null;
  count: number;
  lat: number | null;
  lon: number | null;
  cover_path?: string | null;
  cover_album_id?: number | null;
}

const props = withDefaults(
  defineProps<{
    /** 地点聚合行（来自 list_photo_locations） */
    groups: LocationPoint[];
    /** 画布宽度（px）；高度由投影等比决定 */
    width?: number;
    /** 已选中的地点名（高亮） */
    activeName?: string | null;
  }>(),
  { width: 640, activeName: null },
);

const emit = defineEmits<{
  (e: "pick", g: LocationPoint): void;
}>();

const host = ref<HTMLElement | null>(null);
const lite = shallowRef<MapLite | null>(null);
const loadState = ref<"idle" | "loading" | "ready" | "error">("idle");
const loadError = ref("");
/** 悬停的城市名（用于显示 tooltip；比常驻标签省空间） */
const hoverName = ref<string | null>(null);

const projection = computed(() => createProjection(props.width, 0));

/**
 * 有坐标、且在中国范围内的点位才上图。
 * 境外坐标（少数照片可能带国外 GPS）不在这张地图上——画在边缘会造成误解，
 * 它们由下方的统计行单独说明。
 *
 * 标签避让（dodge）：长三角、珠三角这类城市群在地图上相隔不到 20px，
 * 直接给每个点都画标签会叠成一团字（首版实测上海与杭州完全叠死）。
 * 策略分两级：
 *   ① 默认只给张数 ≥10 的点画标签（稀疏区域不必堆字）；
 *   ② 其中若两个标签的包围盒相交，**张数少的那一个**不画常驻标签，
 *      但悬停时仍然显示（hoverName 分支，见模板）。
 * 这比“永远不画小标签”好：用户在稀疏区一样能看到小城名字。
 */
const points = computed(() => {
  const proj = projection.value;
  const raw: Array<LocationPoint & { x: number; y: number; r: number; label: string }> = [];
  for (const g of props.groups) {
    if (g.lat == null || g.lon == null) continue;
    if (!inChina(g.lon, g.lat)) continue;
    const [x, y] = proj.project(g.lon, g.lat);
    raw.push({
      ...g,
      x,
      y,
      r: pointRadius(g.count),
      label: (g.location ?? "未记录").replace(/[省市]$/, ""),
    });
  }
  // 张数多的先画，少的后画 —— 否则大点会盖住小点
  raw.sort((a, b) => b.count - a.count);

  // —— 标签避让 ——
  // 估算标签包围盒：中文约 1em 宽/字，加上数字与空格。取 10px 字号。
  const boxes: Array<{ x1: number; y1: number; x2: number; y2: number }> = [];
  const withLabel = new Set<string>();
  for (const p of raw) {
    if (p.count < 10) continue;
    const text = `${p.label} ${p.count}`;
    const w = text.length * 9 + 6;
    const box = { x1: p.x + p.r + 4, y1: p.y - 8, x2: p.x + p.r + 4 + w, y2: p.y + 8 };
    const clash = boxes.some((b) => box.x1 < b.x2 && box.x2 > b.x1 && box.y1 < b.y2 && box.y2 > b.y1);
    if (clash) continue;
    boxes.push(box);
    withLabel.add(p.location ?? "unknown");
  }
  return raw.map((p) => ({ ...p, showLabel: withLabel.has(p.location ?? "unknown") }));
});

/** 省轮廓的 path 列表：一次性算好并缓存（依赖底图与投影） */
const provincePaths = computed<string[]>(() => {
  const data = lite.value;
  if (!data) return [];
  const proj = projection.value;
  const out: string[] = [];
  // 层级：provs → r（环数组）→ 环（点数组）→ 点 [lon,lat]
  for (const p of data.provs) {
    for (const ring of p.r) {
      const d = ringToPath(ring, proj);
      if (d) out.push(d);
    }
  }
  return out;
});

const viewBox = computed(
  () => `0 0 ${projection.value.width.toFixed(0)} ${projection.value.height.toFixed(0)}`,
);

/** 只在进入视口后加载底图（首屏不为一张图付费） */
async function load() {
  if (loadState.value !== "idle") return;
  loadState.value = "loading";
  try {
    const res = await fetch("/china-provinces-lite.json");
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = (await res.json()) as MapLite;
    if (!Array.isArray(data.provs) || data.provs.length === 0) {
      throw new Error("底图数据为空");
    }
    lite.value = data;
    loadState.value = "ready";
  } catch (e) {
    loadState.value = "error";
    loadError.value = e instanceof Error ? e.message : String(e);
    // 底图失败不阻塞点位信息：下方统计与卡片视图仍然可用
  }
}

let io: IntersectionObserver | null = null;

onMounted(() => {
  if (!host.value) return;
  if (typeof IntersectionObserver === "undefined") {
    void load();
    return;
  }
  io = new IntersectionObserver(
    (entries) => {
      if (entries.some((en) => en.isIntersecting)) {
        void load();
        io?.disconnect();
        io = null;
      }
    },
    { rootMargin: "200px" },
  );
  io.observe(host.value);
});

onBeforeUnmount(() => {
  io?.disconnect();
  io = null;
});
</script>

<template>
  <div ref="host" class="u-map" :style="{ maxWidth: `${width}px` }">
    <svg
      :viewBox="viewBox"
      class="u-map-svg"
      role="img"
      :aria-label="`中国地图，${points.length} 个城市有照片定位`"
    >
      <!-- 省轮廓：只做极淡暗示（4% 白），它不该抢点位的视觉重量 -->
      <g class="u-provinces">
        <path v-for="(d, i) in provincePaths" :key="i" :d="d" />
      </g>

      <!-- 光点：金色，半径按张数对数增长 -->
      <g class="u-points">
        <g
          v-for="p in points"
          :key="p.location ?? 'unknown'"
          class="u-point"
          :class="{ on: p.location === activeName, hover: hoverName === p.location }"
          @mouseenter="hoverName = p.location"
          @mouseleave="hoverName = null"
          @click="emit('pick', p)"
        >
          <circle :cx="p.x" :cy="p.y" :r="p.r * 2.6" class="halo" />
          <circle :cx="p.x" :cy="p.y" :r="p.r" class="dot" />
          <text
            v-if="p.showLabel || hoverName === p.location"
            :x="p.x + p.r + 5"
            :y="p.y + 3.5"
            class="lbl"
          >
            {{ p.label }} {{ p.count }}
          </text>
        </g>
      </g>
    </svg>

    <!-- 加载 / 失败提示：不遮挡地图区域，压在下方 -->
    <p v-if="loadState === 'loading'" class="u-map-hint">正在载入底图…</p>
    <p v-else-if="loadState === 'error'" class="u-map-hint err">
      底图加载失败（{{ loadError }}），点位信息仍可在下方查看
    </p>
    <p v-else-if="loadState === 'ready' && points.length === 0" class="u-map-hint">
      地图上没有点位 —— 照片未记录 GPS 定位
    </p>
  </div>
</template>

<style scoped>
.u-map {
  position: relative;
  width: 100%;
}

.u-map-svg {
  display: block;
  width: 100%;
  height: auto;
  overflow: visible;
}

/* 省级轮廓：4% 白 + 6% 白填充。
   为什么要填充而不是纯描边：纯描边时中国版图只是一堆线，缺少"这是一块陆地"
   的体感；一层几乎看不见的填充能把各省连成整块形状。
   为什么是 4%：再深就变成"地图控件"，而这页的主角是照片不是地图。 */
.u-provinces path {
  fill: rgba(255, 255, 255, 0.06);
  stroke: rgba(255, 255, 255, 0.04);
  stroke-width: 1;
  /* 相邻省会共用边，抗锯齿会让接缝显出一条更深的线（"描边加深"错觉）。
     shape-rendering 在这里不做优化 —— 保留抗锯齿，接缝更柔和。 */
}

.u-point {
  cursor: pointer;
}

.u-point .halo {
  fill: var(--c-accent);
  opacity: 0.12;
  transition: opacity var(--dur-2) var(--ease-out);
}

.u-point .dot {
  fill: var(--c-accent);
  transition:
    opacity var(--dur-2) var(--ease-out),
    r var(--dur-2) var(--ease-both);
}

.u-point.hover .halo,
.u-point.on .halo {
  opacity: 0.26;
}
.u-point.hover .dot,
.u-point.on .dot {
  fill: #fff;
}

.u-point .lbl {
  font-size: 10px;
  fill: var(--c-fg-2);
  letter-spacing: 0.02em;
  /* 标签不加描边/阴影：地图已经很淡，加描边会让它变成"图表标注" */
  pointer-events: none;
  user-select: none;
}

.u-map-hint {
  margin: 6px 0 0;
  font-size: var(--fs-meta);
  color: var(--c-fg-2);
  letter-spacing: var(--ls-meta);
}
.u-map-hint.err {
  color: var(--c-danger);
}

/* 浅色：省轮廓换暗色系，否则 6% 白在浅底上完全不可见 */
body:not(.theme-dark) .u-provinces path {
  fill: rgba(23, 22, 26, 0.05);
  stroke: rgba(23, 22, 26, 0.06);
}

/* 降级：系统要求减少动态时不放大光晕 */
@media (prefers-reduced-motion: reduce) {
  .u-point .halo,
  .u-point .dot {
    transition: none;
  }
}
</style>
