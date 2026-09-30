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
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
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
/**
 * 只为「给定子集」补缩略图（按 album_id 分组批量取，复用指纹缓存）。
 * 不再一次性对全量时间线照片生成缩略图 —— 照片一多首屏非常重；
 * 改为按需：首屏只取封面/本月精选，打开月度浏览框时再取该月。
 */
async function loadThumbs(items: ContentSearchHit[]) {
  const byAlbum = new Map<number, string[]>();
  for (const r of items) {
    if (thumbMap.value[r.path]) continue; // 已有缓存：跳过
    const aid = r.album_id ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    byAlbum.get(aid)!.push(r.path);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, paths]) => {
      if (!paths.length) return;
      try {
        // 分批（300/批）：单次 IPC 载荷可控，大批量月也能边滚边补
        for (let i = 0; i < paths.length; i += 300) {
          const pairs = await invoke<[string, string][]>("get_photo_thumbs", {
            albumId: aid,
            paths: paths.slice(i, i + 300),
          });
          for (const [path, thumb] of pairs) if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
        }
      } catch {
        /* 缺图不阻塞：卡片回退占位 */
      }
    }),
  );
}

/* -------------------- 近期人物（FEAT-046：真实头像 + 点击跳转） -------------------- */
/** 展示前 8 位；pid → 头像 asset URL（获取失败无键，回退首字占位） */
const topPersons = computed(() => persons.value.slice(0, 8));
const personAvatarMap = ref<Record<string, string>>({});

async function loadPersonAvatars() {
  // BUG-2026-0910-002 同源（N+1 往返）：改用批量命令，1 次往返替代逐个 8 次 +
  // 8 次逐条重渲染；未命中（需现场裁剪）保留首字占位，点击跳转后由画廊按需补
  const pids = topPersons.value.filter((p) => !personAvatarMap.value[p.id]).map((p) => p.id);
  if (!pids.length) return;
  try {
    const paths = await invoke<(string | null)[]>("get_person_avatars_bulk", { pids });
    const next = { ...personAvatarMap.value };
    pids.forEach((pid, i) => {
      const path = paths[i];
      if (path) next[pid] = convertFileSrc(path);
    });
    personAvatarMap.value = next;
  } catch {
    /* 无代表脸 / 原图缺失：保留首字占位 */
  }
}

/** 点击人物 → 智慧相册人物 tab 并自动打开该人物照片（PersonGallery focusPid） */
function gotoPerson(p: PersonInfo) {
  router.push({ path: "/smart", query: { tab: "face", person: p.id } });
}

/* -------------------- 故事/年度卡底色（鲜艳彩虹 · 用户色卡） --------------------
   六个基色来自用户提供的色卡：#FFFCBD / #F075C7 / #65D5F9 / #FB6D9B / #505FDD / #3A36E4；
   文字是白字，底部靠 .mem-story-fade / .mem-year-fade 的黑色 scrim 保对比度。 */
