<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useRoute, useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";
import { useAlbumStore } from "../stores/album";
import PersonGallery from "../components/PersonGallery.vue";
import CategoryGallery from "../components/CategoryGallery.vue";
import LocationGallery from "../components/LocationGallery.vue";
import type { ContentSearchHit } from "../types/content";
import type { PersonInfo } from "../types/photo";
import AppHeader, { type HeaderTab } from "../components/ui/AppHeader.vue";
import StatBand, { type StatItem } from "../components/ui/StatBand.vue";

/**
 * 智慧相册 —— 模块页（FEAT-105 重做）
 *
 * 与上一版的区别（为什么重写）：
 *   旧版是「彩虹渐变 Hero + 4 张渐变入口卡 + 胶囊 tab」三件套。三个问题：
 *     ① 渐变 hero 抢走了全部注意力，而这页真正的内容是下面的人脸墙；
 *     ② 4 张渐变卡把"人物/回忆/时间线/搜索"提到了与模块同高的层级 ——
 *        它们本来就是这个模块内部的小功能，应该活在二级 tab 里；
 *     ③ 顶部还有一条全站导航，等于一页里出现两套导航，层级彻底塌了。
 *   现在：一条模块页头（你在哪） + 一条数据带（有多少） + 一条二级 tab（看哪块）
 *        + 内容。小功能回到 tab 里，页头承担"这是智慧相册"。
 *
 * 默认 tab 是**内容分类**（FEAT-106）：
 *   用户进入智慧相册时最想看到的是"按内容找照片"（美food / 风景 / 夜景…），
 *   而不是一堆还没命名的人脸聚类（P001 / P094）。人脸墙要逐个改名才可用，
 *   放在入口第一个位置等于把"未完成的事"当主入口。
 *   所以顺序改为：内容分类 → 人物 → 地点 → 回忆 → 时间线 → 智能搜索。
 *   人物仍可从 tab 或回忆页「近期人物」直达（深链 /smart?tab=face&person={pid}）。
 */
const route = useRoute();
const router = useRouter();
const theme = useThemeStore();
const store = useAlbumStore();

/** FEAT-046：深链定位的人物 id（/smart?tab=face&person={pid}） */
const focusPid = ref<string | null>(null);

/** 状态型 tab：只有这三个在本页内切换。
 *  顺序即展示顺序 —— 内容分类在第一个（它是本模块的主入口）。 */
const tabs = [
  { key: "category", label: "内容分类", enabled: true },
  { key: "face", label: "人物", enabled: true },
  { key: "location", label: "地点", enabled: true },
] as const;

const activeTab = ref<string>("category");
const activeDef = computed(() => tabs.find((t) => t.key === activeTab.value));

/* -------- 统计：总照片 / 占存储 / 相册 / 已入库 / 已扫描 / 人物 -------- */
const totalPhotos = ref(0);
const totalBytes = ref(0);
const scannedIn = ref(0);
const totalPersons = ref(0);
const totalAlbums = ref(0);
const scannedAlbums = ref(0);

/** 字节数人性化显示（B/KB/MB/GB） */
function fmtSize(bytes: number): string {
  if (!bytes) return "0 B";
  if (bytes >= 1024 * 1024 * 1024) return (bytes / 1024 / 1024 / 1024).toFixed(1) + " GB";
  if (bytes >= 1024 * 1024) return (bytes / 1024 / 1024).toFixed(1) + " MB";
  if (bytes >= 1024) return (bytes / 1024).toFixed(1) + " KB";
  return bytes + " B";
}

async function loadStats() {
  try {
    if (!store.albums.length) await store.fetchAlbums();
    // 已扫描入库数（跨相册）与人物数
    const [tl, pl] = await Promise.all([
      invoke<ContentSearchHit[]>("list_timeline"),
      invoke<PersonInfo[]>("list_persons"),
    ]);
    scannedIn.value = tl.length;
    totalPersons.value = pl.length;
  } catch {
    /* 统计不可用也不阻塞入口 */
  }
}

/** 总照片 / 占存储 / 相册数 / 已入库相册数：从相册列表聚合（fill_album_stats 已填充） */
watch(
  () => store.albums,
  (list) => {
    totalPhotos.value = list.reduce((n, a) => n + (a.photo_count || 0), 0);
    totalBytes.value = list.reduce((n, a) => n + (a.size_bytes || 0), 0);
    totalAlbums.value = list.length;
    scannedAlbums.value = list.filter((a) => (a.scanned_photo_count || 0) > 0).length;
  },
  { immediate: true },
);

/**
 * 数据带：本模块的"有多少"。
 * 照片/存储/相册是全局量（本模块要用），人物与已入库是模块自己的量。
 * 不加 delta/sparkline —— 这些数没有时间序列，编一个趋势比不画更糟。
 */
const statItems = computed<StatItem[]>(() => [
  { label: "张照片", value: totalPhotos.value, unit: "张" },
  { label: "占用存储", value: fmtSize(totalBytes.value) },
  {
    label: "相册",
    value: totalAlbums.value,
    unit: "个",
    // 入库进度 = 已扫描照片 / 总照片（与下一格的“已入库”同一项目径）
    progress: totalPhotos.value ? scannedIn.value / totalPhotos.value : 0,
    progressTone: "muted",
  },
  {
    label: "已入库",
    value: scannedIn.value,
    unit: "张",
    progress: totalPhotos.value ? scannedIn.value / totalPhotos.value : 0,
  },
  { label: "人物", value: totalPersons.value, unit: "位" },
]);

