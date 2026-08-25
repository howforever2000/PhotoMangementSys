<script setup lang="ts">
/**
 * 「回忆」页面 —— 百度网盘智能相册风格
 *
 * 数据源：
 *  - `list_timeline` 跨相册按拍摄时间聚合的全部已扫描照片
 *  - `list_persons` 人物注册表（取出现次数 top N 作「近期人物」）
 *  - 主页：渐变 Hero + 故事海报（按月聚合）+ 本月精选 + 年度回顾横滚
 *
 * 视觉参考：
 *  - 故事卡：3:4 大图、渐变叠加、月份 + 张数 + 主地点，水平滚动
 *  - 年度回顾：每年一张 16:9 大图 + 年份 + 张数 + 主人物
 *  - Hero：紫蓝渐变 + 关键统计（总照片/总人物/总相册/本月）
 */
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useAlbumStore } from "../stores/album";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";
import type { ContentSearchHit } from "../types/content";
import type { PersonInfo } from "../types/photo";
import PhotoLightbox from "../components/PhotoLightbox.vue";

const router = useRouter();
const store = useAlbumStore();
const theme = useThemeStore();
const notify = useNotify();

const loading = ref(true);
const error = ref("");
const rows = ref<ContentSearchHit[]>([]);
const persons = ref<PersonInfo[]>([]);
/** path → 已缓存缩略图路径（用现有网格缩略图管线） */
const thumbMap = ref<Record<string, string>>({});

function fileUrl(p: string) {
  return p ? convertFileSrc(p) : "";
}

/* -------------------- 数据聚合 -------------------- */
interface MonthGroup {
  key: string; // "2025-08"
  year: number;
  month: number; // 1~12
  label: string; // "2025 年 8 月"
  items: ContentSearchHit[];
  /** 用于故事卡封面的代表照片 */
  hero: ContentSearchHit | null;
  /** 主地点：出现最多的 location */
  topLocation: string;
}
interface YearGroup {
  year: number;
  total: number;
  items: ContentSearchHit[];
  /** 年封面：mid 位置 + 有 person */
  hero: ContentSearchHit | null;
}

/** 把 timeline 数据按年→月聚合，同时挑故事卡封面 */
const monthGroups = computed<MonthGroup[]>(() => {
  const map = new Map<string, ContentSearchHit[]>();
  for (const r of rows.value) {
    if (!r.shoot_time) continue;
    const key = r.shoot_time.slice(0, 7); // "YYYY-MM"
    if (!map.has(key)) map.set(key, []);
    map.get(key)!.push(r);
  }
  const out: MonthGroup[] = [];
  for (const [key, items] of map) {
    const [y, m] = key.split("-").map(Number);
    // 故事卡封面优先：含人脸 → 有地点 → 较新
    const hero = pickHero(items);
    const locCount = new Map<string, number>();
    for (const it of items) {
      if (it.location) locCount.set(it.location, (locCount.get(it.location) ?? 0) + 1);
    }
    let topLocation = "";
    let topCount = 0;
    for (const [loc, n] of locCount) if (n > topCount) { topLocation = loc; topCount = n; }
    out.push({
      key,
      year: y,
      month: m,
      label: `${y} 年 ${m} 月`,
      items: items.sort((a, b) => (a.shoot_time || "").localeCompare(b.shoot_time || "")),
      hero,
      topLocation,
    });
  }
  out.sort((a, b) => b.key.localeCompare(a.key));
  return out;
});

/** 按年聚合（仅用于「年度回顾」模块） */
const yearGroups = computed<YearGroup[]>(() => {
  const map = new Map<number, ContentSearchHit[]>();
  for (const r of rows.value) {
    if (!r.shoot_time) continue;
    const y = Number(r.shoot_time.slice(0, 4));
    if (!map.has(y)) map.set(y, []);
    map.get(y)!.push(r);
  }
  const out: YearGroup[] = [];
  for (const [year, items] of map) {
    out.push({ year, total: items.length, items, hero: pickHero(items) });
  }
  out.sort((a, b) => b.year - a.year);
  return out;
});