const PALETTE = [
  "linear-gradient(135deg, #FFFCBD 0%, #F075C7 100%)",
  "linear-gradient(135deg, #65D5F9 0%, #FB6D9B 100%)",
  "linear-gradient(135deg, #F075C7 0%, #505FDD 100%)",
  "linear-gradient(135deg, #FB6D9B 0%, #3A36E4 100%)",
  "linear-gradient(135deg, #65D5F9 0%, #F075C7 100%)",
  "linear-gradient(135deg, #FFFCBD 0%, #65D5F9 100%)",
  "linear-gradient(135deg, #FB6D9B 0%, #505FDD 100%)",
  "linear-gradient(135deg, #505FDD 0%, #3A36E4 100%)",
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

/* -------------------- 故事卡点击 → 打开「月度浏览框」（只加载该月） -------------------- */
/**
 * 点月回忆不再跳时间线页：时间线要拉全量照片 + 全量缩略图，计算很重；
 * 改为在本页弹出该月的照片浏览框（参考相册详情页「缩略图浏览」框：
 * 框内滚动 + 右下回到顶部 + 点击开大图），只补该月缺失的缩略图。
 */
const monthBox = ref<MonthGroup | null>(null);
const monthBoxEl = ref<HTMLElement | null>(null);
const mbShowTop = ref(false);
const mbLoading = ref(false);

async function openMonthBox(m: MonthGroup) {
  monthBox.value = m;
  mbShowTop.value = false;
  window.addEventListener("keydown", onMonthBoxKey, true); // capture：先于全局 ESC（否则会 router.back 退出本页）
  document.body.style.overflow = "hidden"; // 背景页锁滚
  await nextTick();
  monthBoxEl.value?.scrollTo({ top: 0 });
  // 只为该月补缺失缩略图（已有缓存的直接复用）
  if (m.items.some((r) => !thumbMap.value[r.path])) {
    mbLoading.value = true;
    try {
      await loadThumbs(m.items);
    } finally {
      mbLoading.value = false;
    }
  }
}

function closeMonthBox() {
  if (!monthBox.value) return;
  monthBox.value = null;
  document.body.style.overflow = "";
  window.removeEventListener("keydown", onMonthBoxKey, true);
}

function onMonthBoxKey(e: KeyboardEvent) {
  if (e.key !== "Escape" || !monthBox.value) return;
  // 上层还有弹层（灯箱/右键菜单/确认框）时交由它们自己处理，只关最上层
  if (document.querySelector(".lb-overlay, .context-menu, .dialog-mask, .pm-modal")) return;
  e.preventDefault(); // 让全局 ESC 识别为「页面已拦截」，不触发 router.back
  e.stopPropagation();
  closeMonthBox();
}

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onMonthBoxKey, true);
  document.body.style.overflow = "";
});

function onMbScroll() {
  mbShowTop.value = (monthBoxEl.value?.scrollTop ?? 0) > 300;
}
function mbToTop() {
  monthBoxEl.value?.scrollTo({ top: 0, behavior: "smooth" });
}

/* -------------------- 仍需整条时间线时（跨月对比）才跳时间线页 -------------------- */
/**
 * FEAT-E：带 query 跳转，Timeline 页会读 year + month 自动滚动 / 展开 / 高亮。
 * - 月度浏览框右上角「时间线中定位」用：需要跨月浏览时才走这里。
 * - month 必须传两位 "MM"（Timeline 分组 id 为 `y-{y}-m-{MM}`）。
 */
function gotoMonth(m: MonthGroup) {
  closeMonthBox();
  router.push({ path: "/timeline", query: { year: String(m.year), month: m.key.slice(5, 7) } });
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
    // FEAT-046：近期人物头像异步填充（不阻塞首屏）
    void loadPersonAvatars();
    // 同时确保 store 有最新相册列表（统计用）
    if (!store.albums.length) {
      store.fetchAlbums().catch(() => {});
    }
    await loadThumbs(initialThumbItems());
  } catch (e) {
    error.value = String(e);
    notify.error("加载回忆失败", String(e));
  } finally {
    loading.value = false;
  }
});

/** 首屏只需：故事卡封面 + 年度卡封面 + 本月精选（其余按打开浏览框时再补） */
function initialThumbItems(): ContentSearchHit[] {
  const covers = [
    ...monthGroups.value.map((m) => m.hero),
    ...yearGroups.value.map((y) => y.hero),
  ].filter((x): x is ContentSearchHit => !!x);
  return [...covers, ...thisMonthItems.value];
}
/** 大图工具栏「📁 在相册中查看」：关掉看图器，跳相册并带 ?focus 定位高亮该照片 */
function goAlbumFromLightbox(albumId: number) {
  lightboxOpen.value = false;
  const path = lightboxPhotos.value[lightboxIndex.value]?.path;
  router.push({ path: `/album/${albumId}`, query: path ? { focus: path } : undefined });
}
</script>

