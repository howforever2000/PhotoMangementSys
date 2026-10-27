<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useRoute, useRouter } from "vue-router";
import { useNotify } from "../composables/useNotify";
import PhotoLightbox from "../components/PhotoLightbox.vue";
import EmptyState from "../components/EmptyState.vue";
import StatBand, { type StatItem } from "../components/ui/StatBand.vue";
import GroupStat from "../components/ui/GroupStat.vue";
import AppHeader, { type HeaderTab } from "../components/ui/AppHeader.vue";
import type { ContentSearchHit } from "../types/content";
import { getThumbs } from "../utils/thumbQueue";
import { justifyRows } from "../utils/justifiedRows";
import { useIncrementalReveal } from "../composables/useIncrementalReveal";

/**
 * 照片时间线（FEAT-033）—— 跨相册按拍摄时间聚合浏览
 *
 * 数据源：`photo_content_scan`（组合扫描入库），含 path / album_id / shoot_time / location。
 * 缩略图：**只对进入视口的卡片入队**（IntersectionObserver），按 album_id 分批 60 条
 * 交给全局队列 `utils/thumbQueue`（在飞 ≤3）。不再一次性把每个相册的全部照片都要
 * 一遍 —— 那正是 BUG-2026-1023-009（升级后全库缓存失效 + 129 路并发 → 线程 732 /
 * 内存 1.2GB → 0xC00000FD 进程退出）的引火线。
 * 分组：年 → 月，未记录拍摄时间的归入「未记录」。
 *
 * FEAT-E：回忆页面跳转定位
 *   从 Memories.vue 跳转时携带 query { year, month }，
 *   加载完成后自动滚动到对应年 / 月分组并高亮提示。
 */
const router = useRouter();
const route = useRoute();
const notify = useNotify();

const loading = ref(true);
const rows = ref<ContentSearchHit[]>([]);

/** 二级导航：时间线属于智慧相册模块，与全局概览/内容分类/人物/回忆/搜索同级。
 *  顺序必须与 SmartAlbum 一致（全局概览在第一个），否则页间跳转时 tab 会跳位。 */
const moduleTabs: HeaderTab[] = [
  { label: "全局概览", to: "/smart" },
  { label: "内容分类", to: "/smart?tab=category" },
  { label: "人物", to: "/smart?tab=face" },
  { label: "地点", to: "/smart?tab=location" },
  { label: "回忆", to: "/memories" },
  { label: "时间线", to: "/timeline" },
  { label: "智能搜索", to: "/search" },
];
const error = ref("");
/** path → 缩略图缓存路径 */
const thumbMap = ref<Record<string, string>>({});
const yearFilter = ref<string>("all");
/** FEAT-E：高亮标记的目标节点 id（仅在 query 指定 year/month 时设置） */
const highlightKey = ref<string | null>(null);

const years = computed(() => {
  const set = new Set<string>();
  for (const r of rows.value) {
    const y = r.shoot_time?.slice(0, 4);
    if (y) set.add(y);
  }
  return [...set].sort();
});

/**
 * FEAT-103：时间线专属统计数据带
 *
 * 之前这里只有一个“共 N 张” + 年份下拉。而时间线最需要回答的
 * 是**时间维度的健康度** —— UX 评审里已经点名了两个真实问题：
 *   · “31 张照片无拍摄时间”—— 但界面上看不出来这是多严重
 *   · “地点 653/653 全为 NULL”—— 时间线甚至不展示这个���
 * 把这两项做成读数后，“空转”从“安静的空态”变成“可见的数字”。
 *
 * “时间跨度”取月数而不是天数：相册的尺度是月/年，日粒度无意义。
 */
const timelineStats = computed<StatItem[]>(() => {
  const all = rows.value;
  const noTime = all.filter((r) => !r.shoot_time).length;
  const noLoc = all.filter((r) => !r.location).length;

  // 时间跨度：按实际出现过的年月计数。
  // ⚠️ 局部变量不能叫 years —— 会遮蔽外层的 years computed，
  // 于是 `years.value` 就变成在访问自己（TS7022/TS2448）。首版踩过。
  const ym = new Set<string>();
  for (const r of all) {
    if (r.shoot_time && r.shoot_time.length >= 7) ym.add(r.shoot_time.slice(0, 7));
  }
  const yearCount = years.value.length;

  const items: StatItem[] = [
    { label: "照片", value: all.length },
    {
      label: "时间跨度",
      value: yearCount,
      unit: yearCount ? `年 / ${ym.size} 个月` : "无数据",
    },
    /* FEAT-115：原来的「无拍摄时间」与「地点覆盖」各占一列，但它们是**诊断读数** ——
       在任何一年都读同一个值，占两列等于把首屏宽度送给不常变的信息。
       合并成一列：主读数给"有拍摄时间"的占比，注释里同时交代两个缺口。
       唯一一处金色留给它 —— 它回答"这页的数据全不全"。
       ⚠️ 不能把两个缺口都写成 tone:"up"（金色）：金色是"这数在变"的信号。 */
    {
      label: "元数据完整度",
      value: all.length ? Math.round(((all.length - noTime) / all.length) * 100) : 0,
      unit: "%",
      note:
        (noTime ? `${noTime} 张无拍摄时间` : "拍摄时间齐全") +
        " · " +
        (noLoc === all.length ? "未记录 GPS" : `${noLoc} 张无定位`),
      progress: all.length ? (all.length - noTime) / all.length : 0,
      progressTone: noTime === 0 ? "muted" : "accent",
    },
  ];
  return items;
});

/** 按年→月分组（拍摄时间升序） */
interface MonthGroup {
  month: string; // "01"~"12"
  label: string;
  items: ContentSearchHit[];
}
interface YearGroup {
  year: string;
  months: MonthGroup[];
}
/* ── 左侧时间栏的扁平节列表（FEAT-113 ③-3）──────────────────────────
   为什么要「扁平」：左栏要把年和月放在同一根竖线上（年粗、月细），
   如果按 groups 嵌套渲染，年与月之间的那条竖线会被行盒切断。
   拍平成 [{kind:'year'},{kind:'month'}...] 后用一个 flex column 绘制，
   每个节点自带左侧圆点，竖线就是容器上的 background 直线 —— 不断。 */
