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
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
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
    /**
     * 画布宽度（px）；高度由投影等比决定。
     *
     * 传 0（或不传）则按**容器实测宽度**自适应 —— 旧调用方写死 :width="820"，
     * 在 1440 屏上只占中间一半，两边各留 300px 空白（实测）。
     * 写死一个像素宽度永远只能对上一个窗口尺寸。
     */
    width?: number;
    /** 已选中的地点名（高亮） */
    activeName?: string | null;
  }>(),
  { width: 0, activeName: null },
);

const emit = defineEmits<{
  (e: "pick", g: LocationPoint): void;
}>();

const host = ref<HTMLElement | null>(null);
/** SVG 根元素（screenToCanvas 与拖拽换算需要它拿 CTM） */
const svgEl = ref<SVGSVGElement | null>(null);
const lite = shallowRef<MapLite | null>(null);
const loadState = ref<"idle" | "loading" | "ready" | "error">("idle");
const loadError = ref("");
/** 悬停的城市名（用于显示 tooltip；比常驻标签省空间） */
const hoverName = ref<string | null>(null);

/* ── 自适应宽度 ──────────────────────────────────────────────────────
   写死 px 宽度只能对上一个窗口尺寸。这里量容器（ResizeObserver）。
   量不到时（首帧）给 820 作保守值，避免 0 宽算出坏 path。
   ⚠️ 必须声明在 projection 之前 —— computed 虽然惰性，
      但同层 watch / 首帧渲染会提前求值，跨 TDZ 会直接抛错（踩过）。 */
const canvasWidth = ref(props.width || 820);
let widthRO: ResizeObserver | null = null;

function measureWidth() {
  if (props.width > 0) return; // 调用方指定了固定宽度
  const w = host.value?.clientWidth ?? 0;
  if (w > 0 && Math.abs(w - canvasWidth.value) > 1) canvasWidth.value = w;
}

const projection = computed(() => createProjection(canvasWidth.value, 0));

/* ══════════════════════════════════════════════════════════════════════════
   FEAT-107：Ctrl + 滚轮缩放 / 拖拽平移
   ────────────────────────────────────────────────────────────────────────
   为什么是「Ctrl + 滚轮」而不是「裸滚轮」：
     裸滚轮在地图上缩放会**劫持页面滚动** —— 用户想往下看地点卡片，
     手指一动地图就缩进去了，而且滚不动页面。地图占半屏时这是硬伤。
     Ctrl + 滚轮既符合「浏览器里缩放整页」的肌肉记忆，
     又保证不按 Ctrl 时滚轮属于页面。

   实现方式：在 SVG 里用一个 <g :transform> 做「视图变换」，
   而不是改 viewBox ——
     · viewBox 变化会触发整个 SVG 重新布局，261 个省的 path 都要重新算屏幕坐标；
     · <g> 的 transform 是合成层变换，不触发重排，缩放跟手。
   代价是 stroke-width 会跟着一起放大（线变粗），所以给 stroke 加
   vector-effect: non-scaling-stroke 抵消 —— 缩放后轮廓线仍是 1px。

   缩放中心：以**光标位置**为不动点（zoom at cursor），而不是画布中心。
   数学上：光标在画布坐标 (mx,my)，当前变换 (k, tx, ty)，
   要求缩放后该点仍落同一屏幕位置：
       mx = k'·p + tx'   且   mx = k·p + tx
   ⇒  tx' = mx - k'/k · (mx - tx)
   这就是下方 zoomAt() 里那两行。
   ══════════════════════════════════════════════════════════════════════════ */

/** 当前缩放倍数（1 = 全局视图） */
const scale = ref(1);
/** 当前平移量（画布坐标，已包含缩放） */
const tx = ref(0);
const ty = ref(0);

const MIN_SCALE = 1;
/**
 * 上限 6。
 *
 * 曾经定 12，实测两个理由降回来：
 *   ① 底图是 DP 简化到 2.2km 的省级轮廓（china-provinces-lite.json），
 *      放 12 倍时 1 度经度 ≈ 7px，但轮廓本身只有省级精度 ——
 *      看到的是一条被拉粗的折线，不是“更详细的地图”（错的期待）；
 *   ② 6 倍时长三角/珠三角的城市点已经能完全分开（这正是缩放的目的），
 *      再大就只剩空白区域。
 * 如果以后换了更精细的底图（或接入市级轮廓），改这一个值即可。
 */
const MAX_SCALE = 6;
const WHEEL_STEP = 1.0015; // 每 1px 滚动量对应的指数系数

