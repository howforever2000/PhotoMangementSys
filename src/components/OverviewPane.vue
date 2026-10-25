<script setup lang="ts">
/**
 * OverviewPane —— 智慧相册「全局概览」仪表墙（FEAT-110）
 *
 * 数据源全部为现有命令，零新增后端：
 *   list_timeline（跨相册已扫描照片，含 shoot_time/location/person_ids）
 *   list_persons + get_person_avatars_bulk
 *   list_photo_locations（GPS 聚合）
 *   list_categories + category_index_stats（语义分类与索引覆盖）
 *   album store（照片总数 / 存储 / 相册封面）
 *
 * 布局：12 列 widget 墙（4 个行带）。widget 基类只管材质与占位，
 * 内容全部由 *Core 原子芯承载，芯子在其他页面可直接复用。
 */
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useAlbumStore } from "../stores/album";
import { getThumbs } from "../utils/thumbQueue";
import type { ContentSearchHit, CategoryOverview, CategoryIndexStats, LocationGroupRow } from "../types/content";
import type { PersonInfo } from "../types/photo";
import WidgetGrid from "./ui/widget/WidgetGrid.vue";
import UiWidget from "./ui/widget/UiWidget.vue";
import StatCore from "./ui/widget/StatCore.vue";
import GaugeCore from "./ui/widget/GaugeCore.vue";
import BarsCore from "./ui/widget/BarsCore.vue";
import CoversCore from "./ui/widget/CoversCore.vue";
import FacesCore from "./ui/widget/FacesCore.vue";
import GeoPinsCore from "./ui/widget/GeoPinsCore.vue";
import RankBarsCore from "./ui/widget/RankBarsCore.vue";
import MemoryCore from "./ui/widget/MemoryCore.vue";

const router = useRouter();
const store = useAlbumStore();

const loading = ref(true);
const error = ref("");

const timeline = ref<ContentSearchHit[]>([]);
const persons = ref<PersonInfo[]>([]);
const locations = ref<LocationGroupRow[]>([]);
const categories = ref<CategoryOverview[]>([]);
const catStats = ref<CategoryIndexStats | null>(null);

/** path → 缩略图可渲染 URL（时间线照片走缩略图管线；相册封面直转） */
const thumbMap = ref<Record<string, string>>({});
/** pid → 头像 URL */
const faceMap = ref<Record<string, string>>({});

function fmtSize(bytes: number): string {
  if (!bytes) return "0 B";
  if (bytes >= 1024 ** 3) return (bytes / 1024 ** 3).toFixed(1) + " GB";
  if (bytes >= 1024 ** 2) return (bytes / 1024 ** 2).toFixed(1) + " MB";
  if (bytes >= 1024) return (bytes / 1024).toFixed(1) + " KB";
  return bytes + " B";
}
const fileUrl = (p: string | null | undefined) => (p ? convertFileSrc(p) : "");

/* ───────────────────────── 聚合统计 ───────────────────────── */
const totalPhotos = computed(() => store.albums.reduce((n, a) => n + (a.photo_count || 0), 0));
const totalBytes = computed(() => store.albums.reduce((n, a) => n + (a.size_bytes || 0), 0));
const totalAlbums = computed(() => store.albums.length);
const scanned = computed(() => timeline.value.length);
const indexedRatio = computed(() =>
  catStats.value && catStats.value.known > 0 ? catStats.value.indexed / catStats.value.known : 0,
);
const knownRatio = computed(() => (totalPhotos.value ? scanned.value / totalPhotos.value : 0));

/** 年份跨度 */
const years = computed(() => {
  const ys = timeline.value.map((r) => Number(r.shoot_time?.slice(0, 4))).filter((y) => y > 1900);
  return ys.length ? { min: Math.min(...ys), max: Math.max(...ys) } : null;
});
const yearSpan = computed(() => (years.value ? years.value.max - years.value.min + 1 : 0));
void yearSpan; /* 年份跨度预留：当前模板用 years.min/max，后续“时间跨度”卡可直接用 */