interface RailNode {
  kind: "year" | "month";
  /** 锚点 id，与右侧分区一致（点一下就滚过去） */
  id: string;
  label: string;
  count: number;
}
const railNodes = computed<RailNode[]>(() => {
  const out: RailNode[] = [];
  for (const g of groups.value) {
    const yCount = g.months.reduce((n, mg) => n + mg.items.length, 0);
    out.push({
      kind: "year",
      id: `y-${g.year}`,
      label: g.year === "未记录" ? "未记录" : `${g.year}`,
      count: yCount,
    });
    for (const mg of g.months) {
      if (!mg.month) continue; /* 未记录时间的月份不占位，否则一年只有一个月却多一行 */
      out.push({
        kind: "month",
        id: `y-${g.year}-m-${mg.month}`,
        label: `${Number(mg.month)} 月`,
        count: mg.items.length,
      });
    }
  }
  return out;
});

/** 左栏当前高亮的锚点（随滚动自动跟随，点击时立即设） */
const railActive = ref<string>("");

/** 左栏点击：滚到对应分区（用 scrollIntoView 而非改 hash，避免污染路由） */
function railGo(id: string) {
  const el = document.getElementById(id);
  if (!el) return;
  railActive.value = id;
  el.scrollIntoView({ behavior: "smooth", block: "start" });
  /* 滚过去后闪一下高亮，告诉用户“就是这一块”（与 query 定位同一套视觉） */
  const key = id.replace(/^y-/, "y-");
  highlightKey.value = key;
  window.setTimeout(() => {
    if (highlightKey.value === key) highlightKey.value = null;
  }, 1800);
}

/* ── 右侧照片错落密排（FEAT-113 ③-3）──────────────────────────────
   复用相册墙的 justifyRows（行式密排），而不是另写一套：
   两者目标完全一致（每行铺满、宽度按宽高比分配、位置稳定可预期）。
   与相册墙的差别只有参数：时间线是小图浏览，名义高度低、缝更小、
   不放假宽幅（宽幅会把某个月的一行拉成一条横幅，月份多时显得碎）。 */
const boardW = ref(0);
let boardRO: ResizeObserver | null = null;
const boardEl = ref<HTMLElement | null>(null);

/**
 * 量容器宽。
 * ⚠️ 坑：不能靠 document.getElementById 在 onMounted 里量。
 *    #tl-board 在 v-else（loading 结束后）才渲染，onMounted 时它还没进 DOM，
 *    于是 boardW 一直停在回退值 1000 → 所有行按 1000px 算宽，
 *    实际容器 1322px → 每行右侧留一大块空白（实测 0/18 行铺满）。
 *    用模板 ref + watch 监听元素出现才是正确时机。
 */
function measureBoard() {
  const el = boardEl.value;
  if (!el) return;
  const w = Math.round(el.clientWidth);
  if (w > 0 && Math.abs(w - boardW.value) > 1) boardW.value = w;
  if (!boardRO && typeof ResizeObserver !== "undefined") {
    boardRO = new ResizeObserver((ents) => {
      const nw = Math.round(ents[0]?.contentRect.width ?? 0);
      if (nw > 0 && Math.abs(nw - boardW.value) > 1) boardW.value = nw;
    });
    boardRO.observe(el);
  }
}

watch(boardEl, (el) => {
  if (el) nextTick(measureBoard);
});

/** 密排参数：与**相册管理墙完全一致**（AlbumList.wallRows）
    用户要的就是“参考相册管理界面”，不要另造一套。
    错落来自**宽度**（每 5 张一张 1.8 倍宽横幅），不是行高抳动：
    行内上下严格对齐，行与行左右也齐，只有“一大两小”的节奏变化。 */
const JUSTIFY_OPTS = {
  gap: 8,
  captionH: 0, /* 图注是叠在封面上的，不占额外高度 */
  /* 名义高 200：与相册墙用同一套公式，但比它的 232 低一档 ——
     时间线是“小图快浏览”，行太高会让一个月占满好几屏。
     实测 1322px 容器：H0=200 → 每行 2~4 块、行高在 200~260 浮动；
     H0=232 → 每行 1~4 块、且行高被顶到上限 260（偏大）。 */
  nominalHeight: 200,
  baseAspect: 1.28,
  wideEvery: 5,
  wideFactor: 1.8,
  minCoverH: 130,
  maxCoverH: 260,
} as const;

/** 每个月的起始下标（在整个时间线序列里的位置）—— 用作强制断行点 */
const monthStarts = computed<number[]>(() => {
  const out: number[] = [];
  let i = 0;
  for (const g of groups.value)
    for (const mg of g.months) {
      if (i > 0) out.push(i);
      i += mg.items.length;
    }
  return out;
});

/** 全局序列（与 monthStarts 同一顺序） */
const flatItems = computed<Array<{ item: ContentSearchHit; mg: MonthGroup; g: YearGroup }>>(() => {
  const flat: Array<{ item: ContentSearchHit; mg: MonthGroup; g: YearGroup }> = [];
  for (const g of groups.value)
    for (const mg of g.months) for (const it of mg.items) flat.push({ item: it, mg, g });
  return flat;
});

/**
 * 整条时间线一次算密排，但在月份边界强制断开。
 *
 * 这是“参考相册管理界面”的关键：相册管理是整面墙一次算，所以行高只有
 * 254/201 两种、规律交替；而按月各算时会有 147~260 五种，跳得厉害。
 * 传 breakPoints 后两者兼得：共用一份预算（行高接近相册墙），
 * 又保证行不跨月（月份标题能正常插入）。
 */
const lineRows = computed(() => {
  if (!flatItems.value.length) return [];
  return justifyRows({
    count: flatItems.value.length,
    containerWidth: boardW.value || 1000,
    ...JUSTIFY_OPTS,
    breakPoints: monthStarts.value,
  }).map((row) => ({
    height: row.height,
    cells: row.cells.map((c) => ({ ...c, ...flatItems.value[c.index] })),
  }));
});

