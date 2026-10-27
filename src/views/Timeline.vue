<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useRoute, useRouter } from "vue-router";
import { useNotify } from "../composables/useNotify";
import PhotoLightbox from "../components/PhotoLightbox.vue";
import EmptyState from "../components/EmptyState.vue";
import StatBand, { type StatItem } from "../components/ui/StatBand.vue";
import GroupStat from "../components/ui/GroupStat.vue";
import AppHeader, { type HeaderTab } from "../components/ui/AppHeader.vue";
import type { TimelineCard, TimelineIndex } from "../types/timeline";
import {
  cardsOf,
  ensureCards,
  ensureThumbs,
  loadIndex,
  thumbOf,
  timelineKey,
  version as tlVersion,
} from "../stores/timeline";
import {
  MONTH_HEAD_H,
  ROW_GAP,
  YEAR_UNIT_H,
  findUnitAt,
  layoutMonth,
  monthUnitHeight,
  type LayoutRow,
  type MonthLayout,
} from "../utils/timelineLayout";

/**
 * 照片时间线（FEAT-033 → FEAT-122 重构）—— 跨相册按拍摄时间聚合浏览
 *
 * ═══════════════════════════════════════════════════════════════════════
 * 这一版为什么整体重写（BUG-2026-1027-003）
 * ───────────────────────────────────────────────────────────────────────
 * 改前：进页面 `list_timeline` 一次拉 **24,980 行 / 15.09 MB**，随后在主线程
 * 跑四层派生（rows→groups→flatItems→lineRows→monthRowMap，每层都新建 2.5 万
 * 个对象），再把全部年份的月份都建成 DOM（`visibleMonths` 只增不减）。
 * 实测首屏 2~4 s；滚到底 = 2.5 万个 `<figure>` + 2.5 万个 IntersectionObserver
 * → WebView2 渲染进程节点/内存爆炸，表现为「偶尔退出当前界面」。
 *
 * 现在三层：
 *   1. **骨架 + 按月分页**：`timeline_index`（97 行 / 约 3 KB）画左栏与统计，
 *      明细用 `timeline_month` 按月拉，缩略图路径随行下发（不再每卡一次 invoke）；
 *   2. **窗口化渲染**：board 用「绝对定位单元 + 显式高度账本」，
 *      只保留视口 ±1/±2.4 屏内的月份与行，其余是等高空占位，
 *      DOM 恒定在 ~1000 节点（与库有多大无关）；
 *   3. **跳转即定位**：左栏点击 / 回忆跳转 / 年份切换都走 `jumpTo()` ——
 *      用像素账本直接 `scrollTo`，不再 `scrollIntoView({smooth})` 穿过上千张卡，
 *      目标月即使还没渲染也能一跳到位。
 *
 * 高度账本由 `utils/timelineLayout.layoutMonth(count, width)` 给出，是
 * 「只由张数 + 容器宽决定」的纯函数 —— 所以明细没加载时滚动条长度就已经是准的，
 * 跳转落点不会因为后续加载而漂移。
 */
const router = useRouter();
const route = useRoute();
const notify = useNotify();

const index = ref<TimelineIndex | null>(null);
const loading = ref(true);
const error = ref("");
const yearFilter = ref<string>("all");

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

// ══════════════════════════════════════════════════════════════════════════
// 骨架 → 年组 / 月组
// ══════════════════════════════════════════════════════════════════════════

interface MonthIdx {
  year: string;
  month: string;
  count: number;
  label: string;
}

interface YearGrp {
  year: string;
  count: number;
  months: MonthIdx[];
}

interface YearStat {
  monthCount: number;
  photoCount: number;
  avg: number;
  dist: number[];
  distHighlight: number;
}

const yearGroups = computed<YearGrp[]>(() => {
  const idx = index.value;
  if (!idx) return [];
  const map = new Map<string, YearGrp>();
  for (const m of idx.months) {
    if (yearFilter.value !== "all" && m.year !== yearFilter.value) continue;
    let g = map.get(m.year);
    if (!g) {
      g = { year: m.year, count: 0, months: [] };
      map.set(m.year, g);
    }
    g.months.push({
      year: m.year,
      month: m.month,
      count: m.count,
      label: m.month ? `${m.year}年${Number(m.month)}月` : "未记录时间",
    });
    g.count += m.count;
  }
  return [...map.values()];
});