/** 今年月度柱阵（10 月之前填实，未来月不画柱；高亮当月） */
const now = new Date();
const year = now.getFullYear();
const monthBars = computed(() => {
  const counts = new Array(12).fill(0);
  for (const r of timeline.value) {
    if (!r.shoot_time || Number(r.shoot_time.slice(0, 4)) !== year) continue;
    const m = Number(r.shoot_time.slice(5, 7));
    if (m >= 1 && m <= 12) counts[m - 1]++;
  }
  const max = Math.max(1, ...counts);
  return { values: counts.map((c) => c / max), active: now.getMonth(), counts };
});

/** 人物照片覆盖（timeline 行里带 person_ids 的比例） */
const faceRatio = computed(() => {
  if (!scanned.value) return 0;
  const withFace = timeline.value.filter((r) => r.person_ids && r.person_ids.length > 0).length;
  return withFace / scanned.value;
});
const facePhotos = computed(() =>
  timeline.value.filter((r) => r.person_ids && r.person_ids.length > 0).length,
);

/** GPS 覆盖与 Top 城市 */
const namedLoc = computed(() => locations.value.filter((g) => g.location !== null));
const gpsRatio = computed(() => (scanned.value ? namedLoc.value.reduce((n, g) => n + g.count, 0) / scanned.value : 0));
const topCities = computed(() => {
  const rows = [...namedLoc.value].sort((a, b) => b.count - a.count).slice(0, 3);
  const max = rows[0]?.count || 1;
  return rows.map((g) => ({
    /* 地名形如 “四川省·成都” / “重庆市”：优先取 · 后的城市名，再去行政后缀 */
    label: (g.location ?? "").split("·").pop()!.replace(/[省市]$/, "").slice(0, 4),
    value: g.count,
    ratio: g.count / max,
  }));
});

/** 最近导入：时间线倒序前 6 张 */
const recent = computed(() => [...timeline.value].reverse().slice(0, 6));
const recentCovers = computed(() =>
  recent.value.map((r) => ({ src: thumbMap.value[r.path] || "", name: r.album_name ?? "" })),
);

/** 相册拼贴：按张数取前 3 个有封面的相册 */
const albumMosaic = computed(() =>
  [...store.albums]
    .filter((a) => a.cover_path)
    .sort((a, b) => (b.photo_count || 0) - (a.photo_count || 0))
    .slice(0, 3)
    .map((a) => ({ src: fileUrl(a.cover_path), name: a.name })),
);

/** 人物：face_count 前 6（折叠由 FacesCore 处理） */
const topPersons = computed(() => [...persons.value].sort((a, b) => b.face_count - a.face_count).slice(0, 6));
const faceItems = computed(() =>
  topPersons.value.map((p) => ({ src: faceMap.value[p.id] || "", text: (p.name || "?").slice(0, 1) })),
);

/** 分类：启用的语义分类与计数 */
const enabledCats = computed(() => categories.value.filter((c) => c.enabled));
const catCount = computed(() => catStats.value?.semantic_categories ?? enabledCats.value.length);
const catPhotoCount = computed(() => enabledCats.value.reduce((n, c) => n + (c.count || 0), 0));
const catNames = computed(() => enabledCats.value.slice(0, 5).map((c) => c.name).join(" · "));

/** 那年今日（过去年份同月同日） */
const mm = String(now.getMonth() + 1).padStart(2, "0");
const dd = String(now.getDate()).padStart(2, "0");
const onThisDay = computed(() =>
  timeline.value.filter(
    (r) => r.shoot_time?.slice(5, 7) === mm && r.shoot_time?.slice(8, 10) === dd && Number(r.shoot_time.slice(0, 4)) < year,
  ),
);
const memoryHero = computed(() => [...onThisDay.value].reverse()[0] ?? null);
const memoryCover = computed(() => (memoryHero.value ? thumbMap.value[memoryHero.value.path] || "" : ""));
const memoryYears = computed(() => new Set(onThisDay.value.map((r) => r.shoot_time!.slice(0, 4))).size);

async function loadThumb(paths: { albumId: number; path: string }[]) {
  const byAlbum = new Map<number, string[]>();
  for (const { albumId, path } of paths) {
    if (thumbMap.value[path]) continue;
    if (!byAlbum.has(albumId)) byAlbum.set(albumId, []);
    byAlbum.get(albumId)!.push(path);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, ps]) => {
      try {
        for (const [path, thumb] of await getThumbs(aid, ps)) thumbMap.value[path] = convertFileSrc(thumb);
      } catch {
        /* 缩略图失败不阻塞概览 */
      }
    }),
  );
}