/** 组件内部分区导航：全部属于「智慧相册」这个大组件。
 *  顺序与 tabs 一致：内容分类在第一个。 */
const moduleTabs = computed<HeaderTab[]>(() => [
  { key: "category", label: "内容分类" },
  { key: "face", label: "人物", count: totalPersons.value || undefined },
  { key: "location", label: "地点" },
  { label: "回忆", to: "/memories" },
  { label: "时间线", to: "/timeline" },
  { label: "智能搜索", to: "/search" },
]);

/** 页头读数：一行说清这页装了什么，替代已被删掉的 hero 副标题 */
const headerMeta = computed(() => {
  const parts: string[] = [];
  if (totalPersons.value) parts.push(`${totalPersons.value} 位人物`);
  if (totalAlbums.value) parts.push(`${totalAlbums.value} 个相册`);
  return parts.join(" · ");
});

onMounted(() => {
  loadStats();
  // FEAT-046：读 query 深链 —— tab 切换 + 人物定位（回忆页近期人物跳转入口）
  const tab = String(route.query.tab ?? "");
  if (tab && tabs.some((t) => t.key === tab && t.enabled)) {
    activeTab.value = tab;
  } else {
    // 没带 tab（或带了非法值）→ 落到默认的内容分类上，
    // 并把 URL 补齐：这样页头那一排 tab 能正确高亮"当前在哪一块"，
    // 刷新/分享链接也能回到同一块（而不是又跌回默认）。
    syncTabQuery(activeTab.value);
  }
  const person = String(route.query.person ?? "").trim();
  if (person) focusPid.value = person;
});

/** 切换本页分区：状态 + URL 一起改（URL 是真源，刷新后不丢） */
function pickTab(key: string) {
  activeTab.value = key;
  syncTabQuery(key);
}

function syncTabQuery(key: string) {
  const cur = String(route.query.tab ?? "");
  if (cur === key) return;
  // replace 而不是 push：切 tab 不该往历史里塞十几条记录，
  // 否则从智慧相册按返回键要按十几次才回得到 Home。
  router.replace({ path: route.path, query: { ...route.query, tab: key } }).catch(() => {});
}
</script>

<template>
  <div class="smart-page" :style="{ color: theme.textColor }">
    <!-- 大组件页头：组件名 + 本组件内部分区（内容分类/人物/地点/回忆/时间线/搜索） -->
    <AppHeader
      title="智慧相册"
      icon="🧠"
      :meta="headerMeta"
      :tabs="moduleTabs"
      :active="activeTab"
      @pick="pickTab"
    />

    <!-- 数据带：统计信息保留，但不做成彩虹横幅 -->
    <StatBand :items="statItems" class="smart-stats" />

    <!-- 默认内容分类：按内容找照片（美food / 风景 / 夜景…）是这页的主入口。
         人物墙放在第二个 —— 未命名的人脸聚类需要逐个改名才可用，不适合当首屏。 -->
    <CategoryGallery v-if="activeTab === 'category'" />
    <PersonGallery v-else-if="activeTab === 'face'" :focus-pid="focusPid" />
    <LocationGallery v-else-if="activeTab === 'location'" />
    <div v-else class="smart-placeholder">
      「{{ activeDef?.label }}」分类展示即将上线 —— 完成对应扫描后即可在此浏览。
    </div>
  </div>
</template>

<style scoped>
/* 页面壳：只负责左右留白与纵向节奏。
   宽度不设 max-width —— 人脸墙要的是列数，锁 1100px 会让 1440 屏浪费 340px。 */
.smart-page {
  min-height: 100vh;
  padding-bottom: 48px;
}

/* 数据带（FEAT-107）：自带 padding 与下缘发丝线，与页头留 12px。
   数据带和 tab 是"同一条信息轴"（先看在哪，再看有多少，再看哪块）。 */
.smart-stats {
  margin: 12px 0 16px;
}

/* 内容区左右留白与页头/数据带对齐（--sp-8 = 52px）。
   三个画廊组件的根元素本身没有 padding（pg-wrap 只有 min-width:0），
   所以不能给根元素加 padding —— 那会把内容往内推两次。
   只统一它们内部的工具条与网格。 */
.smart-page > :deep(.pg-wrap > *),
.smart-page > :deep(.cg-wrap > *),
.smart-page > :deep(.lg-wrap > *) {
  padding-left: var(--sp-8);
  padding-right: var(--sp-8);
}

.smart-placeholder {
  padding: 60px var(--sp-8);
  font-size: var(--fs-body);
  color: var(--c-fg-2);
}

/* 窄窗：页头与数据带同步收窄，内容区跟上（否则内容比页头更靠里） */
@media (max-width: 900px) {
  .smart-page > :deep(.pg-wrap > *),
  .smart-page > :deep(.cg-wrap > *),
  .smart-page > :deep(.lg-wrap > *) {
    padding-left: var(--sp-5);
    padding-right: var(--sp-5);
  }
}
</style>