/** 年份下拉的选项（从骨架来，不依赖明细） */
const years = computed(() => {
  const set = new Set<string>();
  for (const m of index.value?.months ?? []) if (m.month) set.add(m.year);
  return [...set].sort();
});

// ══════════════════════════════════════════════════════════════════════════
// 单元与高度账本
// ══════════════════════════════════════════════════════════════════════════

interface Unit {
  kind: "year" | "month";
  /** 月：`year|month`；年：`year|` */
  key: string;
  /** DOM 锚点 id（与改前一致，回忆页跳转仍按它定位） */
  id: string;
  year: string;
  month: string;
  label: string;
  count: number;
  /** 相对 board 顶部的偏移（px） */
  top: number;
  /** 单元高度（px，与 DOM 显式高度严格一致） */
  h: number;
  /** 月才有：行式密排结果 */
  layout?: MonthLayout;
  /** 年才有：年组统计（月数 / 张数 / 月均 / 月度分布） */
  stat?: YearStat;
}

const boardEl = ref<HTMLElement | null>(null);
const boardW = ref(1200);

const units = computed<Unit[]>(() => {
  void tlVersion.value; // 数据版本变化（骨架/月度明细）时重算
  const out: Unit[] = [];
  let top = 0;
  let phase = 0;
  const w = boardW.value || 1200;
  for (const g of yearGroups.value) {
    out.push({
      kind: "year",
      key: `${g.year}|`,
      id: `y-${g.year}`,
      year: g.year,
      month: "",
      label: g.year,
      count: g.count,
      top,
      h: YEAR_UNIT_H,
      stat: yearStat(g),
    });
    top += YEAR_UNIT_H;
    for (const m of g.months) {
      const layout = layoutMonth(m.count, w, phase);
      const h = monthUnitHeight(layout);
      out.push({
        kind: "month",
        key: timelineKey(m.year, m.month),
        id: m.month ? `y-${m.year}-m-${m.month}` : `y-${m.year}-m-none`,
        year: m.year,
        month: m.month,
        label: m.label,
        count: m.count,
        top,
        h,
        layout,
      });
      top += h;
      phase += m.count;
    }
  }
  return out;
});

/** 账本总高（board 的显式高度 —— 明细没加载时也已经是准的） */
const totalH = computed(() => {
  const list = units.value;
  if (!list.length) return 0;
  const last = list[list.length - 1];
  return last.top + last.h;
});

const tops = computed(() => units.value.map((u) => u.top));

/** 左栏节点（年 + 月拍平，年粗月细） */
const railNodes = computed(() =>
  units.value.map((u) => ({
    kind: u.kind,
    id: u.id,
    key: u.key,
    label:
      u.kind === "year"
        ? u.year === "未记录"
          ? "未记录"
          : u.year
        : u.month
          ? `${Number(u.month)} 月`
          : "未记录",
    count: u.count,
  })),
);

/** 只含月份的偏移表 —— 左栏高亮二分用 */
const monthTops = computed(() => units.value.filter((u) => u.kind === "month").map((u) => u.top));
const monthIds = computed(() => units.value.filter((u) => u.kind === "month").map((u) => u.id));

// ══════════════════════════════════════════════════════════════════════════
// 视口窗口（虚拟化的核心）
// ══════════════════════════════════════════════════════════════════════════

const bandTop = ref(0);
const bandBottom = ref(0);
const pinned = ref<number | null>(null);
const railActive = ref("");

const viewportH = () => (typeof window === "undefined" ? 800 : window.innerHeight || 800);

/** board 顶部相对文档的偏移（每次读一次 gBCR，自愈式，不怕上方内容高度变化） */
function boardOffset(): number {
  const el = boardEl.value;
  if (!el) return 0;
  return el.getBoundingClientRect().top + window.scrollY;
}