/** 把行按月份归档：断点保证了每行只属于一个月，这里直接分桶即可 */
const monthRowMap = computed<Record<string, Array<{ height: number; cells: Array<Record<string, any>> }>>>(() => {
  const map: Record<string, Array<{ height: number; cells: Array<Record<string, any>> }>> = {};
  for (const row of lineRows.value) {
    const first = row.cells[0];
    if (!first) continue;
    const k = String(first.g.year) + "|" + String(first.mg.month);
    if (!map[k]) map[k] = [];
    map[k].push({ height: row.height, cells: row.cells });
  }
  return map;
});

const groups = computed<YearGroup[]>(() => {
  const map = new Map<string, Map<string, ContentSearchHit[]>>();
  for (const r of rows.value) {
    if (yearFilter.value !== "all" && r.shoot_time?.slice(0, 4) !== yearFilter.value) continue;
    const y = r.shoot_time?.slice(0, 4) ?? "未记录";
    const m = r.shoot_time?.slice(5, 7) ?? "";
    if (!map.has(y)) map.set(y, new Map());
    const mm = map.get(y)!;
    if (!mm.has(m)) mm.set(m, []);
    mm.get(m)!.push(r);
  }
  const out: YearGroup[] = [];
  for (const [year, mm] of map) {
    const months: MonthGroup[] = [];
    for (const [m, items] of mm) {
      months.push({
        month: m,
        label: m ? `${year}年${Number(m)}月` : "未记录时间",
        items,
      });
    }
    months.sort((a, b) => a.month.localeCompare(b.month));
    out.push({ year, months });
  }
  out.sort((a, b) => a.year.localeCompare(b.year));
  return out;
});

const totalFiltered = computed(() => groups.value.reduce((n, g) => n + g.months.reduce((k, mg) => k + mg.items.length, 0), 0));

/* ══════════════ FEAT-117：时间线热加载（按屏幕预算增量渲染）══════════════
   问题：缩略图是懒的，但 DOM 是急的 —— 这里原先 `v-for="g in groups"` 一次把
   所有年/月/行建成节点。2.5 万张照片 = 2.5 万个 <figure> + 2.5 万个 observer，
   进页面几秒白屏。

   做法：把「月份」当成最小渲染单元（月份是天然断点，也是用户认得出的分界），
   交给 useIncrementalReveal 按"屏幕高度"决定先渲多少 —— 首屏 5 屏，
   滚到底再追加 2 屏。具体一屏放多少张不用猜：`costOf` 用的是当月各行的
   真实行高（justifiedRows 已算好），所以预算换算出来的就是实际像素。 */
interface MonthUnit {
  key: string;
  year: string;
  month: string;
  label: string;
  items: ContentSearchHit[];
  /** 估算高度：月份标题 + 每行（行高 + 行间距） */
  height: number;
}

const monthUnits = computed<MonthUnit[]>(() => {
  const out: MonthUnit[] = [];
  for (const g of groups.value) {
    for (const mg of g.months) {
      const rows = monthRowMap.value[g.year + "|" + mg.month] ?? [];
      const h = 46 + rows.reduce((n, r) => n + r.height + 8, 0);
      out.push({
        key: g.year + "|" + mg.month,
        year: g.year,
        month: mg.month,
        label: mg.label,
        items: mg.items,
        height: h,
      });
    }
  }
  return out;
});

const { visible: visibleMonths, hasMore: hasMoreMonths, loadMore: loadMoreMonths, reset: resetMonths, visibleCount: visibleMonthCount } =
  useIncrementalReveal<MonthUnit>({
    units: monthUnits,
    costOf: (u) => u.height,
    screens: 5,
    stepScreens: 2,
  });

/* 渲染视图：只带已渲染的月份，但年份统计仍取自**完整**年组
   （否则"2024 年 1234 张"会随着滚动不断变小，读数就成了假的）。 */
const renderGroups = computed<YearGroup[]>(() => {
  const shown = new Set(visibleMonths.value.map((u) => u.key));
  const out: YearGroup[] = [];
  for (const g of groups.value) {
    const months = g.months.filter((mg) => shown.has(g.year + "|" + mg.month));
    if (!months.length) continue;
    out.push({ year: g.year, months });
  }
  return out;
});

/** 切换年份筛选 → 重新从 5 屏开始（否则累积预算会一次渲完新结果） */
watch(yearFilter, () => nextTick(resetMonths));

/** 每个年组的统计（月数 / 张数 / 月均）+ 月度分布迷你柱 */
interface YearStat {
  monthCount: number;
  photoCount: number;
  avg: number;
  dist: number[];
  distHighlight: number;
}
function yearStat(g: YearGroup): YearStat {
  const monthCount = g.months.filter((m) => m.month).length;
  const photoCount = g.months.reduce((n, mg) => n + mg.items.length, 0);
  // 月度分布：按 1~12 月对齐（缺月补 0），这样不同年份的柱子可直接横向对比
  const dist = Array.from({ length: 12 }, () => 0);
  for (const mg of g.months) {
    const idx = Number(mg.month);
    if (idx >= 1 && idx <= 12) dist[idx - 1] = mg.items.length;
  }
  const maxIdx = dist.indexOf(Math.max(...dist));
  return {
    monthCount,
    photoCount,
    avg: monthCount ? Math.round(photoCount / monthCount) : 0,
    dist,
    distHighlight: dist[maxIdx] > 0 ? maxIdx : -1,
  };
}

function fileUrl(p: string): string {
  return p ? convertFileSrc(p) : "";
}

/**
 * 缩略图惰性填充（BUG-2026-1023-009 修复）
 *
 * 旧实现：`rows` 全量按 album 分组后 `Promise.all` —— 一次点开时间线就发出
 * 129 路 `get_photo_thumbs`（单相册最多 2876 条），后端每路再开 8 线程解码，
 * 冷缓存时直接把进程打死。
 *
 * 新实现：每个卡片自己声明「我进视口了」，按 album 分组、每批 60 条交给
 * `thumbQueue`（全局在飞 ≤3）。首屏只命中几十张，滚动时渐进补齐，与相册网格
 * （PhotoGrid）同一套惰性语义。
 */
const pendingThumbs = new Map<string, number>(); // path → albumId
const FLUSH_BATCH = 60;
/**
 * 待入队上限：快速滚动时会一次刷过几百上千张卡，若全排进生成队列，
 * “滑到底” 等于把整条时间线都排进去（虽然是限流的，但也没必要）。
 * 只保留最近的这批；被丢弃的卡片没被 unobserve，滚回来时
 * IntersectionObserver 会重新触发。
 */
