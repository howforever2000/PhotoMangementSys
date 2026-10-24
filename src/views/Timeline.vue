<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useRoute, useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";
import PhotoLightbox from "../components/PhotoLightbox.vue";
import EmptyState from "../components/EmptyState.vue";
import StatBand, { type StatItem } from "../components/ui/StatBand.vue";
import GroupStat from "../components/ui/GroupStat.vue";
import AppHeader, { type HeaderTab } from "../components/ui/AppHeader.vue";
import type { ContentSearchHit } from "../types/content";
import { getThumbs } from "../utils/thumbQueue";

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
const theme = useThemeStore();
const notify = useNotify();

const loading = ref(true);
const rows = ref<ContentSearchHit[]>([]);

/** 二级导航：时间线属于智慧相册模块，与内容分类/人物/回忆/搜索同级。
 *  顺序必须与 SmartAlbum 一致（内容分类在第一个），否则页间跳转时 tab 会跳位。 */
const moduleTabs: HeaderTab[] = [
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
    {
      label: "无拍摄时间",
      value: noTime,
      delta: all.length ? `${Math.round((noTime / all.length) * 100)}%` : "",
      tone: noTime > 0 ? "up" : "mute",
      // 有拍摄时间的占比（进度条 = 已完成的部分，倒过来读更好懂）
      progress: all.length ? (all.length - noTime) / all.length : 0,
    },
    {
      label: "地点覆盖",
      value: all.length ? Math.round(((all.length - noLoc) / all.length) * 100) : 0,
      unit: "%",
      // 这是“照片没 GPS”与“功能坏了”的唯一区分依据，因此 0% 时给出明确提示
      delta: noLoc === all.length ? "未记录 GPS" : `${noLoc} 张无定位`,
      tone: noLoc === all.length ? "mute" : "up",
      progress: all.length ? (all.length - noLoc) / all.length : 0,
      progressTone: noLoc === all.length ? "muted" : "accent",
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
    scrollRaf = null;
  });
}

function scrollToTop() {
  window.scrollTo({ top: 0, behavior: "smooth" });
}

onMounted(() => window.addEventListener("scroll", onWinScroll, { passive: true }));
onBeforeUnmount(() => {
  window.removeEventListener("scroll", onWinScroll);
  if (scrollRaf != null) cancelAnimationFrame(scrollRaf);
  scrollRaf = null;
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
  <div class="tl-page" :style="{ color: theme.textColor }">
    <AppHeader
      title="智慧相册"
      icon="📅"
      :meta="`${rows.length} 张已入库照片`"
      :tabs="moduleTabs"
    />
    <header class="tl-header">
      <h1 class="tl-title">照片时间线</h1>
      <p class="tl-subtitle">跨相册按拍摄时间聚合浏览</p>
    </header>

    <!-- 工具条：年份过滤 + 总数 -->
    <div class="tl-toolbar">
      <label class="tl-count">共 {{ totalFiltered }} 张</label>
      <select v-model="yearFilter" class="tl-year-select" @change="onYearFilter">
        <option value="all">全部年份</option>
        <option v-for="y in years" :key="y" :value="y">{{ y }}</option>
      </select>
    </div>

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

    <!-- 时间线分组 -->
    <div v-else class="tl-content">
      <section
        v-for="g in groups"
        :key="g.year"
        :id="`y-${g.year}`"
        class="tl-year"
        :class="{ 'tl-highlight': highlightKey === `y-${g.year}` }"
      >
        <h2 class="tl-year-title">
          {{ g.year === "未记录" ? "🕐 未记录拍摄时间" : `🗓️ ${g.year} 年` }}
          <span class="tl-year-count">{{ yearStat(g).photoCount }} 张</span>
          <!-- FEAT-103：组级统计（月数 / 张数 / 月均）+ 月度分布迷你柱。
               放在年标题同一行、字号降一档 —— 贴着标题读，不打断浏览流。 -->
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
          <div class="tl-grid">
            <figure
              v-for="r in mg.items"
              :key="r.id"
              :ref="(el) => registerCard(el, r.path, r.album_id ?? 0)"
              class="tl-card"
              :title="[r.label, r.location, r.album_name].filter(Boolean).join(' · ')"
              @click="openCard(r)"
            >
              <img v-if="thumbMap[r.path]" :src="fileUrl(thumbMap[r.path])" loading="lazy" class="tl-thumb" alt="" />
              <div v-else class="tl-thumb tl-thumb-ph">🖼️</div>
              <figcaption class="tl-cap">
                <span v-if="r.label" class="tl-label">{{ r.label }}</span>
                <span v-if="r.location" class="tl-loc">📍 {{ r.location }}</span>
                <button class="tl-open" title="跳转到该照片所在相册" @click.stop="gotoAlbum(r)">↗</button>
              </figcaption>
            </figure>
          </div>
        </div>
      </section>
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

/* 页内标题降一档：顶栏已经有「智慧相册 · 时间线」，
   这里只需要说明"这页看的是什么"，不需要再来一个 22px 大标题抢主视觉 */
.tl-header {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
  margin: 16px 0 10px;
}
.tl-title {
  font-size: var(--fs-head);
  margin: 0;
  font-weight: var(--fw-em);
  letter-spacing: -0.2px;
  /* 页面背景上的标题：on-bg 对比色（BUG-2026-0919-004） */
  color: var(--color-on-bg, inherit);
}
.tl-subtitle {
  margin: 0;
  font-size: var(--fs-meta);
  color: var(--c-fg-2);
}
.tl-toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 6px 0 18px;
}
.tl-count {
  font-size: 14px;
  opacity: 0.8;
}
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

.tl-year {
  background: var(--card-bg, rgba(127, 127, 127, 0.05));
  border-radius: 12px;
  padding: 16px 16px 4px;
  margin-bottom: 18px;
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
  margin: 0 0 10px;
  display: flex;
  align-items: center;
  gap: 10px;
}
.tl-year-count {
  font-size: 12px;
  opacity: 0.65;
  font-weight: 500;
}
.tl-month {
  margin-bottom: 14px;
}
.tl-month-title {
  font-size: 14px;
  margin: 0 0 8px;
  opacity: 0.85;
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
.tl-card {
  margin: 0;
  position: relative;
  aspect-ratio: 1 / 1;
  border-radius: 10px;
  overflow: hidden;
  cursor: pointer;
  background: rgba(127, 127, 127, 0.1);
  transition: transform var(--dur-1);
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