/**
 * 更新窗口。
 *
 * 量化到 200px 才重算：滚动时如果每帧都重建渲染结构，等于把省下来的开销又还回去。
 * ±1 屏 / +2.4 屏的余量足够覆盖量化误差，不会看到空白。
 */
function setBand(force = false) {
  const rel = window.scrollY - boardOffset();
  const wantTop = rel - viewportH() * 1.0;
  if (force || Math.abs(wantTop - bandTop.value) > 200) {
    bandTop.value = wantTop;
    bandBottom.value = rel + viewportH() * 2.4;
  }
  updateRail();
}

/** 左栏高亮：像素账本上二分，零 DOM 查询（改前每帧 97 次 getElementById + gBCR） */
function updateRail() {
  const mt = monthTops.value;
  if (!mt.length) return;
  const rel = window.scrollY - boardOffset() + viewportH() * 0.35;
  const i = findUnitAt(mt, rel);
  const id = monthIds.value[i < 0 ? 0 : i];
  if (id && railActive.value !== id) railActive.value = id;
}

interface RenderCell {
  index: number;
  width: number;
  card: TimelineCard | null;
  thumb: string;
}
interface RenderRow {
  key: string;
  height: number;
  cells: RenderCell[];
}
interface RenderSegment {
  key: string;
  h: number;
  rows: RenderRow[] | null;
}
interface ViewItem {
  unit: Unit;
  segments: RenderSegment[];
  /** 本月需要在渲染前加载到第几张（下标上限 + 1） */
  need: number;
}

/**
 * 把某月切成「前置占位 / 可见行 / 后置占位」三段。
 *
 * 这样一个月即使有 4,840 张（未记录组），DOM 里也只有落进视口那几十行；
 * 前后占位用同一套行高累加，几何完全等价于整月渲染。
 */
function segmentsFor(u: Unit, forced?: { top: number; bottom: number }): {
  segments: RenderSegment[];
  need: number;
} {
  const layout = u.layout;
  if (!layout || !layout.rows.length) return { segments: [], need: 0 };
  const bTop = (forced ? forced.top : bandTop.value) - u.top - MONTH_HEAD_H;
  const bBottom = (forced ? forced.bottom : bandBottom.value) - u.top - MONTH_HEAD_H;
  const rows = layout.rows;
  const cards = cardsOf(u.year, u.month);

  const rowSpan = (i: number) => rows[i].height + ROW_GAP;
  const sumRows = (from: number, to: number) => {
    let n = 0;
    for (let i = from; i < to; i++) n += rowSpan(i);
    return n;
  };

  let y = 0;
  let first = -1;
  let last = -1;
  for (let i = 0; i < rows.length; i++) {
    const h = rowSpan(i);
    if (y + h > bTop && y < bBottom) {
      if (first < 0) first = i;
      last = i;
    }
    y += h;
  }
  if (first < 0) {
    return { segments: [{ key: `${u.key}-gap`, h: layout.rowsHeight, rows: null }], need: 0 };
  }

  const segments: RenderSegment[] = [];
  const beforeH = sumRows(0, first);
  if (beforeH > 0) segments.push({ key: `${u.key}-pre`, h: beforeH, rows: null });

  let need = 0;
  const renderRows: RenderRow[] = [];
  for (let i = first; i <= last; i++) {
    const row: LayoutRow = rows[i];
    const cells: RenderCell[] = row.cells.map((cell) => {
      const card = cards[cell.index] ?? null;
      if (cell.index + 1 > need) need = cell.index + 1;
      return {
        index: cell.index,
        width: cell.width,
        card,
        thumb: card ? thumbOf(card.path) ?? "" : "",
      };
    });
    renderRows.push({ key: `${u.key}-r${i}`, height: row.height, cells });
  }
  segments.push({ key: `${u.key}-rows`, h: sumRows(first, last + 1), rows: renderRows });

  const afterH = sumRows(last + 1, rows.length);
  if (afterH > 0) segments.push({ key: `${u.key}-post`, h: afterH, rows: null });

  return { segments, need };
}