const MAX_PENDING = 300;
let cardObserver: IntersectionObserver | null = null;
let flushing = false;

function getCardObserver(): IntersectionObserver | null {
  if (cardObserver) return cardObserver;
  if (typeof IntersectionObserver === "undefined") return null;
  cardObserver = new IntersectionObserver(onCardsIntersect, { rootMargin: "600px 0px" });
  return cardObserver;
}

/** 模板函数 ref：卡片挂载时登记；已有缩略图的直接跳过 */
function registerCard(el: unknown, path: string, albumId: number) {
  const node = el as HTMLElement | null;
  if (!node) return; // 卸载
  if (!path || thumbMap.value[path]) return;
  node.dataset.path = path;
  node.dataset.album = String(albumId);
  getCardObserver()?.observe(node);
}

function onCardsIntersect(entries: IntersectionObserverEntry[]) {
  let added = false;
  for (const en of entries) {
    if (!en.isIntersecting) continue;
    const node = en.target as HTMLElement;
    const path = node.dataset.path;
    // 不 unobserve：已生成过的靠 thumbMap 跳过，被排队上限丢弃的靠
    // “滚出去再滚回来” 重新触发，卡片最终都能自愈。
    if (!path || thumbMap.value[path] || pendingThumbs.has(path)) continue;
    pendingThumbs.set(path, Number(node.dataset.album ?? 0));
    added = true;
  }
  if (added) void flushPending();
}

/** 后台队列冲刷：按相册分组、每批 60 条，队列内部还有全局限流 */
async function flushPending() {
  if (flushing) return;
  flushing = true;
  try {
    while (pendingThumbs.size > 0) {
      // 超出上限时丢掉最早那批（通常是已经被滑过去的卡）
      if (pendingThumbs.size > MAX_PENDING) {
        const keys = [...pendingThumbs.keys()];
        for (const k of keys.slice(0, pendingThumbs.size - MAX_PENDING)) pendingThumbs.delete(k);
      }
      const byAlbum = new Map<number, string[]>();
      for (const [path, aid] of pendingThumbs) {
        if (!byAlbum.has(aid)) byAlbum.set(aid, []);
        byAlbum.get(aid)!.push(path);
      }
      pendingThumbs.clear();
      for (const [aid, paths] of byAlbum) {
        for (let i = 0; i < paths.length; i += FLUSH_BATCH) {
          try {
            const pairs = await getThumbs(aid, paths.slice(i, i + FLUSH_BATCH));
            for (const [path, thumb] of pairs) {
              if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
            }
          } catch {
            // 单批失败不阻塞：缺图卡片回退占位，滚出再滚回会重新入队
          }
        }
      }
    }
  } finally {
    flushing = false;
  }
}

/* ---- 回到顶部：时间线跨年/跨月很长，滚下去后一键回顶 ---- */
const showBackToTop = ref(false);
let scrollRaf: number | null = null;

/** rAF 节流监听窗口滚动（与相册列表页同一套阈值 300px） */
function onWinScroll() {
  if (scrollRaf != null) return;
  scrollRaf = requestAnimationFrame(() => {
    showBackToTop.value = window.scrollY > 300;
    syncRailActive();
    scrollRaf = null;
  });
}

/**
 * 左栏高亮跟随滚动。
 *
 * 做法：取所有锚点元素，找出「顶部已过判断线、且最靠上」的那一个。
 * 为什么不用 IntersectionObserver：
 *   观察器只能告诉你“进入了/离开了”，但月份高度差异很大（一个月能跨好几屏），
 *   会出现「两个锚点同时可见」的情况，观察器无法回答“现在到底在哪个里”。
 *   按位置排序找最后一个越过判断线的，语义明确且不受高度影响。
 * 判断线取视口 35% 处：这样标题刚滑到屏幕中上方就切换，符合阅读时眼位。
 */
function syncRailActive() {
  if (!railNodes.value.length) return;
  const line = window.innerHeight * 0.35;
  let cur = railNodes.value[0].id;
  for (const n of railNodes.value) {
    const el = document.getElementById(n.id);
    if (!el) continue;
    if (el.getBoundingClientRect().top <= line) cur = n.id;
    else break; /* 节点按文档顺序排列，一旦有节点在判断线下方，后面的更加靠下 */
  }
  railActive.value = cur;
}

/* 首次渲染完就要给左栏一个初始高亮，否则停在顶部时左栏一个都不亮。
   watch groups：数据到达 → 分区渲染 → nextTick 后锚点已存在。 */
watch(
  groups,
  () => {
    nextTick(() => requestAnimationFrame(() => syncRailActive()));
  },
  { immediate: true },
);

function scrollToTop() {
  window.scrollTo({ top: 0, behavior: "smooth" });
}

onMounted(() => window.addEventListener("scroll", onWinScroll, { passive: true }));
onBeforeUnmount(() => {
  window.removeEventListener("scroll", onWinScroll);
  if (scrollRaf != null) cancelAnimationFrame(scrollRaf);
  scrollRaf = null;
  boardRO?.disconnect();
  boardRO = null;
  cardObserver?.disconnect();
  cardObserver = null;
  pendingThumbs.clear();
});

/* ---- FEAT-034-B：内部大图查看器 ---- */
const lightboxOpen = ref(false);
const lightboxIndex = ref(0);
/** 传给 Lightbox 的 photos：path + albumId （Lightbox 借此触发 ensure_photo_scanned） */
const lightboxPhotos = computed(() =>
  rows.value.map((r) => ({ path: r.path, albumId: r.album_id ?? null })),
);

/** 点击照片：打开内部 Lightbox（FEAT-034-B）。
 *  - 之前错误地跳转到相册详情页，现改为内部大图查看体验。
 *  - 如该照片无 albumId（理论上不会出现，但兑底）走 openOriginal。 */
function openCard(r: ContentSearchHit) {
  if (r.album_id == null) {
    void openOriginal(r);
    return;
  }
  const idx = rows.value.findIndex((x) => x.path === r.path);
  if (idx < 0) return;
  lightboxIndex.value = idx;
  lightboxOpen.value = true;
}

