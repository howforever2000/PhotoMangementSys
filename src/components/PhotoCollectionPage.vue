<script setup lang="ts">
/**
 * PhotoCollectionPage —— 通用「照片集合」全页二级视图（FEAT-201）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么要有这个组件
 * ──────────────────────────────────────────────────────────────────────────
 * FEAT-200 之后，分类/人物画廊都换成了「页面内二级视图 + 分页热加载」；
 * 概览页的点击入口（那年今日 / 最近导入…）还需要同一套呈现，但它们没有
 * chips / 批量管理 / 设头像这些专属交互 —— 如果每次都复制整份画廊骨架，
 * 分页逻辑会散落三四处。本组件把「集合浏览」的公共部分抽出来：
 *
 *   [← 返回] 标题 · 已加载 N 张        [格子缩放]
 *   ┌ 照片网格（FEAT-200 同款：首屏 300，滚近底部 400px 追加 150）┐
 *   └ 哨兵 → IntersectionObserver → loader(limit, offset) ┘
 *
 * 与后端解耦：宿主传入 `loader(limit, offset)`，把任意命令的行映射成
 * CollectionPhoto（types/collection.ts）。「不足一整页 = 没有更多」，
 * 所以 loader 不需要返回总数。
 *
 * 刻意不做的（与人物/分类画廊的分工）：
 *   · 批量管理 / 右键菜单 / 删除 —— 集合是纯浏览入口，删除走相册/分类页，
 *     避免 PhotoDeleteDialog + PhotoActionMenu 的整套装进通用组件；
 *   · chips / 设头像 —— 那些是画廊专属交互。
 * 已落地的入口：那年今日按天（?type=day）、最近导入（?type=recent）；
 * 后续任何「给用户一组照片看」的需求都从这里长出去。
 */
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useNotify } from "../composables/useNotify";
import { getThumbs } from "../utils/thumbQueue";
import PhotoLightbox from "./PhotoLightbox.vue";
import PhotoZoomBar from "./PhotoZoomBar.vue";
import type { CollectionPhoto } from "../types/collection";

const props = withDefaults(
  defineProps<{
    /** 页头标题（如「3 年前的今天 · 2023-11-21」「最近导入」） */
    title: string;
    /** 标题后的小字说明（口径/排序等，可空） */
    subtitle?: string;
    /** 集合为空时的文案 */
    emptyText?: string;
    /** 分页加载器：返回不足 limit 行即视为没有更多 */
    loader: (limit: number, offset: number) => Promise<CollectionPhoto[]>;
  }>(),
  { subtitle: "", emptyText: "这个集合里还没有照片" },
);

const router = useRouter();
const notify = useNotify();

/* -------------------- 数据（FEAT-200 同款分页热加载） -------------------- */
const PAGE_INITIAL = 300;
const PAGE_STEP = 150;
const items = ref<CollectionPhoto[]>([]);
const offset = ref(0);
const done = ref(false);
const loading = ref(false);
const loadingMore = ref(false);
const loadError = ref("");
const sentinelEl = ref<HTMLElement | null>(null);

function expectedPage(): number {
  return offset.value === 0 ? PAGE_INITIAL : PAGE_STEP;
}

/** 拉取下一页并追加；缩略图按页入队 */
async function fetchPage(): Promise<void> {
  const want = expectedPage();
  const page = await props.loader(want, offset.value);
  const isFirst = offset.value === 0;
  offset.value += page.length;
  if (page.length < want) done.value = true;
  items.value = isFirst ? page : [...items.value, ...page];
  void loadThumbsFor(page);
}

async function loadMore() {
  if (done.value || loadingMore.value || loading.value) return;
  loadingMore.value = true;
  try {
    await fetchPage();
  } catch (e) {
    notify.error("加载更多失败", String(e));
    done.value = true;
  } finally {
    loadingMore.value = false;
    // 追加后哨兵可能仍在视口内（本页没撑出滚动条）→ 继续补页直到撑出为止
    requestAnimationFrame(() => {
      const el = sentinelEl.value;
      if (el && el.getBoundingClientRect().top < window.innerHeight + 400) void loadMore();
    });
  }
}