const isZoomed = computed(() => scale.value > 1.001);

/** SVG 上的视图变换 */
const viewTransform = computed(
  () => `translate(${tx.value.toFixed(2)} ${ty.value.toFixed(2)}) scale(${scale.value.toFixed(4)})`,
);

/**
 * 把缩放比例夹到 [MIN, MAX] 并按光标为不动点重算平移。
 *
 * mx/my 是**画布坐标系**里的光标位置（与 project() 输出同一坐标系），
 * 不是屏幕像素 —— 调用方负责换算。
 */
function zoomAt(nextScale: number, mx: number, my: number) {
  const k = scale.value;
  const k2 = Math.min(MAX_SCALE, Math.max(MIN_SCALE, nextScale));
  if (k2 === k) return;
  // 不动点公式：tx' = mx - (k2/k)·(mx - tx)
  const ratio = k2 / k;
  tx.value = mx - ratio * (mx - tx.value);
  ty.value = my - ratio * (my - ty.value);
  scale.value = k2;
  // 缩回 1 倍时把平移归零，避免留下浮点残余导致「看起来还在偏移」
  if (k2 === MIN_SCALE) resetPan();
}

function resetPan() {
  tx.value = 0;
  ty.value = 0;
}

function resetView() {
  scale.value = 1;
  resetPan();
}

/** 限制平移范围：地图不能被拖出视口（最多拖到边缘对齐） */
function clampPan() {
  const proj = projection.value;
  const k = scale.value;
  if (k <= MIN_SCALE) return resetPan();
  // 放大后内容的可视尺寸 = 原尺寸 × k；可平移范围 = 溢出量
  const overflowX = proj.width * (k - 1);
  const overflowY = proj.height * (k - 1);
  tx.value = Math.min(0, Math.max(-overflowX, tx.value));
  ty.value = Math.min(0, Math.max(-overflowY, ty.value));
}

/**
 * 屏幕坐标 → **根坐标系**（未应用 viewTransform）。
 *
 * getScreenCTM() 返回的是 svg 根坐标系到屏幕的矩阵，**不包含**里面 <g> 的 transform
 * —— 正好是我们要的那个坐标系：省轮廓 path 里的坐标就在这一层。
 */
function screenToCanvas(clientX: number, clientY: number): [number, number] {
  const svg = svgEl.value;
  if (!svg) return [0, 0];
  const pt = svg.createSVGPoint();
  pt.x = clientX;
  pt.y = clientY;
  const ctm = svg.getScreenCTM();
  if (!ctm) return [0, 0];
  const p = pt.matrixTransform(ctm.inverse());
  return [p.x, p.y];
}

/**
 * 滚轮：只有按着 Ctrl（或 ⌘）才缩放，否则**完全不处理**。
 * 必须用 passive: false 才能 preventDefault —— 但只在本组件真的处理时才调，
 * 不按 Ctrl 时立刻 return，页面滚动不受影响。
 */
function onWheel(e: WheelEvent) {
  if (!e.ctrlKey && !e.metaKey) return;
  // 阻止浏览器把「Ctrl+滚轮」解释成整页缩放
  e.preventDefault();
  // zoomAt 的 mx/my 要的是**根坐标系**里的光标位置（与 tx/ty 同一坐标系）
  const [mx, my] = screenToCanvas(e.clientX, e.clientY);
  zoomAt(scale.value * Math.pow(WHEEL_STEP, -e.deltaY), mx, my);
  clampPan();
}

/* ── 拖拽平移：只有放大之后才启用（1 倍时拖动无意义，还会与点击冲突）──
   ⚠️ 曾经的真 bug（用户报「地图放大之后点击就不跳转了」）：
     pointerdown 立刻 setPointerCapture(svg) → **click 事件也被重定向到 svg**
     （按 Pointer Events 规范，捕获期间指针事件及其后续 click 都发给捕获元素），
     于是 <g class="u-point"> 上的 @click 永远收不到 → 点城市没反应。
   修法：不在 pointerdown 就捕获，而是**移动超过阈值才算拖拽**：
     · 移动 < DRAG_THRESHOLD：不捕获、不使能 dragging → 让 click 正常落到光点上
     · 移动 ≥ 阈值：此时才 setPointerCapture 并进入拖拽
   这样“点一下”与“拖一下”被彻底分开，不依赖任何时序巧合。 */
const DRAG_THRESHOLD = 4;
const dragging = ref(false);
let dragStart = { x: 0, y: 0, tx: 0, ty: 0 };
/** 已按下但还没确定是“点”还是“拖” */
let pressing = false;
let pressStart = { x: 0, y: 0 };