<template>
  <div class="memories-page" :style="{ color: theme.textColor }">
    <!-- 顶部返回 -->
    <button class="btn mem-back" @click="router.push('/home')">← 主页</button>

    <!-- Hero：彩虹横幅（按需求保留）；自带 45% 暗纱保证白字 ≥4.5:1 -->
    <section
      class="mem-hero glass-surface candy-surface"
      :style="{
        background:
          'linear-gradient(135deg, #FFFCBD 0%, #65D5F9 55%, #F075C7 100%)',
      }"
    >
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
          class="mem-story candy-surface"
          :style="{ background: paletteFor(m.key) }"
          @click="openMonthBox(m)"
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
          class="mem-year candy-surface"
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
        <div
          v-for="p in topPersons"
          :key="p.id"
          class="mem-person"
          :title="`查看 ${p.name} 的照片`"
          @click="gotoPerson(p)"
        >
          <div class="mem-person-avatar">
            <img
              v-if="personAvatarMap[p.id]"
              :src="personAvatarMap[p.id]"
              alt=""
            />
            <div v-else class="mem-person-fb">{{ p.name.slice(0, 1) }}</div>
          </div>
          <div class="mem-person-name">{{ p.name }}</div>
          <div class="mem-person-count">{{ p.face_count }} 次</div>
        </div>
      </div>
    </section>

    <!-- 月度浏览框：只含该月照片，框内滚动（参考相册缩略图浏览框）
         不 Teleport：与页内 PhotoLightbox 同处 .app-content 层叠上下文，
         z-index 950 < 灯箱 1000，开大图时不会被盖住 -->
    <div v-if="monthBox" class="mb-mask" @click.self="closeMonthBox">
        <div class="mb-panel" role="dialog" aria-modal="true" :aria-label="`${monthBox.label} 照片浏览`">
          <header class="mb-head">
            <div class="mb-head-main">
              <h3 class="mb-title">{{ monthBox.label }}</h3>
              <span class="mb-sub">{{ monthBox.items.length }} 张</span>
              <span v-if="monthBox.topLocation" class="mb-sub">📍 {{ monthBox.topLocation }}</span>
              <span v-if="mbLoading" class="mb-sub mb-loading">正在生成缩略图…</span>
            </div>
            <div class="mb-actions">
              <button class="mb-btn" title="需要跨月对比时，再去完整时间线定位该月" @click="gotoMonth(monthBox)">📅 时间线定位</button>
              <button class="mb-btn mb-btn-primary" @click="closeMonthBox">✕ 关闭</button>
            </div>
          </header>

          <div v-if="!monthBox.items.length" class="mb-empty">该月暂无可浏览的照片</div>

          <div v-else ref="monthBoxEl" class="mb-scroll" @scroll.passive="onMbScroll">
            <div class="mb-grid">
              <figure
                v-for="r in monthBox.items"
                :key="r.id"
                class="mb-cell"
                :title="[r.label, r.location, r.album_name].filter(Boolean).join(' · ') || r.path"
                @click="openLightbox(r.path)"
              >
                <img
                  v-if="thumbMap[r.path]"
                  :src="fileUrl(thumbMap[r.path])"
                  loading="lazy"
                  decoding="async"
                  alt=""
                  class="mb-img"
                />
                <div v-else class="mb-ph"></div>
                <figcaption v-if="r.label || r.location" class="mb-cap">
                  <span v-if="r.label" class="mb-cap-label">{{ r.label }}</span>
                  <span v-if="r.location">📍 {{ r.location }}</span>
                </figcaption>
              </figure>
            </div>

            <!-- 框内回到顶部箭头（与相册缩略图浏览框一致） -->
            <transition name="mb-top">
              <button v-if="mbShowTop" class="mb-top-btn" title="回到顶部" @click="mbToTop">↑</button>
            </transition>
          </div>
        </div>
    </div>

    <!-- 看图器：仅传原图路径（meta 可选；timeline 中使用轻量场景不需） -->
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