/** FEAT-034-B：兑底调用——极少数无 albumId 的记录仍可用系统默认看图器打开原图。 */
async function openOriginal(r: ContentSearchHit) {
  try {
    const { openPath } = await import("@tauri-apps/plugin-opener");
    await openPath(r.path);
  } catch (e) {
    notify.error("无法打开原图", String(e));
  }
}

/** 点击右下角按钮：跳转到该照片所属相册并定位（FEAT-034-B）。
 *  - 携带 ?focus=path 查询参数，AlbumDetail 接收后高亮该照片。
 *  - 之前错误地调用系统外部打开器，现改为跳相册以保持上一级上下文。 */
function gotoAlbum(r: ContentSearchHit) {
  if (r.album_id == null) {
    notify.warning("无法定位", "该照片未关联到任何相册");
    return;
  }
  router.push({
    path: `/album/${r.album_id}`,
    query: { focus: r.path },
  });
}

/** 年份过滤器变化时仅需重算分组（无需重新拉缩略图） */
function onYearFilter() {
  // 分组重算；缩略图已全部缓存，无需重复请求
}

onMounted(async () => {
  try {
    rows.value = await invoke<ContentSearchHit[]>("list_timeline");
    // 缩略图不再预先全量拉取：由每个卡片的 IntersectionObserver 按视口惰性入队
    // （见 registerCard / flushPending）。
    // FEAT-E：根据 query 跳转到目标年 / 月（Memories 跳过来时使用）
    applyJumpQuery();
  } catch (e) {
    error.value = String(e);
    notify.error("加载时间线失败", String(e));
  } finally {
    loading.value = false;
  }
});

/**
 * FEAT-E：读取路由 query 中的 year / month，自动滚动并高亮。
 * - 仅有 year：选中该年的过滤器并滚动到该年分组。
 * - 既有 year 又有 month：定位到该月的具体分组块并高亮。
 */
function applyJumpQuery() {
  const y = String(route.query.year ?? "").trim();
  const mRaw = String(route.query.month ?? "").trim();
  if (!y) return;
  // 月份归一化为两位 "MM" 格式（兼容 "8"/"08" 两种入参，与分组 id `y-{y}-m-{MM}` 对齐）
  const m = mRaw && /^\d+$/.test(mRaw) ? mRaw.padStart(2, "0") : mRaw;
  // 1. 选中该年（保持其他年折叠效果由过滤器收敛到仅当前年）
  yearFilter.value = y;
  // 高亮定位目标：月份 → 无月份时定位年份
  highlightKey.value = m ? `y-${y}-m-${m}` : `y-${y}`;
  // 等待 computed groups 渲染完
  nextTick(() => {
    // 月份锚点落空时回退到年份锚点（保证至少定位到该年，不停在顶部早期月份）
    const monthEl = m ? document.getElementById(`y-${y}-m-${m}`) : null;
    const el = monthEl ?? document.getElementById(`y-${y}`);
    if (el) {
      if (!monthEl) highlightKey.value = `y-${y}`; // 回退年份时高亮年份分组
      el.scrollIntoView({ behavior: "smooth", block: "start" });
      // 高亮动画 1.8s 后清除
      window.setTimeout(() => {
        highlightKey.value = null;
      }, 1800);
    } else {
      // 锚点不存在（异常 query）也要清高亮，避免状态残留
      highlightKey.value = null;
    }
  });
}
/** 大图工具栏「📁 在相册中查看」：关掉看图器，跳相册并带 ?focus 定位高亮该照片 */
function goAlbumFromLightbox(albumId: number) {
  lightboxOpen.value = false;
  const path = lightboxPhotos.value[lightboxIndex.value]?.path;
  router.push({ path: `/album/${albumId}`, query: path ? { focus: path } : undefined });
}
</script>