/** 当前要渲染的单元 + 每个月要渲染的行（窗口化 + 跳转落点钉住） */
const view = computed<ViewItem[]>(() => {
  void tlVersion.value;
  const list = units.value;
  if (!list.length) return [];
  const top = bandTop.value;
  const bottom = bandBottom.value;
  let start = findUnitAt(tops.value, top);
  if (start < 0) start = 0;
  const out: ViewItem[] = [];
  for (let i = Math.max(0, start - 1); i < list.length; i++) {
    const u = list[i];
    if (u.top > bottom) break;
    if (u.top + u.h < top) continue;
    if (u.kind === "month") {
      const r = segmentsFor(u);
      out.push({ unit: u, segments: r.segments, need: r.need });
    } else {
      out.push({ unit: u, segments: [], need: 0 });
    }
  }
  // 跳转落点：目标月即使不在窗口内也要先渲染出来（否则跳过去是一块空白）
  const pin = pinned.value;
  if (pin != null && list[pin]) {
    const u = list[pin];
    if (!out.some((x) => x.unit.key === u.key)) {
      if (u.kind === "month") {
        const r = segmentsFor(u, { top: u.top, bottom: u.top + viewportH() });
        out.push({ unit: u, segments: r.segments, need: r.need });
      } else {
        out.push({ unit: u, segments: [], need: 0 });
      }
    }
  }
  return out;
});

/** 需要补数据/补缩略图的月份（window 变化后调用） */
function syncLoads() {
  const items = view.value;
  const missingThumbs: TimelineCard[] = [];
  for (const it of items) {
    if (it.unit.kind !== "month") continue;
    const { year, month, count } = it.unit;
    if (it.need > 0) void ensureCards(year, month, count, Math.min(it.need, count));
    if (!it.need) continue;
    const cards = cardsOf(year, month);
    const upto = Math.min(it.need, cards.length);
    for (let i = 0; i < upto; i++) {
      const c = cards[i];
      if (c && !thumbOf(c.path)) missingThumbs.push(c);
    }
  }
  if (missingThumbs.length) ensureThumbs(missingThumbs);
}

// ══════════════════════════════════════════════════════════════════════════
// 跳转：一次定位（不再穿过整条时间线）
// ══════════════════════════════════════════════════════════════════════════

const highlightKey = ref<string | null>(null);
let highlightTimer: number | null = null;

function flash(id: string) {
  highlightKey.value = id;
  if (highlightTimer != null) window.clearTimeout(highlightTimer);
  highlightTimer = window.setTimeout(() => {
    highlightKey.value = null;
    highlightTimer = null;
  }, 1800);
}

/**
 * 跳到某个单元。
 *
 * 与改前的区别：不再 `scrollIntoView({behavior:"smooth"})` ——
 * 那条路要「沿路」经过上千张卡（IO 回调 + 图片解码 + 追加渲染），
 * 而且目标位置边滚边变（追加渲染把文档撑高），落点会漂移。
 * 现在：像素账本 → 先钉住目标（pin 让它立刻进渲染窗口）→ 一次 `scrollTo`。
 */
function jumpTo(key: string) {
  const list = units.value;
  const idx = list.findIndex((u) => u.key === key);
  if (idx < 0) return;
  pinned.value = idx;
  const target = list[idx];
  window.scrollTo({ top: Math.max(0, boardOffset() + target.top + 1), behavior: "auto" });
  setBand(true);
  // 两帧后解除钉住：此时 scroll 事件已把窗口算到目标附近，正常逻辑接管
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      pinned.value = null;
      setBand(true);
    }),
  );
}

function railGo(node: { id: string; key: string }) {
  flash(node.id);
  jumpTo(node.key);
}

// ══════════════════════════════════════════════════════════════════════════
// 统计读数（全部来自骨架，不需要明细）
// ══════════════════════════════════════════════════════════════════════════