async function load() {
  loading.value = true;
  error.value = "";
  try {
    if (!store.albums.length) await store.fetchAlbums();
    const [tl, pl, ll, cl] = await Promise.all([
      invoke<ContentSearchHit[]>("list_timeline"),
      invoke<PersonInfo[]>("list_persons"),
      invoke<LocationGroupRow[]>("list_photo_locations"),
      invoke<CategoryOverview[]>("list_categories"),
    ]);
    timeline.value = tl;
    persons.value = pl;
    locations.value = ll;
    categories.value = cl;
    try {
      catStats.value = await invoke<CategoryIndexStats>("category_index_stats");
    } catch {
      catStats.value = null;
    }

    /* 人物头像（bulk，一次往返） */
    try {
      const top = [...pl].sort((a, b) => b.face_count - a.face_count).slice(0, 6).map((p) => p.id);
      const paths = await invoke<(string | null)[]>("get_person_avatars_bulk", { pids: top });
      top.forEach((pid, i) => {
        if (paths[i]) faceMap.value[pid] = convertFileSrc(paths[i]!);
      });
    } catch {
      /* 无头像则显示姓名首字 */
    }

    /* 缩略图：最近 6 张 + 那年今日封面 */
    const need = [...recent.value, ...(memoryHero.value ? [memoryHero.value] : [])]
      .filter((r) => r.album_id != null)
      .map((r) => ({ albumId: r.album_id!, path: r.path }));
    await loadThumb(need);
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

onMounted(load);

/* ───────────────────────── 导航 ───────────────────────── */
function goFace(pid?: string) {
  router.push({ path: "/smart", query: { tab: "face", ...(pid ? { person: pid } : {}) } });
}
function goLocation() {
  router.push({ path: "/smart", query: { tab: "location" } });
}
function goCategory() {
  router.push({ path: "/smart", query: { tab: "category" } });
}
function goAlbum(id?: number | null) {
  if (id != null) router.push(`/album/${id}`);
}
function onGeoPick() {
  goLocation();
}
</script>

<template>
  <div class="overview">
    <!-- 加载骨架 -->
    <div v-if="loading" class="ov-skel" aria-label="正在汇总照片库">
      <span v-for="i in 9" :key="i" class="ov-skel-card"></span>
    </div>

    <!-- 失败：不阻塞智慧相册其他分区，给重试 -->
    <div v-else-if="error" class="ov-error">
      <p>全局概览加载失败：{{ error }}</p>
      <button type="button" @click="load">重试</button>
    </div>

    <WidgetGrid v-else>
      <!-- ── XL：照片库总览 ── -->
      <UiWidget :span="8" :rows="2" label-cn="照片库总览">
        <template #right><span>{{ year }}</span></template>
        <div class="ov-hero">
          <div class="ov-hero-l">
            <div class="w-stat-row">
              <span class="w-stat-num">{{ totalPhotos.toLocaleString() }}</span>
              <span class="w-stat-unit">张照片</span>
            </div>
            <div class="ov-chips">
              <span class="ov-chip">💾 {{ fmtSize(totalBytes) }}</span>
              <span class="ov-chip" v-if="years">📅 {{ years.min }} – {{ years.max }}</span>
              <span class="ov-chip">📁 {{ totalAlbums }} 个相册</span>
            </div>
            <div class="ov-hero-bar">
              <div class="ov-bar-label">
                <span>已入库</span>
                <b>{{ scanned.toLocaleString() }} / {{ totalPhotos.toLocaleString() }} · {{ (knownRatio * 100).toFixed(1) }}%</b>
              </div>
              <div class="w-pbar lg"><i :style="{ width: `${knownRatio * 100}%` }"></i></div>
            </div>
          </div>
          <div class="ov-hero-r">
            <div class="ov-bar-label"><span>{{ year }} 月度新增</span><b>{{ monthBars.counts[now.getMonth()] }} 张</b></div>
            <BarsCore :values="monthBars.values" :active-indexes="[monthBars.active]" />
          </div>
        </div>
      </UiWidget>

      <!-- ── 语义索引 ── -->
      <UiWidget :span="4" :rows="1" label="语义索引">
        <template #right><button class="ui-widget-link" @click="goCategory">详情</button></template>
        <GaugeCore :ratio="indexedRatio" :size="58" variant="ok">
          <b>{{ catStats?.indexed.toLocaleString() ?? scanned.toLocaleString() }}</b> 张已建索引<br />
          <template v-if="(catStats?.stale ?? 0) > 0">
            <span class="hot">{{ catStats!.stale }} 张</span> 属其他模型档
          </template>
          <template v-else>索引完整</template>
        </GaugeCore>
      </UiWidget>

      <!-- ── 那年今日速览 ── -->
      <UiWidget :span="4" :rows="1" label="那年今日" :to="'/memories'">
        <StatCore
          :value="onThisDay.length"
          unit="组"
          small
          :delta="memoryYears ? `${memoryYears} 个年份` : ''"
          delta-tone="mute"
        />
      </UiWidget>

      <!-- ── 相册 ── -->
      <UiWidget :span="4" :rows="2" label-cn="相册" clickable @click="router.push('/albums')">
        <template #right><b>{{ totalAlbums }}</b>&nbsp;个</template>
        <CoversCore v-if="albumMosaic.length" :items="albumMosaic" variant="mosaic" @pick="() => router.push('/albums')" />
        <div v-else class="ov-empty">还没有相册</div>
        <p class="w-stat-note" style="margin-top:9px">点击进入相册管理</p>
      </UiWidget>

      <!-- ── 人物 ── -->
      <UiWidget :span="4" :rows="2" label-cn="人物" clickable @click="goFace()">
        <template #right><b>{{ persons.length }}</b>&nbsp;位</template>
        <FacesCore v-if="faceItems.length" :items="faceItems" :max="5" @pick="(i: number) => goFace(topPersons[i]?.id)" />
        <div v-else class="ov-empty">尚未识别出人物</div>
        <p class="w-stat-note" style="margin-top:auto">按出现次数排序 · {{ facePhotos.toLocaleString() }} 张照片含人物</p>
      </UiWidget>

      <!-- ── 地点 ── -->
      <UiWidget :span="4" :rows="2" label-cn="地点" clickable @click="goLocation()">
        <template #right><b>{{ namedLoc.length }}</b>&nbsp;城</template>
        <GeoPinsCore v-if="namedLoc.length" :groups="locations" @pick="onGeoPick" />
        <div v-else class="ov-empty">照片里还没有定位信息</div>
        <p class="w-stat-note" style="margin-top:auto">
          GPS 覆盖 <b>{{ (gpsRatio * 100).toFixed(1) }}%</b> · {{ namedLoc.reduce((n, g) => n + g.count, 0).toLocaleString() }} 张有定位
        </p>
      </UiWidget>

      <!-- ── 内容分类 ── -->
      <UiWidget :span="4" :rows="2" label="内容分类" clickable @click="goCategory()">
        <StatCore :value="catCount" unit="个语义分类">
          <template #note>{{ catNames || '扫描语义向量后自动归类' }}</template>
        </StatCore>
        <div class="w-pbar" style="margin-top:12px"><i :style="{ width: `${Math.min(100, indexedRatio * 100)}%` }"></i></div>
        <p class="w-stat-note">{{ catPhotoCount.toLocaleString() }} 张已归类</p>
      </UiWidget>

      <!-- ── 人物照片覆盖 ── -->
      <UiWidget :span="4" :rows="2" label="人物照片覆盖" clickable @click="goFace()">
        <GaugeCore :ratio="faceRatio" :size="76">
          <b>{{ facePhotos.toLocaleString() }}</b> 张照片含人物<br />归并为 <b>{{ persons.length }}</b> 位
        </GaugeCore>
      </UiWidget>

      <!-- ── GPS 覆盖 ── -->
      <UiWidget :span="4" :rows="2" label="地点定位覆盖" clickable @click="goLocation()">
        <GaugeCore :ratio="gpsRatio" :size="76">
          <b>{{ namedLoc.reduce((n, g) => n + g.count, 0).toLocaleString() }}</b> 张带 GPS<br />聚为 <b>{{ namedLoc.length }}</b> 个地点
        </GaugeCore>
        <RankBarsCore v-if="topCities.length" :items="topCities" />
      </UiWidget>

      <!-- ── 最近导入 ── -->
      <UiWidget :span="8" :rows="2" label-cn="最近导入" :to="'/timeline'">
        <template #right><span class="ui-widget-link">时间线 →</span></template>
        <CoversCore
          v-if="recentCovers.length"
          :items="recentCovers"
          variant="strip"
          @pick="(i: number) => goAlbum(recent[i]?.album_id)"
        />
        <div v-else class="ov-empty">还没有已入库的照片</div>
      </UiWidget>

      <!-- ── 那年今日大卡 ── -->
      <UiWidget :span="4" :rows="2" label-cn="那年今日" :to="'/memories'">
        <MemoryCore
          v-if="memoryHero"
          :src="memoryCover"
          :title="`${year - Number(memoryHero.shoot_time!.slice(0, 4))} 年前的今天`"
          :subtitle="`${memoryHero.shoot_time!.slice(0, 4)}${memoryHero.location ? ' · ' + memoryHero.location : ''}`"
        />
        <div v-else class="ov-empty ov-empty-mem">往年今天的照片会出现在这里</div>
      </UiWidget>
    </WidgetGrid>
  </div>
</template>

<style scoped>
.overview {
  padding: 4px var(--sp-8) 48px;
}

/* XL 总览：左右分栏 */
.ov-hero {
  flex: 1;
  display: flex;
  gap: 22px;
  margin-top: 8px;
  min-height: 0;
}
.ov-hero-l {
  width: 46%;
  display: flex;
  flex-direction: column;
}
.ov-hero-r {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  padding-left: 20px;
  /* 结构分隔线：双刻线（亮侧 + 暗侧）。实测单层 11% 白只有 1.24:1，亮底上更只有 1.5:1。 */
  border-left: 1px solid var(--w-divider);
  box-shadow: -1px 0 0 var(--w-divider-dark);
}
.ov-chips {
  display: flex;
  gap: 6px;
  margin-top: 9px;
  flex-wrap: wrap;
}
.ov-chip {
  font-size: 10px;
  color: var(--color-text-2);
  padding: 3px 9px;
  border-radius: 999px;
  border: 1px solid var(--w-line);
  background: rgba(128, 128, 128, 0.08);
}
.ov-hero-bar {
  margin-top: auto;
}
.ov-bar-label {
  display: flex;
  justify-content: space-between;
  font-size: 10px;
  color: var(--color-text-2);
  margin-bottom: 6px;
}
.ov-bar-label b {
  color: var(--color-text);
  font-weight: 600;
}
.hot {
  color: var(--w-accent);
}

/* 空态 / 骨架 / 错误 */
.ov-empty {
  flex: 1;
  display: grid;
  place-items: center;
  font-size: 11px;
  color: var(--color-text-3);
  text-align: center;
}
.ov-empty-mem {
  margin-top: 10px;
  border-radius: var(--w-radius-inner);
  border: 1px dashed var(--w-line);
}
.ov-skel {
  display: grid;
  grid-template-columns: repeat(12, 1fr);
  grid-auto-rows: 86px;
  gap: 14px;
  padding: 4px var(--sp-8);
}
.ov-skel-card {
  grid-column: span 4;
  grid-row: span 2;
  border-radius: var(--w-radius);
  background: linear-gradient(100deg, rgba(128, 128, 128, 0.10) 30%, rgba(128, 128, 128, 0.2) 50%, rgba(128, 128, 128, 0.10) 70%);
  background-size: 200% 100%;
  animation: ov-shimmer 1.3s infinite;
}
.ov-skel-card:nth-child(1),
.ov-skel-card:nth-child(8) {
  grid-column: span 8;
}
@keyframes ov-shimmer {
  from { background-position: 200% 0; }
  to { background-position: -200% 0; }
}
.ov-error {
  padding: 40px var(--sp-8);
  text-align: center;
  color: var(--color-text-2);
  font-size: 12px;
}
.ov-error button {
  margin-top: 10px;
  padding: 6px 16px;
  border-radius: var(--r-ui);
  border: 1px solid var(--w-line);
  background: transparent;
  color: inherit;
  cursor: pointer;
}

@media (max-width: 1000px) {
  .overview {
    padding: 4px var(--sp-5) 40px;
  }
  .ov-hero {
    flex-direction: column;
    gap: 14px;
  }
  .ov-hero-l {
    width: 100%;
  }
  .ov-hero-r {
    padding-left: 0;
    border-left: none;
  }
}
</style>