/* 哨兵观察：滚近底部 400px 即触发追加（与分类/人物画廊同款） */
let io: IntersectionObserver | null = null;

function ensureIo(): IntersectionObserver | null {
  if (typeof IntersectionObserver === "undefined") return null;
  if (!io) {
    io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) void loadMore();
      },
      { rootMargin: "400px 0px" },
    );
  }
  return io;
}

watch([sentinelEl, done, loading], () => {
  const obs = ensureIo();
  if (!obs) return;
  obs.disconnect();
  if (sentinelEl.value && !done.value && !loading.value) obs.observe(sentinelEl.value);
});

onMounted(async () => {
  loading.value = true;
  loadError.value = "";
  try {
    await fetchPage();
  } catch (e) {
    loadError.value = String(e);
    done.value = true;
  } finally {
    loading.value = false;
  }
});

onBeforeUnmount(() => io?.disconnect());

/* -------------------- 缩略图管线（按页取，缺图回退占位） -------------------- */
const thumbMap = ref<Record<string, string>>({});

async function loadThumbsFor(rows: CollectionPhoto[]) {
  const byAlbum = new Map<number, string[]>();
  for (const r of rows) {
    if (thumbMap.value[r.path]) continue;
    const aid = r.albumId ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    byAlbum.get(aid)!.push(r.path);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, paths]) => {
      try {
        for (const [path, thumb] of await getThumbs(aid, paths)) {
          if (!thumbMap.value[path]) thumbMap.value[path] = convertFileSrc(thumb);
        }
      } catch {
        /* 单相册失败不阻塞：缺图卡片回退占位 */
      }
    }),
  );
}

function thumbSrc(r: CollectionPhoto): string {
  return thumbMap.value[r.path] || "";
}

function cellTip(r: CollectionPhoto): string {
  return [r.shootTime, r.location].filter(Boolean).join(" · ") || r.path;
}

/* -------------------- 格子缩放 / 灯箱 / 返回 -------------------- */
const cellSize = ref(150);

/** PhotoZoomBar 模板引用（Ctrl+滚轮复用同一套档位步进） */
const zoomBarRef = ref<InstanceType<typeof PhotoZoomBar> | null>(null);

/** Ctrl+滚轮调整格子尺寸（与缩放条同档位；preventDefault 拦住 WebView2 页面缩放） */
function onGridWheel(e: WheelEvent) {
  if (!e.ctrlKey) return;
  e.preventDefault();
  zoomBarRef.value?.stepGear(e.deltaY < 0 ? 1 : -1);
}

const lightboxOpen = ref(false);
const lightboxIndex = ref(0);
const lightboxPhotos = items; // 结构即 { path, albumId }，直接可用

function openPhoto(i: number) {
  lightboxIndex.value = i;
  lightboxOpen.value = true;
}

/** 返回上一页（概览页）；直接以本页打开（无历史）时兜底回智慧相册 */
function goBack() {
  if (window.history.state && window.history.state.back != null) router.back();
  else router.push("/smart");
}
</script>

