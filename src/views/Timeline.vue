<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useRoute, useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";
import type { ContentSearchHit } from "../types/content";

/**
 * 照片时间线（FEAT-033）—— 跨相册按拍摄时间聚合浏览
 *
 * 数据源：`photo_content_scan`（组合扫描入库），含 path / album_id / shoot_time / location。
 * 缩略图：按 album_id 分组调用 `get_photo_thumbs`（复用指纹缓存），懒加载填充。
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

function fileUrl(p: string): string {
  return p ? convertFileSrc(p) : "";
}

/** 按 album_id 分组批量取缩略图 */
async function loadThumbs() {
  const byAlbum = new Map<number, string[]>();
  for (const r of rows.value) {
    const aid = r.album_id ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    if (!thumbMap.value[r.path]) byAlbum.get(aid)!.push(r.path);
  }
  const albumIds = [...byAlbum.keys()];
  await Promise.all(
    albumIds.map(async (aid) => {
      const paths = byAlbum.get(aid)!;
      if (!paths.length) return;
      try {
        const pairs = await invoke<[string, string][]>("get_photo_thumbs", { albumId: aid, paths });
        for (const [path, thumb] of pairs) {
          if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
        }
      } catch {
        // 单相册失败不阻塞：缺图卡片回退占位
      }
    }),
  );
}

function openCard(r: ContentSearchHit) {
  if (r.album_id != null) {
    router.push(`/album/${r.album_id}`);
  } else {
    void openOriginal(r);
  }
}

async function openOriginal(r: ContentSearchHit) {
  try {
    const { openPath } = await import("@tauri-apps/plugin-opener");
    await openPath(r.path);
  } catch (e) {
    notify.error("无法打开原图", String(e));
  }
}

/** 年份过滤器变化时仅需重算分组（无需重新拉缩略图） */
function onYearFilter() {
  // 分组重算；缩略图已全部缓存，无需重复请求
}

onMounted(async () => {
  try {
    rows.value = await invoke<ContentSearchHit[]>("list_timeline");
    await loadThumbs();
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
  const m = String(route.query.month ?? "").trim();
  if (!y) return;
  // 1. 选中该年（保持其他年折叠效果由过滤器收敛到仅当前年）
  yearFilter.value = y;
  if (m) {
    // 高亮定位到月份
    highlightKey.value = `y-${y}-m-${m}`;
  } else {
    highlightKey.value = `y-${y}`;
  }
  // 等待 computed groups 渲染完
  nextTick(() => {
    const el = document.getElementById(highlightKey.value!);
    if (el) {
      el.scrollIntoView({ behavior: "smooth", block: "start" });
      // 高亮动画 1.8s 后清除
      window.setTimeout(() => {
        highlightKey.value = null;
      }, 1800);
    }
  });
}
</script>

<template>
  <div class="tl-page" :style="{ color: theme.textColor }">
    <header class="tl-header">
      <button class="btn" @click="router.push('/home')">← 返回主页</button>
      <h1 class="tl-title">📅 照片时间线</h1>
      <p class="tl-subtitle">跨相册按拍摄时间聚合浏览（AI 扫描入库的照片）</p>
    </header>

    <!-- 工具条：年份过滤 + 总数 -->
    <div class="tl-toolbar">
      <label class="tl-count">共 {{ totalFiltered }} 张</label>
      <select v-model="yearFilter" class="tl-year" @change="onYearFilter">
        <option value="all">全部年份</option>
        <option v-for="y in years" :key="y" :value="y">{{ y }}</option>
      </select>
    </div>

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
    <div v-else-if="error" class="tl-empty">
      <div class="tl-empty-icon">⚠️</div>
      <p class="tl-empty-text">加载失败：{{ error }}</p>
      <button class="btn" @click="router.push('/home')">返回主页</button>
    </div>

    <!-- 空态 -->
    <div v-else-if="rows.length === 0" class="tl-empty">
      <div class="tl-empty-icon">🌅</div>
      <p class="tl-empty-title">还没有可展示的时间线</p>
      <p class="tl-empty-text">
        请先在相册详情页执行「组合扫描」/「内容扫描”，将照片的拍摄时间写入数据库后再来查看跨相册时间线。
      </p>
      <button class="btn" @click="router.push('/albums')">去相册扫描</button>
    </div>

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
          <span class="tl-year-count">{{ g.months.reduce((n, mg) => n + mg.items.length, 0) }} 张</span>
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
              class="tl-card"
              :title="[r.label, r.location, r.album_name].filter(Boolean).join(' · ')"
              @click="openCard(r)"
            >
              <img v-if="thumbMap[r.path]" :src="fileUrl(thumbMap[r.path])" loading="lazy" class="tl-thumb" alt="" />
              <div v-else class="tl-thumb tl-thumb-ph">🖼️</div>
              <figcaption class="tl-cap">
                <span v-if="r.label" class="tl-label">{{ r.label }}</span>
                <span v-if="r.location" class="tl-loc">📍 {{ r.location }}</span>
                <button class="tl-open" title="打开原图" @click.stop="openOriginal(r)">⧉</button>
              </figcaption>
            </figure>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
.tl-page {
  padding: 20px;
  max-width: 1200px;
  margin: 0 auto;
  min-height: 100vh;
  box-sizing: border-box;
}
.tl-header {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
  margin-bottom: 12px;
}
.tl-title {
  font-size: 22px;
  margin: 0;
  font-weight: 700;
}
.tl-subtitle {
  margin: 0;
  opacity: 0.7;
  font-size: 13px;
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
.tl-year {
  background: var(--card-bg, rgba(127, 127, 127, 0.05));
  border-radius: 12px;
  padding: 16px 16px 4px;
  margin-bottom: 18px;
  transition: background 0.4s, box-shadow 0.4s;
}
/* FEAT-E：跳定位高亮（query 定位 + 滚动后亮色边框 1.8s 淡出） */
.tl-year.tl-highlight,
.tl-month.tl-highlight {
  background: rgba(57, 108, 216, 0.12);
  box-shadow: 0 0 0 3px rgba(57, 108, 216, 0.35) inset;
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
  transition: transform 0.12s;
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
.tl-empty {
  text-align: center;
  padding: 80px 20px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}
.tl-empty-icon {
  font-size: 52px;
}
.tl-empty-title {
  font-size: 18px;
  font-weight: 600;
  margin: 0;
}
.tl-empty-text {
  opacity: 0.7;
  max-width: 460px;
  line-height: 1.6;
  margin: 0;
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
@media (max-width: 640px) {
  .tl-page {
    padding: 12px;
  }
  .tl-grid {
    grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
  }
  .tl-title {
    font-size: 18px;
  }
}
</style>