/* ---- Hero：糖果彩虹横幅（与下方故事卡同一色族 · 用户色卡） ---- */
.mem-hero {
  position: relative;
  height: 220px;
  border-radius: var(--radius-card);
  overflow: hidden;
  margin-bottom: 28px;
  background: var(--glass-bg); /* 未内联渐变时的玻璃回退 */
  /* 描边只做兜底：边缘光泽由 ::after 渐变棱环主导（左上亮→右下反光） */
  border: 1px solid rgba(255, 255, 255, 0.14);
  /* 糖果浅底 → 墨色字（与故事卡色族一致） */
  color: #1f2733;
  box-shadow: var(--shadow-2);
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
  font-weight: 600;
  /* 糖果底上的角标：墨色加重量 */
  color: #17202e;
  margin-bottom: 6px;
}
.mem-hero-title {
  font-size: 38px;
  margin: 0;
  font-weight: 800;
  letter-spacing: 4px;
  text-shadow: none;
  color: #1f2733;
}
.mem-hero-sub {
  margin: 6px 0 18px;
  font-size: 14px;
  /* 去掉 opacity，用实色：渐变过渡区上副标题不再发虚 */
  color: #2a3547;
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
  text-shadow: none;
}
.stat-label {
  font-size: 12px;
  opacity: 0.9;
}
.stat-divider {
  width: 1px;
  height: 28px;
  background: rgba(31, 39, 51, 0.28);
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
  /* 深绿底上的弱说明文字提亮一档（原 0.7 偏灰） */
  color: var(--color-text-2);
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
  /* 材质升级：糖果釉面的方向棱线 + 更实的落影（原 0.15 浮而不定） */
  box-shadow: var(--shadow-1);
  transition: transform 0.18s ease, box-shadow 0.18s ease;
}
.mem-story:hover {
  transform: translateY(-3px);
  box-shadow: var(--shadow-2);
}
/* candy-surface 颗粒层必须在照片/暗纱之上（absolute 子元素按 DOM 顺序盖住伪元素） */
.mem-story::before,
.mem-story::after {
  z-index: 1;
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
  /* 鲜艳底 + 白字：黑纱从 27% 就开始起、压到 0.9，把标题区顶到 ≥4.5:1 */
  background: linear-gradient(180deg, rgba(0, 0, 0, 0) 27%, rgba(0, 0, 0, 0.9) 100%);
}
.mem-story-body {
  position: absolute;
  inset: auto 0 0 0;
  padding: 14px 16px 16px;
  z-index: 2;
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
  box-shadow: var(--shadow-1);
  transition: transform 0.18s ease, box-shadow 0.18s ease;
}
.mem-year:hover {
  transform: translateY(-3px);
  box-shadow: var(--shadow-2);
}
/* 同 mem-story：颗粒/高光层提到照片之上，正文再提一级 */
.mem-year::before,
.mem-year::after {
  z-index: 1;
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
  /* 同上：年度卡也是鲜艳底 + 白字 */
  background: linear-gradient(180deg, rgba(0, 0, 0, 0.25) 0%, rgba(0, 0, 0, 0.85) 100%);
}
.mem-year-body {
  position: absolute;
  inset: auto 0 0 0;
  padding: 16px 20px;
  z-index: 2;
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
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  display: flex;
  align-items: center;
  justify-content: center;
  /* 单点强调：首字母用强调色 */
  color: var(--color-link);
  font-size: 28px;
  font-weight: 600;
  box-shadow: var(--shadow-1);
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

/* ---- 月度浏览框（参考相册详情页「缩略图浏览」框） ---- */
.mb-mask {
  position: fixed;
  inset: 0;
  z-index: 950; /* 低于 PhotoLightbox（1000）：大图始终盖在其上 */
  background: rgba(15, 18, 26, 0.62);
  backdrop-filter: blur(3px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  box-sizing: border-box;
}
.mb-panel {
  width: min(1080px, 100%);
  max-height: calc(100vh - 48px);
  display: flex;
  flex-direction: column;
  /* 用全局面板令牌：--color-surface 与 --color-text 成对（深色预设下自动翻暗，
     原写法 --panel-bg 回退 #fff + 深色下浅字 → 白底浅字看不清）。
     --color-surface 本身是半透明玻璃：先垫一层实色 --color-bg 再叠玻璃面，
     页面内容不再透过面板，文字对比度稳定。 */
  background-color: var(--color-bg, #0e211b);
  background-image: linear-gradient(var(--color-surface, #ffffff), var(--color-surface, #ffffff));
  color: var(--color-text, #1f2733);
  border: 1px solid var(--color-border, rgba(127, 127, 127, 0.25));
  border-radius: 16px;
  box-shadow: var(--shadow-2, 0 24px 60px rgba(0, 0, 0, 0.35));
  overflow: hidden;
}
.mb-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
  padding: 14px 18px;
  border-bottom: 1px solid var(--color-border, rgba(127, 127, 127, 0.18));
}
.mb-head-main {
  display: flex;
  align-items: baseline;
  gap: 8px;
  flex-wrap: wrap;
  min-width: 0;
}
.mb-title {
  margin: 0;
  font-size: 18px;
  font-weight: 700;
}
.mb-sub {
  font-size: 12px;
  opacity: 0.72;
}
.mb-loading {
  color: var(--color-link, #396cd8);
  opacity: 1;
}
.mb-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}
.mb-btn {
  padding: 7px 14px;
  border-radius: 8px;
  border: 1px solid var(--color-border, rgba(127, 127, 127, 0.3));
  background: transparent;
  color: inherit;
  cursor: pointer;
  font-size: 13px;
  transition: border-color 0.15s, color 0.15s, background 0.15s;
}
.mb-btn:hover {
  border-color: var(--color-link, #396cd8);
  color: var(--color-link, #396cd8);
}
.mb-btn-primary {
  background: #396cd8;
  border-color: #396cd8;
  color: #fff;
}
.mb-btn-primary:hover {
  background: #2f5cc2;
  border-color: #2f5cc2;
  color: #fff;
}
/* 框内滚动：高度自适应视口，照片多时整页不被拖长 */
.mb-scroll {
  position: relative;
  overflow-y: auto;
  padding: 14px;
  scrollbar-width: thin;
}
.mb-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 10px;
}
.mb-cell {
  margin: 0;
  position: relative;
  aspect-ratio: 1 / 1;
  border-radius: 10px;
  overflow: hidden;
  cursor: pointer;
  background: rgba(127, 127, 127, 0.12);
  transition: transform 0.12s ease;
}
.mb-cell:hover {
  transform: translateY(-2px);
}
.mb-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
/* 未生成缩略图的骨架（尺寸稳定，避免格子跳动） */
.mb-ph {
  position: absolute;
  inset: 0;
  overflow: hidden;
}
.mb-ph::after {
  content: "";
  position: absolute;
  inset: 0;
  background: linear-gradient(100deg, transparent 20%, rgba(255, 255, 255, 0.35) 50%, transparent 80%);
  animation: mbShimmer 1.2s infinite;
}
@keyframes mbShimmer {
  from { transform: translateX(-100%); }
  to { transform: translateX(100%); }
}
.mb-cap {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 6px 8px;
  display: flex;
  flex-direction: column;
  gap: 1px;
  font-size: 11px;
  color: #fff;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.65));
}
.mb-cap span {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.mb-cap-label {
  font-weight: 600;
}
/* 框内回到顶部箭头 */
.mb-top-btn {
  position: sticky;
  bottom: 12px;
  margin-left: auto;
  display: block;
  width: 38px;
  height: 38px;
  border: none;
  border-radius: 50%;
  background: #396cd8;
  color: #fff;
  font-size: 17px;
  cursor: pointer;
  box-shadow: 0 4px 14px rgba(57, 108, 216, 0.45);
  transition: transform 0.15s, background 0.15s;
}
.mb-top-btn:hover {
  background: #2f5bc0;
  transform: translateY(-2px);
}
.mb-top-enter-active,
.mb-top-leave-active {
  transition: opacity 0.2s ease;
}
.mb-top-enter-from,
.mb-top-leave-to {
  opacity: 0;
}
.mb-empty {
  text-align: center;
  padding: 60px 20px;
  opacity: 0.7;
  font-size: 14px;
}

@media (max-width: 640px) {
  .memories-page { padding: 12px; }
  .mem-hero { height: 200px; }
  .mem-hero-content { padding: 0 22px; }
  .mem-hero-title { font-size: 30px; }
  .mem-story { flex: 0 0 180px; height: 240px; }
  .mem-year { flex: 0 0 260px; height: 170px; }
  .mem-person { flex: 0 0 80px; }
  .mb-mask { padding: 10px; }
  .mb-panel { max-height: calc(100vh - 20px); }
  .mb-head { padding: 12px 14px; }
  .mb-grid { grid-template-columns: repeat(auto-fill, minmax(110px, 1fr)); }
}
</style>