function onPointerDown(e: PointerEvent) {
  if (!isZoomed.value) return;
  if (e.button !== 0) return;
  pressing = true;
  pressStart = { x: e.clientX, y: e.clientY };
  dragStart = { x: e.clientX, y: e.clientY, tx: tx.value, ty: ty.value };
}

function onPointerMove(e: PointerEvent) {
  if (!pressing && !dragging.value) return;
  /* 超过阈值才升级为拖拽（并把指针捕获到 svg 上，保证拖出边界也能继续） */
  if (!dragging.value) {
    const dist = Math.hypot(e.clientX - pressStart.x, e.clientY - pressStart.y);
    if (dist < DRAG_THRESHOLD) return;
    dragging.value = true;
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
  }
  const svg = svgEl.value;
  if (!svg) return;
  const ctm = svg.getScreenCTM();
  if (!ctm) return;
  /* 屏幕位移 → 根坐标系位移：除以 CTM 的缩放分量（页面缩放/窗口 DPI）。
     ⚠️ 这里**不再乘 scale** —— CTM 只到根坐标系，<g> 的 scale 由 tx/ty 自己管。
     之前乘了一次，导致放大后拖动速度是手指的两倍多（快得无法使用）。 */
  const k = ctm.a || 1;
  tx.value = dragStart.tx + (e.clientX - dragStart.x) / k;
  ty.value = dragStart.ty + (e.clientY - dragStart.y) / k;
  clampPan();
}