<template>
  <div class="tl-page" :style="{ color: 'var(--fg)' }">
    <AppHeader
      title="智慧相册"
      icon="📅"
      :meta="`${rows.length} 张已入库照片`"
      :tabs="moduleTabs"
    />
    <!-- FEAT-115：标题 / 副标题 / 总数 / 年份筛选 合成**一行**。
         改前是三行（标题 → 工具条 → 数据带）约 200px 才看到第一张照片；
         而顶栏已经写着「智慧相册 · 时间线」，页内再来一个 19px 大标题
         是同一信息说两遍。 -->
    <header class="tl-header">
      <h1 class="tl-title">照片时间线</h1>
      <p class="tl-subtitle">跨相册按拍摄时间聚合浏览 · 共 {{ totalFiltered }} 张</p>
      <span class="tl-head-right">
        <select v-model="yearFilter" class="tl-year-select" @change="onYearFilter">
          <option value="all">全部年份</option>
          <option v-for="y in years" :key="y" :value="y">{{ y }}</option>
        </select>
      </span>
    </header>

    <!-- FEAT-103：时间维度统计数据带（无拍摄时间 / 地点覆盖在这里可见） -->
    <StatBand v-if="!loading && !error && rows.length" :items="timelineStats" class="tl-stats" />

    <!-- 加载骨架 -->
    <div v-if="loading" class="tl-skeleton">
      <div v-for="i in 6" :key="i" class="sk-section">
        <div class="sk-title" />
        <div class="sk-grid">
          <div v-for="j in 8" :key="j" class="sk-card" />
        </div>
      </div>
    </div>

    <!-- 错误态 -->
    <EmptyState
      v-else-if="error"
      icon="⚠️"
      title="加载失败"
      :description="error"
      action-label="返回主页"
      tone="error"
      @action="router.push('/home')"
    />

    <!-- 空态 -->
    <EmptyState
      v-else-if="rows.length === 0"
      icon="🌅"
      title="还没有可展示的时间线"
      description="请先在相册详情页执行「组合扫描」或「内容扫描」，把照片的拍摄时间写入数据库，再回来查看跨相册时间线。"
      action-label="去相册扫描"
      @action="router.push('/albums')"
    />

    <!-- 时间线分组（FEAT-113 ③-3）：左侧吸顶时间栏 + 右侧照片错落密排 -->
    <div v-else class="tl-shell">
      <!-- 左栏：年/月两级节点，当前视口所在月份高亮 -->
      <aside class="tl-rail" aria-label="时间导航">
        <div class="tl-rail-inner">
          <button
            v-for="n in railNodes"
            :key="n.id"
            class="tl-rail-node"
            :class="[`is-${n.kind}`, { on: railActive === n.id }]"
            @click="railGo(n.id)"
          >
            <span class="tl-rail-dot" />
            <span class="tl-rail-label">{{ n.label }}</span>
            <span class="tl-rail-count">{{ n.count }}</span>
          </button>
        </div>
      </aside>

      <!-- 右侧：年分组 → 月分组 → 行式密排 -->
      <div ref="boardEl" id="tl-board" class="tl-board">
        <section
          v-for="g in renderGroups"
          :key="g.year"
          :id="`y-${g.year}`"
          class="tl-year"
          :class="{ 'tl-highlight': highlightKey === `y-${g.year}` }"
        >
          <h2 class="tl-year-title">
            {{ g.year === "未记录" ? "🕐 未记录拍摄时间" : `🗓️ ${g.year} 年` }}
            <span class="tl-year-count">{{ yearStat(g).photoCount }} 张</span>
            <GroupStat
              v-if="g.year !== '未记录'"
              class="tl-year-stat"
              :items="[
                { value: yearStat(g).monthCount, label: '个月' },
                { value: yearStat(g).avg, label: '平均/月' },
              ]"
              :dist="yearStat(g).dist"
              :dist-highlight="yearStat(g).distHighlight"
            />
          </h2>
          <div
            v-for="mg in g.months"
            :key="mg.month"
            :id="mg.month ? `y-${g.year}-m-${mg.month}` : `y-${g.year}-m-none`"
            class="tl-month"
            :class="{ 'tl-highlight': highlightKey === `y-${g.year}-m-${mg.month}` }"
          >
            <h3 class="tl-month-title">{{ mg.label }} <span>{{ mg.items.length }}</span></h3>
            <!-- 逐行渲染：整条线共用一份预算，行高接近，不跨月 -->
            <div
              v-for="(row, ri) in monthRowMap[g.year + '|' + mg.month] ?? []"
              :key="ri"
              class="tl-row"
              :style="{ height: row.height + 'px' }"
            >
              <figure
                v-for="cell in row.cells"
                :key="cell.item.id"
                :ref="(el) => registerCard(el, cell.item.path, cell.item.album_id ?? 0)"
                class="tl-card"
                :style="{ width: cell.width + 'px' }"
                :title="[cell.item.label, cell.item.location, cell.item.album_name].filter(Boolean).join(' · ')"
                @click="openCard(cell.item)"
              >
                <img
                  v-if="thumbMap[cell.item.path]"
                  :src="fileUrl(thumbMap[cell.item.path])"
                  loading="lazy"
                  class="tl-thumb"
                  alt=""
                />
                <div v-else class="tl-thumb tl-thumb-ph">🖼️</div>
                <figcaption class="tl-cap">
                  <span v-if="cell.item.label" class="tl-label">{{ cell.item.label }}</span>
                  <span v-if="cell.item.location" class="tl-loc">📍 {{ cell.item.location }}</span>
                  <button class="tl-open" title="跳转到该照片所在相册" @click.stop="gotoAlbum(cell.item)">↗</button>
                </figcaption>
              </figure>
            </div>
          </div>
        </section>

        <!-- 热加载哨兵：还没渲完就显示"往后还有多少"。
             滚动到底部附近会自动追加（useIncrementalReveal 监听 window scroll），
             这里的按钮是给键盘用户/跳转落点的兜底出口。 -->
        <div v-if="hasMoreMonths" class="tl-more">
          <button type="button" class="tl-more-btn" @click="loadMoreMonths">
            继续加载
          </button>
          <span class="tl-more-hint">
            已渲染 {{ visibleMonthCount }} / {{ monthUnits.length }} 个月 ·
            向下滚动自动加载
          </span>
        </div>
      </div>
    </div>

    <!-- 回到顶部箭头：滚动超过一屏后出现，点击平滑回顶 -->
    <transition name="tl-top">
      <button
        v-show="showBackToTop"
        class="tl-top-btn"
        title="回到顶部"
        aria-label="回到顶部"
        @click="scrollToTop"
      >↑</button>
    </transition>

    <!-- FEAT-034-B：内部大图查看器（点击缩略图打开） -->
    <PhotoLightbox
      v-if="lightboxOpen"
      :photos="lightboxPhotos"
      :index="lightboxIndex"
      @close="lightboxOpen = false"
      @open-album="goAlbumFromLightbox"
    />
  </div>
</template>

<style scoped>
/* 页面壳（FEAT-106）：与顶栏、数据带用同一套左右留白。
   旧版 max-width:1200 + padding:20 是居中缩窄的版式 ——
   而顶栏是**全宽玻璃板**（它在 --sp-8 处起止），两者对不齐，
   宽屏下会出现"顶栏贴窗口、内容居中"的错位。 */
.tl-page {
  padding: 0 var(--sp-8) 48px;
  min-height: 100vh;
  box-sizing: border-box;
}

/* FEAT-115：页头一行。标题降到 17px（不再与顶栏的组件名抢主视觉），
   副标题带上总数，年份筛选靠右 —— 一行读完"这页是什么 / 有多少 / 看哪年"。 */
.tl-header {
  display: flex;
  align-items: baseline;
  gap: 12px;
  flex-wrap: wrap;
  margin: 18px 0 14px;
}
.tl-title {
  font-size: 17px;
  margin: 0;
  font-weight: 620;
  letter-spacing: -0.2px;
  color: var(--fg);
}
.tl-subtitle {
  margin: 0;
  font-size: var(--fs-meta);
  color: var(--fg-3);
}
.tl-head-right { margin-left: auto; display: inline-flex; align-items: center; }
/* 年份筛选下拉（FEAT-106）：
   ⚠️ 这里修的是一个**类名碰撞**：旧版下拉与年份卡片都叫 .tl-year，
   于是下拉吃到了卡片的 background: var(--card-bg) + padding: 16px，
   在浅色预设下变成“浅底 + 浅字”（实测 1.33:1，基本看不见）。
   现在下拉用自己的类名，并按玻璃控件口径给完整的字色/底色。 */