const timelineStats = computed<StatItem[]>(() => {
  const idx = index.value;
  if (!idx || !idx.total) return [];
  const yearSet = new Set<string>();
  const monthSet = new Set<string>();
  for (const m of idx.months) {
    if (!m.month) continue;
    yearSet.add(m.year);
    monthSet.add(timelineKey(m.year, m.month));
  }
  const noTime = idx.no_time;
  const noLoc = idx.no_location;
  return [
    { label: "照片", value: idx.total },
    {
      label: "时间跨度",
      value: yearSet.size,
      unit: yearSet.size ? `年 / ${monthSet.size} 个月` : "无数据",
    },
    {
      label: "元数据完整度",
      value: Math.round(((idx.total - noTime) / idx.total) * 100),
      unit: "%",
      note:
        (noTime ? `${noTime} 张无拍摄时间` : "拍摄时间齐全") +
        " · " +
        (noLoc === idx.total ? "未记录 GPS" : `${noLoc} 张无定位`),
      progress: (idx.total - noTime) / idx.total,
      progressTone: noTime === 0 ? "muted" : "accent",
    },
  ];
});

/** 年组统计（月数 / 张数 / 月均 / 月度分布柱）—— 全部可从骨架算出 */
function yearStat(g: YearGrp): YearStat {
  const dist = Array.from({ length: 12 }, () => 0);
  let monthCount = 0;
  for (const m of g.months) {
    if (!m.month) continue;
    monthCount += 1;
    const idx = Number(m.month);
    if (idx >= 1 && idx <= 12) dist[idx - 1] = m.count;
  }
  const maxIdx = dist.indexOf(Math.max(...dist));
  return {
    monthCount,
    photoCount: g.count,
    avg: monthCount ? Math.round(g.count / monthCount) : 0,
    dist,
    distHighlight: dist[maxIdx] > 0 ? maxIdx : -1,
  };
}

// ══════════════════════════════════════════════════════════════════════════
// 卡片交互
// ══════════════════════════════════════════════════════════════════════════

const lightboxOpen = ref(false);
const lightboxIndex = ref(0);
/** 大图查看器的作用域 = 当前月（不再为了翻页把 2.5 万张都塞进去） */
const lightboxPhotos = ref<Array<{ path: string; albumId: number | null }>>([]);

function fileUrl(p: string): string {
  return p ? convertFileSrc(p) : "";
}

function cardTitle(card: TimelineCard | null): string {
  if (!card) return "";
  return [card.label, card.location, card.album_name].filter(Boolean).join(" · ");
}

function openCard(u: Unit, cellIndex: number, card: TimelineCard | null) {
  const cards = cardsOf(u.year, u.month);
  if (!card || !cards.length) return;
  lightboxPhotos.value = cards.map((c) => ({ path: c.path, albumId: c.album_id ?? null }));
  lightboxIndex.value = Math.max(0, Math.min(cellIndex, cards.length - 1));
  lightboxOpen.value = true;
}

function gotoAlbum(card: TimelineCard | null) {
  if (!card) return;
  if (card.album_id == null) {
    notify.warning("无法定位", "该照片未关联到任何相册");
    return;
  }
  router.push({ path: `/album/${card.album_id}`, query: { focus: card.path } });
}

/** 大图工具栏「📁 在相册中查看」 */
function goAlbumFromLightbox(albumId: number) {
  lightboxOpen.value = false;
  const path = lightboxPhotos.value[lightboxIndex.value]?.path;
  router.push({ path: `/album/${albumId}`, query: path ? { focus: path } : undefined });
}

// ══════════════════════════════════════════════════════════════════════════
// 滚动 / 尺寸 / 过滤器
// ══════════════════════════════════════════════════════════════════════════

const showBackToTop = ref(false);
let scrollRaf: number | null = null;

function onWinScroll() {
  if (scrollRaf != null) return;
  scrollRaf = requestAnimationFrame(() => {
    scrollRaf = null;
    showBackToTop.value = window.scrollY > 300;
    setBand();
  });
}

function scrollToTop() {
  // 长列表下用即时定位；平滑滚动会沿路经过上千张卡（改前的卡顿来源之一）
  window.scrollTo({ top: Math.max(0, boardOffset() - 90), behavior: "auto" });
}

let boardRO: ResizeObserver | null = null;

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

/** 窗口变化 → 重算窗口（账本宽度变了，偏移全变，必须 force） */
watch(boardW, () => nextTick(() => setBand(true)));