function onPointerUp(e: PointerEvent) {
  pressing = false;
  if (!dragging.value) return;
  dragging.value = false;
  (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
}

/* ── 键盘可达性：Ctrl+滚轮不是所有人都能用（触控 / 键盘用户）──
   ⚠️ 监听必须挂在**宿主 div** 上而不是 svg：
   svg 的 tabindex 虽然有，但键盘焦点在 drag 或点击后会跳到子元素/丢失，
   keydown 就收不到了。宿主 div 带 tabindex 后自己就是焦点容器，
   事件一定落到它身上。（实测：挂在 svg 上时按 0 / + / - 全部无效） */
function onKeydown(e: KeyboardEvent) {
  if (!isZoomed.value) return;
  const proj = projection.value;
  const cx = proj.width / 2;
  const cy = proj.height / 2;
  if (e.key === "+" || e.key === "=") {
    e.preventDefault();
    zoomAt(scale.value * 1.35, cx, cy);
    clampPan();
  } else if (e.key === "-" || e.key === "_") {
    e.preventDefault();
    zoomAt(scale.value / 1.35, cx, cy);
    clampPan();
  } else if (e.key === "0" || e.key === "Home") {
    e.preventDefault();
    resetView();
  }
}

/** 换城市/换数据时重置视图（否则新点位可能落在视口外，看着像“没数据”） */
watch(
  () => props.groups,
  () => resetView(),
);

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
/**
 * 光点在屏幕上的**固定像素**半径。
 *
 * 为什么不能直接用 pointRadius() 的返回值当 r：
 *   那个值是「地图坐标」（1 倍视图下的像素），套上 <g scale(k)> 后
 *   到 12 倍就会变成 12 倍大 —— 一个 13px 的点变成 156px 的色斑，
 *   整张地图只剩一团黄（实测截图确认）。
 *   地图缩放时，点位应该“位置拉开”而不是“自身变大”——
 *   和地图应用里的 pin 一样，pin 始终是同尺寸的图形。
 *
 * 所以：r_screen 固定（可随 k 微调以避让密集区域），
 * r_svg = r_screen / k，由缩放抵消。
 */
const SCREEN_R = 3.2;

const points = computed(() => {
  const proj = projection.value;
  const k = scale.value;
  const raw: Array<LocationPoint & { x: number; y: number; r: number; label: string }> = [];
  for (const g of props.groups) {
    if (g.lat == null || g.lon == null) continue;
    if (!inChina(g.lon, g.lat)) continue;
    const [x, y] = proj.project(g.lon, g.lat);
    // pointRadius 取对数反映真实张数差异，但结果要除以 k 抵消缩放
    const screenR = Math.max(SCREEN_R, pointRadius(g.count) * 0.42);
    raw.push({
      ...g,
      x,
      y,
      r: screenR / k,
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

/** clipPath 的 id（同一页面可能有多张地图，不能写死字符串） */
const clipId = `u-map-clip-${Math.random().toString(36).slice(2, 9)}`;

/** 剪裁矩形 = 整个画布（放大后的溢出部分被剪掉） */
const clipRect = computed(() => ({
  x: 0,
  y: 0,
  w: projection.value.width,
  h: projection.value.height,
}));

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

  /* 宽度自适应：容器宽度变（窗口缩放 / 左栏折展）时重新投影 */
  measureWidth();
  if (typeof ResizeObserver !== "undefined" && props.width <= 0) {
    widthRO = new ResizeObserver(() => measureWidth());
    widthRO.observe(host.value);
  }

  /* wheel 必须用 addEventListener 显式注册 passive:false。
     理由：Vue 的 @wheel 不保证非 passive；而 passive 监听里调 preventDefault
     会被浏览器忽略并报警 —— 结果就是「Ctrl+滚轮缩放地图」同时把整个页面也缩了。 */
  svgEl.value?.addEventListener("wheel", onWheel, { passive: false });

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
  svgEl.value?.removeEventListener("wheel", onWheel);
  widthRO?.disconnect();
  widthRO = null;
  io?.disconnect();
  io = null;
});
</script>

<template>
  <div
    ref="host"
    class="u-map"
    tabindex="0"
    :style="props.width > 0 ? { maxWidth: `${props.width}px` } : undefined"
    @keydown="onKeydown"
  >
    <!--
      交互层（FEAT-107）挂在 svg 上而不是外层 div：
        · wheel 需要 passive:false 才能 preventDefault，而 Vue 的 @wheel 默认不保证
          非 passive —— 这里显式用 addEventListener 注册（onMounted）
        · 拖拽用 pointer capture，挂在 svg 上才能拿到稳定的坐标系（getScreenCTM）
      本模板区只负责画，事件在 script 里统一注册。
    -->
    <svg
      ref="svgEl"
      :viewBox="viewBox"
      class="u-map-svg"
      :class="{ zoomed: isZoomed, dragging }"
      role="img"
      :aria-label="`中国地图，${points.length} 个城市有照片定位`"
      @pointerdown="onPointerDown"
      @pointermove="onPointerMove"
      @pointerup="onPointerUp"
      @pointercancel="onPointerUp"
      @dblclick="resetView"
    >
      <!--
        一个 <g> 承担全部视图变换（缩放 + 平移）。
        为什么不用改 viewBox：viewBox 变化会触发整个 SVG 重新布局，
        261 个省的 path 都要重新算屏幕坐标；<g> 的 transform 是合成层变换，
        不触发重排，缩放跟手。
        代价：stroke-width 会跟着放大 → 见样式里的 vector-effect。
      -->
      <!--
        剪裁用 clipPath 而不是 svg { overflow: hidden }：
        后者会把 svg 画布本身剪成 viewBox 的宽高比，父容器比它宽时
        两边各留一大块空白（实测）。clipPath 只限制**内容**，不影响占位。
      -->
      <defs>
        <clipPath :id="clipId">
          <rect
            :x="clipRect.x"
            :y="clipRect.y"
            :width="clipRect.w"
            :height="clipRect.h"
          />
        </clipPath>
      </defs>

      <g :transform="viewTransform" :clip-path="`url(#${clipId})`">
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
              v-if="(p.showLabel || hoverName === p.location) && scale < 4"
              :x="p.x + p.r + 5"
              :y="p.y + 3.5"
              class="lbl"
              :style="{ '--s': 1 / scale }"
            >
              {{ p.label }} {{ p.count }}
            </text>
          </g>
        </g>
      </g>
    </svg>

    <!-- 缩放控件：只在放大后出现（不放大时它是一块没用的 UI） -->
    <div v-if="isZoomed" class="u-map-zoom">
      <span class="u-map-scale">{{ scale.toFixed(1) }}×</span>
      <button type="button" title="还原视图（双击地图 / 按 0）" @click="resetView">
        还原
      </button>
    </div>

    <!-- 交互提示：只在未放大时提示一次，不常驻（常驻就是噪音） -->
    <p v-if="!isZoomed && loadState === 'ready' && points.length > 0" class="u-map-zoomhint">
      Ctrl + 滚轮缩放
    </p>

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
  outline: none;
}
/* 键盘焦点环只给容器（svg 不再自己拿焦点） */
.u-map:focus-visible {
  outline: 1.5px solid var(--c-accent);
  outline-offset: 3px;
  border-radius: 4px;
}

.u-map-svg {
  display: block;
  width: 100%;
  height: auto;
  /* 放大后地图会超出画布 —— 必须剪裁，否则省轮廓会画到数据带/壁纸上面。
     ⚠️ 不要用 overflow:hidden 剪：svg 的 overflow:hidden 会把**画布本身**
     剪成 viewBox 的宽高比，父容器比它宽时两边各留一大块空白（实测）。
     改用 clipPath（见模板 <defs>）—— 只限制**内容**的活动范围，
     不影响 svg 自身占位。 */
  border-radius: 4px;
  /* 放大后可拖拽，光标提示可抓；双击还原 */
  touch-action: none;
}
.u-map-svg:focus-visible {
  outline: none;
}
.u-map-svg.zoomed {
  cursor: grab;
}
.u-map-svg.zoomed.dragging {
  cursor: grabbing;
}

/* ══════════════════════════════════════════════════════════════════════
   缩放时轮廓线不能跟着变粗
   ──────────────────────────────────────────────────────────────────────
   <g :transform="scale(k)"> 会把 stroke-width 一起放大 —— 放大 12 倍时
   原本 1px 的省界会变成 12px 黑边，整张图变成“图章”。
   vector-effect: non-scaling-stroke 让描边始终按**屏幕像素**计算，
   于是不管缩到多大，轮廓都还是 1px（这也是矢量地图缩放的标准做法）。
   ══════════════════════════════════════════════════════════════════════ */
.u-provinces path,
.u-point .dot,
.u-point .halo {
  vector-effect: non-scaling-stroke;
}

/* 文字尺寸随缩放【不该】保持不变：标签要跟着城市点一起放大才对得上。
   但放到 12 倍时 10px 字会变成 120px —— 所以用 CSS 变量控制“字”。
   简化处理：标签只在 scale < 4 时显示（见模板 v-if），避免巨大字体。 */

/* 省级轮廓：只有描边，**没有填充**。
   为什么把填充删了：
     · 1 倍时填充只有 6% 白，看着是“一块陆地”；
     · 放大到 6 倍以上，视口里只剩几个省，那 6% 白铺满整屏就成了
       “一整块浅灰玻璃罩住地图”（实测 12 倍时地图完全读不出形状）。
   没有填充之后，缩放多大都是“细线勾出的轮廓”，不会随可视面积变淡或变脏。 */
.u-provinces path {
  fill: none;
  stroke: rgba(255, 255, 255, 0.10);
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
  /* 字号用“反向 zoom”抵消：标签跟着城市点定位，但不跟着变大。
     不抵消的话 4 倍时 10px 字变 40px，整张图只剩几个大字。
     --s 由模板下发（1/k）。 */
  font-size: calc(10px * var(--s, 1));
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

/* ── 缩放控件（只在放大后出现）──────────────────────────────────────
   放在地图右上角，与点位区域重叠但不遮挡中心；pointer-events 只给自己，
   否则整块盖住地图就拖不动了。
   不用半透黑药丸：这一层是**控件**不是**标签**，用一条细描边就够。 */
.u-map-zoom {
  position: absolute;
  top: 0;
  right: 0;
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 3px 4px 3px 10px;
  border-radius: 6px;
  background: var(--hover);
  box-shadow: inset 0 0 0 1px var(--rule);
  font-size: 11px;
  color: var(--fg-2);
}
.u-map-scale {
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.02em;
}
.u-map-zoom button {
  height: 22px;
  padding: 0 9px;
  border-radius: 4px;
  border: none;
  background: none;
  box-shadow: inset 0 0 0 1px var(--rule);
  color: var(--fg-2);
  font-size: 11px;
  cursor: pointer;
  transition: color var(--dur-2) var(--ease-out);
}
.u-map-zoom button:hover {
  color: var(--fg);
}

/* 交互提示：极小的裸文字，压在左上角（不占行） */
.u-map-zoomhint {
  position: absolute;
  left: 0;
  top: 0;
  margin: 0;
  font-size: 10px;
  letter-spacing: 0.02em;
  color: var(--fg-3);
  pointer-events: none;
  opacity: 0;
  /* 默认隐藏，悬停地图时淡入 —— 常驻提示就是噪音
     （用户学会一次就不需要它了）。 */
  transition: opacity var(--dur-2) var(--ease-out);
}
.u-map:hover .u-map-zoomhint {
  opacity: 1;
}

/* 浅色：省轮廓换暗色系，否则 10% 白在浅底上完全不可见 */
body:not(.theme-dark) .u-provinces path {
  stroke: rgba(23, 22, 26, 0.13);
}

/* 降级：系统要求减少动态时不放大光晕 */
@media (prefers-reduced-motion: reduce) {
  .u-point .halo,
  .u-point .dot {
    transition: none;
  }
}
</style>