/** 当前月：含数据则展示「本月精选」 */
const currentMonthKey = computed(() => {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`;
});
const thisMonth = computed(() => monthGroups.value.find((m) => m.key === currentMonthKey.value));
const thisMonthItems = computed(() => (thisMonth.value ? thisMonth.value.items.slice(0, 12) : []));

/** 关键统计：总照片/总人物/总相册/本月新增 */
const stats = computed(() => {
  const total = rows.value.length;
  const personTotal = persons.value.length;
  const albumTotal = store.albums.length;
  const thisMonthTotal = thisMonth.value ? thisMonth.value.items.length : 0;
  return { total, personTotal, albumTotal, thisMonthTotal };
});

/** 故事卡封面挑选：含人脸(>0) → 有地点 → 时间居中（取中位附近） */
function pickHero(items: ContentSearchHit[]): ContentSearchHit | null {
  if (!items.length) return null;
  const withFace = items.filter((r) => r.person_ids && r.person_ids.length > 0);
  const pool = withFace.length ? withFace : items;
  // 选时间居中（避免首张永远是月初）
  const mid = pool[Math.floor(pool.length / 2)];
  return mid;
}

/* -------------------- 缩略图加载 -------------------- */
async function loadThumbs() {
  const byAlbum = new Map<number, string[]>();
  for (const r of rows.value) {
    const aid = r.album_id ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    if (!thumbMap.value[r.path]) byAlbum.get(aid)!.push(r.path);
  }
  await Promise.all(
    [...byAlbum.keys()].map(async (aid) => {
      const paths = byAlbum.get(aid) ?? [];
      if (!paths.length) return;
      try {
        const pairs = await invoke<[string, string][]>("get_photo_thumbs", { albumId: aid, paths });
        for (const [path, thumb] of pairs) if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
      } catch {
        /* 缺图不阻塞 */
      }
    }),
  );
}

/* -------------------- Hero 渐变色（按月 key 分散到柔和调色板） -------------------- */
const PALETTE = [
  "linear-gradient(135deg, #6a8df0 0%, #a764ec 100%)",
  "linear-gradient(135deg, #ff7e5f 0%, #feb47b 100%)",
  "linear-gradient(135deg, #43cea2 0%, #185a9d 100%)",
  "linear-gradient(135deg, #f6d365 0%, #fda085 100%)",
  "linear-gradient(135deg, #8e2de2 0%, #4a00e0 100%)",
  "linear-gradient(135deg, #00c6ff 0%, #0072ff 100%)",
  "linear-gradient(135deg, #f093fb 0%, #f5576c 100%)",
  "linear-gradient(135deg, #5ee7df 0%, #b490ca 100%)",
];
function paletteFor(key: string): string {
  let h = 0;
  for (const ch of key) h = (h * 31 + ch.charCodeAt(0)) | 0;
  return PALETTE[Math.abs(h) % PALETTE.length];
}

/* -------------------- 看图器 -------------------- */
const lightboxOpen = ref(false);
const lightboxIndex = ref(0);
const lightboxPhotos = computed(() => rows.value.map((r) => ({ path: r.path, albumId: r.album_id })));
function openLightbox(photoPath: string) {
  const idx = rows.value.findIndex((r) => r.path === photoPath);
  if (idx < 0) return;
  lightboxIndex.value = idx;
  lightboxOpen.value = true;
}

/* -------------------- 故事卡点击 → 跳到时间线页并定位月份 -------------------- */
/**
 * FEAT-E：带 query 跳转，Timeline 页会读 year + month 自动滚动 / 展开 / 高亮。
 * - 故事卡：跳到对应 yyyy-MM 月份。
 * - 年度回顾：跳到对应年份（Timeline 页会按年自动选中并定位）。
 */
function gotoMonth(m: MonthGroup) {
  router.push({ path: "/timeline", query: { year: String(m.year), month: String(m.month) } });
}

function gotoYear(y: YearGroup) {
  router.push({ path: "/timeline", query: { year: String(y.year) } });
}

/* -------------------- 初始化 -------------------- */
onMounted(async () => {
  try {
    const [tl, pl] = await Promise.all([
      invoke<ContentSearchHit[]>("list_timeline"),
      invoke<PersonInfo[]>("list_persons"),
    ]);
    rows.value = tl;
    persons.value = pl;
    // 同时确保 store 有最新相册列表（统计用）
    if (!store.albums.length) {
      store.fetchAlbums().catch(() => {});
    }
    await loadThumbs();
  } catch (e) {
    error.value = String(e);
    notify.error("加载回忆失败", String(e));
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div class="memories-page" :style="{ color: theme.textColor }">
    <!-- 顶部返回 -->
    <button class="btn mem-back" @click="router.push('/home')">← 主页</button>

    <!-- Hero：渐变背景 + 标题 + 关键统计 -->
    <section class="mem-hero" :style="{ background: 'linear-gradient(135deg, #6a8df0 0%, #a764ec 50%, #f093fb 100%)' }">
      <div class="mem-hero-mask"></div>
      <div class="mem-hero-content">
        <div class="mem-hero-eyebrow">SMART MEMORIES</div>
        <h1 class="mem-hero-title">回忆</h1>
        <p class="mem-hero-sub">把每一段时光重新翻出来 —— 按月 / 按年 / 按人物。</p>
        <div class="mem-stats">
          <div class="stat-cell">
            <span class="stat-num">{{ stats.total }}</span>
            <span class="stat-label">张照片</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat-cell">
            <span class="stat-num">{{ stats.personTotal }}</span>
            <span class="stat-label">位人物</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat-cell">
            <span class="stat-num">{{ stats.albumTotal }}</span>
            <span class="stat-label">个相册</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat-cell">
            <span class="stat-num">{{ stats.thisMonthTotal }}</span>
            <span class="stat-label">本月新增</span>
          </div>
        </div>
      </div>
    </section>

    <!-- 加载 -->
    <div v-if="loading" class="mem-loading">
      <div class="sk-hero"></div>
      <div class="sk-row">
        <div v-for="i in 3" :key="i" class="sk-card"></div>
      </div>
    </div>

    <!-- 错误 -->
    <div v-else-if="error" class="mem-empty">
      <div class="mem-empty-icon">⚠️</div>
      <p>加载失败：{{ error }}</p>
    </div>

    <!-- 空数据 -->
    <div v-else-if="!rows.length" class="mem-empty">
      <div class="mem-empty-icon">🌅</div>
      <p class="mem-empty-title">还没有可展示的回忆</p>
      <p class="mem-empty-text">请先在相册详情页执行「综合扫描」，将照片的拍摄时间 / 地点 / 人物信息写入数据库。</p>
      <button class="btn" @click="router.push('/albums')">去相册扫描</button>
    </div>

    <!-- 故事卡：按月聚合，水平滚动 -->
    <section v-else class="mem-section">
      <header class="mem-section-head">
        <h2>故事 · 按月</h2>
        <span class="mem-section-sub">横向滚动查看所有月份</span>
      </header>
      <div class="mem-row">
        <article
          v-for="m in monthGroups"
          :key="m.key"
          class="mem-story"
          :style="{ background: paletteFor(m.key) }"
          @click="gotoMonth(m)"
        >
          <div class="mem-story-photo">
            <img
              v-if="m.hero && thumbMap[m.hero.path]"
              :src="fileUrl(thumbMap[m.hero.path])"
              loading="lazy"
              alt=""
            />
            <div v-else class="mem-story-ph">📷</div>
            <div class="mem-story-fade"></div>
          </div>
          <div class="mem-story-body">
            <h3 class="mem-story-title">{{ m.label }}</h3>
            <p class="mem-story-meta">
              <span>{{ m.items.length }} 张</span>
              <span v-if="m.topLocation">· 📍 {{ m.topLocation }}</span>
            </p>
            <span class="mem-story-cta">查看全部 →</span>
          </div>
        </article>
      </div>
    </section>

    <!-- 本月精选（若有当月数据） -->
    <section v-if="thisMonth" class="mem-section">
      <header class="mem-section-head">
        <h2>本月精选</h2>
        <span class="mem-section-sub">{{ thisMonth.label }} · {{ thisMonth.items.length }} 张</span>
      </header>
      <div class="mem-grid">
        <figure
          v-for="r in thisMonthItems"
          :key="r.id"
          class="mem-photo"
          :title="[r.label, r.location].filter(Boolean).join(' · ')"
          @click="openLightbox(r.path)"
        >
          <img v-if="thumbMap[r.path]" :src="fileUrl(thumbMap[r.path])" loading="lazy" alt="" />
          <div v-else class="mem-ph">🖼</div>
          <figcaption v-if="r.label || r.location" class="mem-cap">
            <span v-if="r.label">{{ r.label }}</span>
            <span v-if="r.location">📍 {{ r.location }}</span>
          </figcaption>
        </figure>
      </div>
    </section>

    <!-- 年度回顾：按年横滚，每行一张大封面 -->
    <section v-if="yearGroups.length" class="mem-section">
      <header class="mem-section-head">
        <h2>年度回顾</h2>
        <span class="mem-section-sub">精选每年代表性瞬间</span>
      </header>
      <div class="mem-year-row">
        <article
          v-for="y in yearGroups"
          :key="y.year"
          class="mem-year"
          :style="{ background: paletteFor(String(y.year)) }"
          @click="gotoYear(y)"
        >
          <div class="mem-year-photo">
            <img
              v-if="y.hero && thumbMap[y.hero.path]"
              :src="fileUrl(thumbMap[y.hero.path])"
              loading="lazy"
              alt=""
            />
            <div v-else class="mem-story-ph">📷</div>
            <div class="mem-year-fade"></div>
          </div>
          <div class="mem-year-body">
            <h3 class="mem-year-title">{{ y.year }}</h3>
            <p class="mem-year-meta">{{ y.total }} 张 · 点击查看时间线</p>
          </div>
        </article>
      </div>
    </section>

    <!-- 近期人物 -->
    <section v-if="persons.length" class="mem-section">
      <header class="mem-section-head">
        <h2>近期人物</h2>
        <span class="mem-section-sub">出现次数 top 8</span>
      </header>
      <div class="mem-person-row">
        <div v-for="p in persons.slice(0, 8)" :key="p.id" class="mem-person">
          <div class="mem-person-avatar">
            <div class="mem-person-fb">{{ p.name.slice(0, 1) }}</div>
          </div>
          <div class="mem-person-name">{{ p.name }}</div>
          <div class="mem-person-count">{{ p.face_count }} 次</div>
        </div>
      </div>
    </section>

    <!-- 看图器：仅传原图路径（meta 可选；timeline 中使用轻量场景不需） -->
    <PhotoLightbox
      v-if="lightboxOpen"
      :photos="lightboxPhotos"
      :index="lightboxIndex"
      @close="lightboxOpen = false"
    />
  </div>
</template>

<style scoped>
.memories-page {
  max-width: 1200px;
  margin: 0 auto;
  padding: 20px;
  min-height: 100vh;
  box-sizing: border-box;
}
.mem-back {
  margin-bottom: 14px;
}

/* ---- Hero ---- */
.mem-hero {
  position: relative;
  height: 220px;
  border-radius: 20px;
  overflow: hidden;
  margin-bottom: 28px;
  color: #fff;
  box-shadow: 0 10px 30px rgba(106, 141, 240, 0.25);
}
.mem-hero-mask {
  position: absolute;
  inset: 0;
  background: radial-gradient(circle at 80% 20%, rgba(255, 255, 255, 0.25), transparent 60%);
  pointer-events: none;
}
.mem-hero-content {
  position: relative;
  height: 100%;
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 0 36px;
}
.mem-hero-eyebrow {
  font-size: 12px;
  letter-spacing: 3px;
  opacity: 0.85;
  margin-bottom: 6px;
}
.mem-hero-title {
  font-size: 38px;
  margin: 0;
  font-weight: 800;
  letter-spacing: 4px;
  text-shadow: 0 2px 12px rgba(0, 0, 0, 0.18);
}
.mem-hero-sub {
  margin: 6px 0 18px;
  font-size: 14px;
  opacity: 0.9;
}
.mem-stats {
  display: flex;
  align-items: center;
  gap: 18px;
  flex-wrap: wrap;
}
.stat-cell {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 64px;
}
.stat-num {
  font-size: 22px;
  font-weight: 700;
  text-shadow: 0 1px 4px rgba(0, 0, 0, 0.2);
}
.stat-label {
  font-size: 12px;
  opacity: 0.9;
}
.stat-divider {
  width: 1px;
  height: 28px;
  background: rgba(255, 255, 255, 0.35);
}

/* ---- Section 通用 ---- */
.mem-section {
  margin-bottom: 30px;
}
.mem-section-head {
  display: flex;
  align-items: baseline;
  gap: 12px;
  margin-bottom: 14px;
}
.mem-section-head h2 {
  margin: 0;
  font-size: 20px;
  font-weight: 700;
}
.mem-section-sub {
  font-size: 12px;
  opacity: 0.7;
}

/* ---- 故事行（月度）水平滚动 ---- */
.mem-row {
  display: flex;
  gap: 16px;
  overflow-x: auto;
  overflow-y: hidden;
  padding: 6px 2px 14px;
  scroll-snap-type: x proximity;
}
.mem-row::-webkit-scrollbar {
  height: 8px;
}
.mem-row::-webkit-scrollbar-thumb {
  background: rgba(127, 127, 127, 0.25);
  border-radius: 4px;
}

.mem-story {
  flex: 0 0 220px;
  scroll-snap-align: start;
  height: 280px;
  border-radius: 16px;
  overflow: hidden;
  position: relative;
  cursor: pointer;
  color: #fff;
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.15);
  transition: transform 0.18s ease, box-shadow 0.18s ease;
}
.mem-story:hover {
  transform: translateY(-3px);
  box-shadow: 0 12px 26px rgba(0, 0, 0, 0.25);
}
.mem-story-photo {
  position: absolute;
  inset: 0;
}
.mem-story-photo img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.mem-story-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 42px;
  opacity: 0.45;
}
.mem-story-fade {
  position: absolute;
  inset: 0;
  background: linear-gradient(180deg, rgba(0, 0, 0, 0) 50%, rgba(0, 0, 0, 0.65) 100%);
}
.mem-story-body {
  position: absolute;
  inset: auto 0 0 0;
  padding: 14px 16px 16px;
  z-index: 1;
}
.mem-story-title {
  margin: 0 0 4px;
  font-size: 18px;
  font-weight: 700;
  text-shadow: 0 1px 4px rgba(0, 0, 0, 0.4);
}
.mem-story-meta {
  margin: 0 0 8px;
  font-size: 12.5px;
  opacity: 0.92;
  display: flex;
  gap: 4px;
  flex-wrap: wrap;
}
.mem-story-cta {
  display: inline-block;
  font-size: 12px;
  padding: 4px 10px;
  background: rgba(255, 255, 255, 0.18);
  border: 1px solid rgba(255, 255, 255, 0.45);
  border-radius: 999px;
  backdrop-filter: blur(4px);
  transition: background 0.15s;
}
.mem-story:hover .mem-story-cta {
  background: rgba(255, 255, 255, 0.3);
}

/* ---- 本月精选 网格 ---- */
.mem-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 10px;
}
.mem-photo {
  margin: 0;
  position: relative;
  aspect-ratio: 1 / 1;
  border-radius: 10px;
  overflow: hidden;
  cursor: pointer;
  background: rgba(127, 127, 127, 0.1);
  transition: transform 0.12s ease;
}
.mem-photo:hover {
  transform: translateY(-2px);
}
.mem-photo img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.mem-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 30px;
  opacity: 0.6;
}
.mem-cap {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 6px 8px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: 11px;
  color: #fff;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.65));
}

/* ---- 年度回顾 ---- */
.mem-year-row {
  display: flex;
  gap: 16px;
  overflow-x: auto;
  padding: 6px 2px 14px;
}
.mem-year-row::-webkit-scrollbar {
  height: 8px;
}
.mem-year-row::-webkit-scrollbar-thumb {
  background: rgba(127, 127, 127, 0.25);
  border-radius: 4px;
}
.mem-year {
  flex: 0 0 320px;
  height: 200px;
  border-radius: 16px;
  overflow: hidden;
  position: relative;
  color: #fff;
  cursor: pointer;
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
  transition: transform 0.18s ease, box-shadow 0.18s ease;
}
.mem-year:hover {
  transform: translateY(-3px);
  box-shadow: 0 12px 26px rgba(0, 0, 0, 0.28);
}
.mem-year-photo {
  position: absolute;
  inset: 0;
}
.mem-year-photo img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.mem-year-fade {
  position: absolute;
  inset: 0;
  background: linear-gradient(180deg, rgba(0, 0, 0, 0.1) 0%, rgba(0, 0, 0, 0.55) 100%);
}
.mem-year-body {
  position: absolute;
  inset: auto 0 0 0;
  padding: 16px 20px;
  z-index: 1;
}
.mem-year-title {
  margin: 0 0 4px;
  font-size: 28px;
  font-weight: 800;
  letter-spacing: 1px;
  text-shadow: 0 2px 6px rgba(0, 0, 0, 0.35);
}
.mem-year-meta {
  margin: 0;
  font-size: 13px;
  opacity: 0.95;
}

/* ---- 近期人物 ---- */
.mem-person-row {
  display: flex;
  gap: 14px;
  overflow-x: auto;
  padding: 6px 2px 14px;
}
.mem-person-row::-webkit-scrollbar {
  height: 8px;
}
.mem-person-row::-webkit-scrollbar-thumb {
  background: rgba(127, 127, 127, 0.25);
  border-radius: 4px;
}
.mem-person {
  flex: 0 0 92px;
  text-align: center;
  cursor: pointer;
}
.mem-person-avatar {
  width: 72px;
  height: 72px;
  margin: 0 auto 6px;
  border-radius: 50%;
  overflow: hidden;
  background: linear-gradient(135deg, #6a8df0, #a764ec);
  display: flex;
  align-items: center;
  justify-content: center;
  color: #fff;
  font-size: 28px;
  font-weight: 600;
  box-shadow: 0 4px 10px rgba(0, 0, 0, 0.15);
}
.mem-person-avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.mem-person-name {
  font-size: 13px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.mem-person-count {
  font-size: 11px;
  opacity: 0.6;
}

/* ---- 加载 / 空态 ---- */
.mem-loading {
  padding: 20px 0;
}
.sk-hero,
.sk-card {
  background: rgba(127, 127, 127, 0.18);
  border-radius: 12px;
  animation: skpulse 1.2s infinite;
}
.sk-hero {
  height: 180px;
  margin-bottom: 20px;
}
.sk-row {
  display: flex;
  gap: 14px;
}
.sk-card {
  flex: 0 0 200px;
  height: 240px;
}
@keyframes skpulse {
  50% { opacity: 0.4; }
}
.mem-empty {
  text-align: center;
  padding: 80px 20px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}
.mem-empty-icon {
  font-size: 52px;
}
.mem-empty-title {
  font-size: 18px;
  font-weight: 600;
  margin: 0;
}
.mem-empty-text {
  opacity: 0.7;
  max-width: 460px;
  line-height: 1.6;
  margin: 0;
}

@media (max-width: 640px) {
  .memories-page { padding: 12px; }
  .mem-hero { height: 200px; }
  .mem-hero-content { padding: 0 22px; }
  .mem-hero-title { font-size: 30px; }
  .mem-story { flex: 0 0 180px; height: 240px; }
  .mem-year { flex: 0 0 260px; height: 170px; }
  .mem-person { flex: 0 0 80px; }
}
</style>