/** 渲染窗口变化 → 补数据 / 补缩略图 */
watch(view, () => syncLoads(), { flush: "post" });

/** 切年份：账本整体重建，回到顶部（改前是让用户自己滚回去） */
watch(yearFilter, async () => {
  await nextTick();
  bandTop.value = 0;
  bandBottom.value = viewportH() * 3.4;
  window.scrollTo({ top: Math.max(0, boardOffset() - 90), behavior: "auto" });
  setBand(true);
});

// ══════════════════════════════════════════════════════════════════════════
// 生命周期
// ══════════════════════════════════════════════════════════════════════════

onMounted(async () => {
  window.addEventListener("scroll", onWinScroll, { passive: true });
  window.addEventListener("resize", onWinScroll);
  try {
    index.value = await loadIndex();
    await nextTick();
    measureBoard();
    setBand(true);
    syncLoads();
    applyJumpQuery();
  } catch (e) {
    error.value = String(e);
    notify.error("加载时间线失败", String(e));
  } finally {
    loading.value = false;
  }
});

onBeforeUnmount(() => {
  window.removeEventListener("scroll", onWinScroll);
  window.removeEventListener("resize", onWinScroll);
  if (scrollRaf != null) cancelAnimationFrame(scrollRaf);
  scrollRaf = null;
  if (highlightTimer != null) window.clearTimeout(highlightTimer);
  boardRO?.disconnect();
  boardRO = null;
});

/**
 * FEAT-E：回忆页跳转定位（`?year=2024&month=06`）
 *
 * 改前这里靠 `getElementById` 找锚点，但只有前 5 屏的月份被渲染过，
 * 跳到远处的月份根本找不到节点 → 静默失败。现在走像素账本，未渲染也能定位。
 */
function applyJumpQuery() {
  const y = String(route.query.year ?? "").trim();
  const mRaw = String(route.query.month ?? "").trim();
  if (!y) return;
  const m = mRaw && /^\d+$/.test(mRaw) ? mRaw.padStart(2, "0") : mRaw;
  const list = units.value;
  if (m) {
    const target = list.find((u) => u.kind === "month" && u.key === timelineKey(y, m));
    if (target) {
      flash(target.id);
      jumpTo(target.key);
      return;
    }
  }
  const yearUnit = list.find((u) => u.kind === "year" && u.year === y);
  if (yearUnit) {
    flash(yearUnit.id);
    jumpTo(yearUnit.key);
  }
}
</script>