.tl-year-select {
  height: 30px;
  padding: 0 10px;
  border-radius: 8px;
  border: 1px solid var(--c-rule);
  background: color-mix(in srgb, var(--c-fg) 6%, transparent);
  color: var(--c-fg);
  font-size: var(--fs-body);
  cursor: pointer;
  outline: none;
}
.tl-year-select:focus {
  border-color: color-mix(in srgb, var(--c-accent) 55%, transparent);
}
.tl-year-select option {
  /* 下拉选项由系统绘制，必须给明确的实底与字色，否则跟随系统主题可能变成白底白字 */
  background: #1e1b22;
  color: #f2f0f4;
}

/* 年分组：FEAT-113 ③-3 后不再包一层卡片背景。
   左栏已经在充当“结构”了，右栏再套一层圆角底色就是双重容器 ——
   而且这层底会吃掉相片周围的留白，让密排看起来像被关在盒子里。
   现在只靠标题的全行分隔与间距分层。 */
.tl-year {
  padding: 0;
  margin-bottom: 26px;
  transition: background var(--dur-3), box-shadow var(--dur-3);
}
/* FEAT-E：跳定位高亮（query 定位 + 滚动后亮色边框 1.8s 淡出） */
.tl-year.tl-highlight,
.tl-month.tl-highlight {
  background: color-mix(in srgb, var(--color-primary) 12%, transparent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-primary) 35%, transparent) inset;
  border-radius: 10px;
}
.tl-year-title {
  font-size: 17px;
  margin: 0 0 14px;
  display: flex;
  align-items: center;
  gap: 10px;
  padding-bottom: 9px;
  /* 标题下的细分隔线：把年与年之间分开，代替原来的卡片底 */
  border-bottom: 1px solid color-mix(in srgb, var(--c-fg) 12%, transparent);
}
.tl-year-count {
  font-size: 12px;
  opacity: 0.65;
  font-weight: 500;
}
.tl-month {
  margin-bottom: 20px;
}
/* ── 热加载哨兵（FEAT-117）─────────────────────────────────────────────
   它不是"加载更多"按钮，而是**进度说明**：告诉用户下面还有东西、
   而且滚下去就会自己出来。所以文字在前、按钮在后，且只有发丝线不用框。 */
.tl-more {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--sp-3);
  padding: var(--sp-6) 0 var(--sp-2);
  border-top: 1px solid var(--rule);
  margin-top: var(--sp-4);
}
.tl-more-btn {
  padding: 6px 14px;
  border-radius: 999px;
  border: 1px solid var(--rule);
  background: transparent;
  color: var(--fg-2);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
  transition: color var(--dur-1) ease, border-color var(--dur-1) ease;
}
.tl-more-btn:hover {
  color: var(--fg);
  border-color: var(--fg-3);
}
.tl-more-hint {
  font-size: 11px;
  color: var(--fg-3);
  font-variant-numeric: tabular-nums;
}

.tl-month-title {
  font-size: 13px;
  margin: 0 0 9px;
  opacity: 0.8;
  font-weight: 600;
  letter-spacing: 0.01em;
}
.tl-month-title span {
  font-size: 12px;
  opacity: 0.6;
  margin-left: 4px;
}
.tl-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
  gap: 10px;
}

/* ══ FEAT-113 ③-3：左时间栏 + 行式密排 ══════════════════════════════ */