<template>
  <div class="pcp-wrap">
    <div class="pcp-head">
      <button class="btn" @click="goBack">← 返回</button>
      <h2 class="pcp-title">{{ title }}</h2>
      <span class="pcp-count">{{ items.length }} 张</span>
      <span v-if="subtitle" class="pcp-sub">{{ subtitle }}</span>
      <span class="pcp-spacer"></span>
      <PhotoZoomBar ref="zoomBarRef" v-model="cellSize" :disabled="!items.length" />
    </div>

    <!-- 加载骨架 -->
    <div v-if="loading" class="pcp-state">
      <div class="pcp-skel-grid">
        <div v-for="i in 10" :key="i" class="pcp-skel"></div>
      </div>
    </div>

    <!-- 错误 -->
    <div v-else-if="loadError" class="pcp-state">
      <div class="pcp-state-icon">⚠️</div>
      <p>加载失败：{{ loadError }}</p>
      <button class="btn" @click="goBack">返回</button>
    </div>

    <!-- 空 -->
    <div v-else-if="!items.length" class="pcp-state">
      <div class="pcp-state-icon">🖼</div>
      <p>{{ emptyText }}</p>
    </div>

    <!-- 照片网格 -->
    <div
      v-else
      class="pcp-grid"
      :style="{ gridTemplateColumns: `repeat(auto-fill, minmax(${cellSize}px, 1fr))` }"
      @wheel="onGridWheel"
    >
      <figure
        v-for="(r, i) in items"
        :key="r.path"
        class="pcp-cell"
        :title="cellTip(r)"
        @click="openPhoto(i)"
      >
        <img v-if="thumbSrc(r)" :src="thumbSrc(r)" loading="lazy" alt="" />
        <div v-else class="pcp-ph">🖼</div>
        <figcaption v-if="r.location" class="pcp-cap">{{ r.location }}</figcaption>
      </figure>
    </div>
    <!-- 追加页哨兵（滚近底部 400px 自动热加载下一页） -->
    <div
      v-if="!loading && !loadError && items.length && !done"
      ref="sentinelEl"
      class="pcp-more-sentinel"
    >
      <span v-if="loadingMore" class="pcp-more-tip">加载中…</span>
    </div>

    <!-- 大图看图器（纯浏览：删除/归档走相册与分类页，不进通用集合） -->
    <PhotoLightbox
      v-if="lightboxOpen"
      :photos="lightboxPhotos"
      :index="lightboxIndex"
      @close="lightboxOpen = false"
    />
  </div>
</template>

<style scoped>
.pcp-wrap {
  min-width: 0;
  padding: 0 var(--sp-6, 24px) 48px;
}

/* ---- 页头 ---- */
.pcp-head {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 12px;
  padding: 14px 0 0;
}
.pcp-title {
  margin: 0;
  font-size: 19px;
  font-weight: 700;
}
.pcp-count {
  font-size: 12.5px;
  opacity: 0.65;
  font-variant-numeric: tabular-nums;
}
.pcp-sub {
  font-size: 12px;
  opacity: 0.55;
}
.pcp-spacer {
  flex: 1;
}

/* ---- 三态 ---- */
.pcp-state {
  padding: 36px 10px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}
.pcp-state-icon {
  font-size: 42px;
}
.pcp-state p {
  margin: 0;
  font-size: 13px;
  opacity: 0.75;
  max-width: 460px;
  line-height: 1.6;
}

/* 骨架 */
.pcp-skel-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 10px;
  width: 100%;
}
.pcp-skel {
  aspect-ratio: 1;
  border-radius: 10px;
  background: rgba(127, 127, 127, 0.16);
  animation: pcp-pulse 1.2s infinite;
}
@keyframes pcp-pulse {
  50% { opacity: 0.4; }
}

/* ---- 照片网格（与分类画廊 photo-grid 同一视觉语言） ---- */
.pcp-grid {
  display: grid;
  gap: 10px;
}
.pcp-cell {
  margin: 0;
  position: relative;
  aspect-ratio: 1;
  border-radius: 10px;
  overflow: hidden;
  cursor: zoom-in;
  background: rgba(127, 127, 127, 0.12);
  transition: transform 0.15s ease;
}
.pcp-cell:hover {
  transform: translateY(-2px);
}
.pcp-cell img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.pcp-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 28px;
  opacity: 0.6;
}
.pcp-cap {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 14px 8px 6px;
  font-size: 11px;
  color: #fff;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.6));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  pointer-events: none;
}

/* ---- 追加页哨兵 ---- */
.pcp-more-sentinel {
  display: flex;
  justify-content: center;
  align-items: center;
  min-height: 28px;
  padding: 6px 0 2px;
}
.pcp-more-tip {
  font-size: 12px;
  opacity: 0.6;
}
</style>