<template>
  <div class="tl-page" :style="{ color: 'var(--fg)' }">
    <AppHeader
      title="智慧相册"
      icon="📅"
      :meta="`${index?.total ?? 0} 张已入库照片`"
      :tabs="moduleTabs"
    />

    <header class="tl-header">
      <h1 class="tl-title">照片时间线</h1>
      <p class="tl-subtitle">
        跨相册按拍摄时间聚合浏览 · 共 {{ index?.total ?? 0 }} 张
      </p>
      <span class="tl-head-right">
        <select v-model="yearFilter" class="tl-year-select">
          <option value="all">全部年份</option>
          <option v-for="y in years" :key="y" :value="y">{{ y }}</option>
        </select>
      </span>
    </header>

    <!-- 统计读数（来自骨架，进页面即准） -->
    <StatBand
      v-if="!loading && !error && timelineStats.length"
      :items="timelineStats"
      class="tl-stats"
    />

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
      v-else-if="!index || index.total === 0"
      icon="🌅"
      title="还没有可展示的时间线"
      description="请先在相册详情页执行「组合扫描」或「内容扫描」，把照片的拍摄时间写入数据库，再回来查看跨相册时间线。"
      action-label="去相册扫描"
      @action="router.push('/albums')"
    />

    <!-- 时间线：左侧时间栏 + 右侧虚拟化密排 -->
    <div v-else class="tl-shell">
      <aside class="tl-rail" aria-label="时间导航">
        <div class="tl-rail-inner">
          <button
            v-for="n in railNodes"
            :key="n.id"
            class="tl-rail-node"
            :class="[`is-${n.kind}`, { on: railActive === n.id }]"
            @click="railGo(n)"
          >
            <span class="tl-rail-dot" />
            <span class="tl-rail-label">{{ n.label }}</span>
            <span class="tl-rail-count">{{ n.count }}</span>
          </button>
        </div>
      </aside>

      <!-- 高度即总高：明细没加载时滚动条长度也已准确，跳转落点不会漂移 -->
      <div ref="boardEl" class="tl-board" :style="{ height: totalH + 'px' }">
        <div
          v-for="item in view"
          :key="item.unit.key"
          :id="item.unit.id"
          class="tl-unit"
          :class="[
            `is-${item.unit.kind}`,
            { 'tl-highlight': highlightKey === item.unit.id },
          ]"
          :style="{ top: item.unit.top + 'px', height: item.unit.h + 'px' }"
        >
          <!-- 年标题 -->
          <h2 v-if="item.unit.kind === 'year' && item.unit.stat" class="tl-year-title">
            {{
              item.unit.year === "未记录"
                ? "🕐 未记录拍摄时间"
                : `🗓️ ${item.unit.year} 年`
            }}
            <span class="tl-year-count">{{ item.unit.count }} 张</span>
            <GroupStat
              v-if="item.unit.year !== '未记录'"
              class="tl-year-stat"
              :items="[
                { value: item.unit.stat.monthCount, label: '个月' },
                { value: item.unit.stat.avg, label: '平均/月' },
              ]"
              :dist="item.unit.stat.dist"
              :dist-highlight="item.unit.stat.distHighlight"
            />
          </h2>

          <!-- 月标题 + 行（前后用等高占位代替未渲染的行） -->
          <template v-else>
            <h3 class="tl-month-title">
              {{ item.unit.label }} <span>{{ item.unit.count }}</span>
            </h3>
            <div
              v-for="seg in item.segments"
              :key="seg.key"
              class="tl-seg"
              :style="{ height: seg.h + 'px' }"
            >
              <template v-if="seg.rows">
                <div
                  v-for="row in seg.rows"
                  :key="row.key"
                  class="tl-row"
                  :style="{ height: row.height + 'px' }"
                >
                  <figure
                    v-for="cell in row.cells"
                    :key="cell.index"
                    class="tl-card"
                    :style="{ width: cell.width + 'px' }"
                    :title="cardTitle(cell.card)"
                    @click="openCard(item.unit, cell.index, cell.card)"
                  >
                    <img
                      v-if="cell.thumb"
                      :src="fileUrl(cell.thumb)"
                      loading="lazy"
                      decoding="async"
                      class="tl-thumb"
                      alt=""
                    />
                    <div v-else class="tl-thumb tl-thumb-ph">🖼️</div>
                    <figcaption v-if="cell.card" class="tl-cap">
                      <span v-if="cell.card.label" class="tl-label">{{
                        cell.card.label
                      }}</span>
                      <span v-if="cell.card.location" class="tl-loc"
                        >📍 {{ cell.card.location }}</span
                      >
                      <button
                        class="tl-open"
                        title="跳转到该照片所在相册"
                        @click.stop="gotoAlbum(cell.card)"
                      >
                        ↗
                      </button>
                    </figcaption>
                  </figure>
                </div>
              </template>
            </div>
          </template>
        </div>
      </div>
    </div>

    <!-- 回到顶部 -->
    <transition name="tl-top">
      <button
        v-show="showBackToTop"
        class="tl-top-btn"
        title="回到顶部"
        aria-label="回到顶部"
        @click="scrollToTop"
      >
        ↑
      </button>
    </transition>

    <!-- 内部大图查看器（作用域 = 当前月） -->
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
/* 页面壳（FEAT-106）：与顶栏、数据带用同一套左右留白。 */
.tl-page {
  padding: 0 var(--sp-8) 48px;
  min-height: 100vh;
  box-sizing: border-box;
}

/* FEAT-115：页头一行。标题 17px（不与顶栏的组件名抢主视觉），
   副标题带总数，年份筛选靠右。 */
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
.tl-head-right {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
}
/* 年份筛选下拉：类名刻意独立于 .tl-year（改前与年份卡片撞名，
   下拉吃到了卡片的浅底 + 浅字，实测 1.33:1 看不见）。 */
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
  background: #1e1b22;
  color: #f2f0f4;
}