/* 两栏壳：左栏定宽、右栏吃剩余。左栏自身不滚，靠 sticky 内层跟随。 */
.tl-shell {
  display: flex;
  gap: 26px;
  align-items: flex-start;
}
.tl-rail {
  flex: 0 0 148px;
  /* FEAT-115：顶栏现在是 54px 的带（不再是 52px 板 + 10px 空气），
     吸顶位置跟着上移 6px。 */
  position: sticky;
  top: 62px;
  max-height: calc(100vh - 96px);
  overflow-y: auto;
  /* 滚动时不要出现滚动条把它挤窄 */
  scrollbar-width: thin;
}
.tl-rail-inner {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 1px;
  /* 竖线：不靠 border 画在每个节点上（那样年/月之间的缝会断开），
     而是容器上一条统一直线，节点圆点压在它上面 */
  background: linear-gradient(
    to right,
    transparent 6.5px,
    color-mix(in srgb, var(--c-fg) 16%, transparent) 6.5px,
    color-mix(in srgb, var(--c-fg) 16%, transparent) 7.5px,
    transparent 7.5px
  );
  padding: 4px 0;
}
.tl-rail-node {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 3px 6px 3px 0;
  border: 0;
  background: none;
  color: var(--c-fg-2);
  cursor: pointer;
  text-align: left;
  border-radius: 6px;
  transition: color var(--dur-1) ease, background var(--dur-1) ease;
}
.tl-rail-node:hover {
  color: var(--c-fg);
  background: color-mix(in srgb, var(--c-fg) 6%, transparent);
}
.tl-rail-dot {
  flex: none;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  margin-left: 3.5px;
  /* 圆点必须不透明才能“盖断”竖线；底色用页面底色对应的表面色 */
  background: var(--c-bg-solid, var(--color-bg));
  box-shadow: 0 0 0 1.5px color-mix(in srgb, var(--c-fg) 30%, transparent);
  transition: background var(--dur-1) ease, box-shadow var(--dur-1) ease;
}
.tl-rail-label {
  font-size: 12px;
  white-space: nowrap;
  /* 不加 margin-right：靠 count 的 margin-left:auto 推开；
     但 textContent 会读成 "202413" —— 那是探针取值问题，不是布局问题。
     真正需要保证的是视觉上有间距：count 自身左内边距给足。 */
}
.tl-rail-count {
  margin-left: auto;
  padding-left: 8px;
  font-size: 10.5px;
  opacity: 0.55;
  font-variant-numeric: tabular-nums;
}
/* 年节点：粗一号、前上方留空，形成层级 */
.tl-rail-node.is-year {
  margin-top: 9px;
  padding-top: 4px;
  padding-bottom: 4px;
}
.tl-rail-node.is-year:first-child {
  margin-top: 0;
}
.tl-rail-node.is-year .tl-rail-label {
  font-size: 13px;
  font-weight: 700;
  color: var(--c-fg);
  letter-spacing: -0.01em;
}
.tl-rail-node.is-year .tl-rail-dot {
  width: 9px;
  height: 9px;
  margin-left: 2.5px;
}
/* 月节点：缩进、字号降一档 */
.tl-rail-node.is-month .tl-rail-label {
  font-size: 11.5px;
}
.tl-rail-node.is-month {
  padding-left: 8px;
}
/* 当前项：强调色圆点 + 文字提亮 */
.tl-rail-node.on {
  color: var(--c-fg);
}
.tl-rail-node.on .tl-rail-dot {
  background: var(--c-accent);
  box-shadow: 0 0 0 1.5px color-mix(in srgb, var(--c-accent) 70%, transparent),
    0 0 10px color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.tl-rail-node.on .tl-rail-label {
  font-weight: 650;
}

.tl-board {
  flex: 1;
  min-width: 0; /* flex 子项不设会导致内部 nowrap 内容把容器撑宽 */
}

/* 行式密排：一行一个 flex，宽度由 justifyRows 算好（行宽精确=容器宽） */
.tl-row {
  display: flex;
  gap: 8px;
  margin-bottom: 8px;
}

/* 窄屏：左栏收起成横向滚动条（sticky 竖向占用太贵） */
@media (max-width: 1080px) {
  .tl-shell {
    display: block;
  }
  .tl-rail {
    position: sticky;
    top: 56px;
    z-index: 5;
    max-height: none;
    overflow-x: auto;
    overflow-y: hidden;
    flex: none;
    margin-bottom: 14px;
    padding: 6px 0;
    background: color-mix(in srgb, var(--c-bg-solid, var(--color-bg)) 88%, transparent);
    -webkit-backdrop-filter: blur(10px);
    backdrop-filter: blur(10px);
    border-radius: 10px;
  }
  .tl-rail-inner {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    background: none;
    padding: 0 6px;
  }
  .tl-rail-node,
  .tl-rail-node.is-year,
  .tl-rail-node.is-month {
    margin-top: 0;
    padding: 3px 8px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--c-fg) 6%, transparent);
    flex: none;
  }
  .tl-rail-node.on {
    background: color-mix(in srgb, var(--c-accent) 22%, transparent);
  }
  .tl-rail-dot {
    display: none;
  }
  .tl-rail-node.is-year .tl-rail-label,
  .tl-rail-node.is-month .tl-rail-label {
    font-size: 12px;
  }
}
.tl-card {
  margin: 0;
  position: relative;
  /* 高宽由行式密排给（:style 上的 width + 行的 height），不再固定 1:1 */
  height: 100%;
  border-radius: 10px;
  overflow: hidden;
  cursor: pointer;
  background: rgba(127, 127, 127, 0.1);
  transition: transform var(--dur-1);
  flex: none;
}
.tl-card:hover {
  transform: translateY(-2px);
}
.tl-thumb {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.tl-thumb-ph {
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 30px;
  opacity: 0.6;
}
.tl-cap {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 6px 8px;
  display: flex;
  align-items: flex-end;
  gap: 6px;
  flex-wrap: nowrap;
  font-size: 11px;
  color: #fff;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.65));
  pointer-events: auto;
}
.tl-label {
  font-weight: 600;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  max-width: 70%;
}
.tl-loc {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  flex-shrink: 1;
}
.tl-open {
  margin-left: auto;
  background: rgba(255, 255, 255, 0.2);
  border: none;
  color: #fff;
  border-radius: 50%;
  width: 22px;
  height: 22px;
  cursor: pointer;
  flex-shrink: 0;
}
.tl-skeleton {
  margin-top: 10px;
}
.sk-section {
  margin-bottom: 18px;
}
.sk-title,
.sk-card {
  background: rgba(127, 127, 127, 0.15);
  border-radius: 10px;
}
.sk-title {
  width: 120px;
  height: 18px;
  margin-bottom: 10px;
}
.sk-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
  gap: 10px;
}
.sk-card {
  aspect-ratio: 1 / 1;
  animation: skpulse 1.2s infinite;
}
@keyframes skpulse {
  50% {
    opacity: 0.4;
  }
}
/* 回到顶部按钮（与相册列表页同款：右下角悬浮圆钮） */
.tl-top-btn {
  position: fixed;
  right: 24px;
  bottom: 24px;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  border: 1px solid var(--color-border, rgba(127, 127, 127, 0.3));
  background: var(--card-bg, #fff);
  box-shadow: 0 2px 12px rgba(0, 0, 0, 0.16);
  font-size: 20px;
  color: var(--color-primary-text);
  cursor: pointer;
  z-index: 150;
  display: flex;
  align-items: center;
  justify-content: center;
  user-select: none;
  transition: transform var(--dur-2) ease, background var(--dur-2);
}
.tl-top-btn:hover {
  background: var(--color-soft-accent, color-mix(in srgb, var(--color-primary) 12%, transparent));
  transform: scale(1.08);
}
.tl-top-btn:active {
  transform: scale(0.95);
}
.tl-top-enter-active,
.tl-top-leave-active {
  transition: opacity var(--dur-2) ease, transform var(--dur-2) ease;
}
.tl-top-enter-from,
.tl-top-leave-to {
  opacity: 0;
  transform: translateY(8px);
}

@media (max-width: 640px) {
  .tl-page {
    padding: 0 var(--sp-5) 40px;
  }
  .tl-grid {
    grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
  }
  .tl-title {
    font-size: 18px;
  }
}
</style>

/* ===== FEAT-107：统计数据带（自带 padding + 下缘发丝线）===== */
.tl-stats {
  margin: 0 0 16px;
  /* 数据带自带 var(--gutter) 左右内边距；时间线容器已给了页面留白，
     这里用负 margin 抵消，让数据带的左侧文字与年份标题左对齐。 */
  margin-left: calc(var(--gutter) * -1);
  margin-right: calc(var(--gutter) * -1);
}

/* 年组统计：跟在"X 张"后面，与标题同基线但不抢视觉重量。
   加一条分隔竖线把它与标题区隔开 —— 否则"2026 年 388 张 5个月 平均78/月"
   会读成一串同权重的文字。 */
.tl-year-stat {
  padding-left: 12px;
  border-left: 1px solid var(--c-rule);
  font-weight: 400;
}