/* ══ 两栏壳 ══════════════════════════════════════════════════════════ */
.tl-shell {
  display: flex;
  gap: 26px;
  align-items: flex-start;
}
.tl-rail {
  flex: 0 0 148px;
  position: sticky;
  top: 62px;
  max-height: calc(100vh - 96px);
  overflow-y: auto;
  scrollbar-width: thin;
}
.tl-rail-inner {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 1px;
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
  background: var(--c-bg-solid, var(--color-bg));
  box-shadow: 0 0 0 1.5px color-mix(in srgb, var(--c-fg) 30%, transparent);
  transition: background var(--dur-1) ease, box-shadow var(--dur-1) ease;
}
.tl-rail-label {
  font-size: 12px;
  white-space: nowrap;
}
.tl-rail-count {
  margin-left: auto;
  padding-left: 8px;
  font-size: 10.5px;
  opacity: 0.55;
  font-variant-numeric: tabular-nums;
}
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
.tl-rail-node.is-month .tl-rail-label {
  font-size: 11.5px;
}
.tl-rail-node.is-month {
  padding-left: 8px;
}
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

/* ══ 虚拟化板 ════════════════════════════════════════════════════════
   board 高度 = 账本总高；每个单元绝对定位到账本给的 top/height。
   为什么用绝对定位而不是普通流：这样「账本 = 真实布局」是构造性成立的，
   不依赖 margin 是否合并、字体度量是否一致，跳转落点才能精确。 */
.tl-board {
  flex: 1;
  min-width: 0;
  position: relative;
}
.tl-unit {
  position: absolute;
  left: 0;
  right: 0;
}
/* 月块用 flex column：子项 margin 不合并，账本与 DOM 高度严格一致 */
.tl-unit.is-month {
  display: flex;
  flex-direction: column;
}

/* 年标题：高度写死 44px（与 timelineLayout.YEAR_UNIT_H 的 66 = 44 + 22 留白对应） */
.tl-year-title {
  height: 44px;
  box-sizing: border-box;
  margin: 0;
  padding: 0 0 10px;
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 17px;
  border-bottom: 1px solid color-mix(in srgb, var(--c-fg) 12%, transparent);
}
.tl-year-count {
  font-size: 12px;
  opacity: 0.65;
  font-weight: 500;
}
.tl-year-stat {
  padding-left: 12px;
  border-left: 1px solid var(--c-rule);
  font-weight: 400;
}

/* 月标题：高度写死 22 + 8 外边距（= MONTH_HEAD_H 30） */
.tl-month-title {
  flex: none;
  height: 22px;
  line-height: 22px;
  margin: 0 0 8px;
  font-size: 13px;
  opacity: 0.8;
  font-weight: 600;
  letter-spacing: 0.01em;
  overflow: hidden;
}
.tl-month-title span {
  font-size: 12px;
  opacity: 0.6;
  margin-left: 4px;
}

/* 段落容器（前置占位 / 可见行 / 后置占位） */
.tl-seg {
  flex: none;
  overflow: hidden;
}

/* 行式密排：行高由账本给；8px 行距与 timelineLayout.ROW_GAP 一致 */
.tl-row {
  display: flex;
  gap: 8px;
  margin-bottom: 8px;
}

/* 跳转落点高亮 */
.tl-unit.tl-highlight {
  background: color-mix(in srgb, var(--color-primary) 12%, transparent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-primary) 35%, transparent) inset;
  border-radius: 10px;
}

.tl-card {
  margin: 0;
  position: relative;
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

/* 窄屏：左栏收起成横向滚动条 */
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

/* 加载骨架 */
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

/* 回到顶部按钮 */
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
  .tl-title {
    font-size: 18px;
  }
}
</style>

/* ===== FEAT-107：统计数据带（自带 padding + 下缘发丝线）===== */
<style>
.tl-stats {
  margin: 0 0 16px;
  margin-left: calc(var(--gutter) * -1);
  margin-right: calc(var(--gutter) * -1);
}
</style>
