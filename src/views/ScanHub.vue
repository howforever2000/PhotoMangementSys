<script setup lang="ts">
/**
 * 扫描入库模块 Hub（FEAT-038，FEAT-105 重做，FEAT-151 统一分区来源）
 *
 * 页内结构：模块页头（你在哪 + 分区导航） + 数据带（有多少） + 当前分区内容。
 *
 * 分区（真值在 `utils/scanModuleTabs.ts`，本页与分组工具子页共用同一份）：
 *   · 全局扫描：默认分区，直接渲染 GlobalScanPanel
 *   · 重复清理：FEAT-143，直接渲染 DedupePanel
 *   · 入库体检：FEAT-152，直接渲染 AlbumHealthPanel（替换掉未开发的「按年·地点浏览」）
 *   · 扫描分组工具：独立子页 /scan/test —— 页头按**路由型 tab** 直接跳转，
 *     不再需要"先点 tab、再点一个打开按钮"的中转占位
 *
 * 全局扫描任务状态在 Pinia store 中独立存活，切换分区 / 离开 Hub 不中断扫描。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import GlobalScanPanel from "../components/GlobalScanPanel.vue";
import DedupePanel from "../components/DedupePanel.vue";
import AlbumHealthPanel from "../components/AlbumHealthPanel.vue";
import AppHeader from "../components/ui/AppHeader.vue";
import StatBand, { type StatItem } from "../components/ui/StatBand.vue";
import {
  DEFAULT_SCAN_SECTION,
  SCAN_SECTIONS,
  scanModuleTabs,
  type ScanSectionKey,
} from "../utils/scanModuleTabs";

const route = useRoute();
const router = useRouter();
const albumStore = useAlbumStore();
const contentStore = useContentStore();

/** 页头分区：状态型（本页内切换）+ 路由型（/scan/test）由 AppHeader 自动分流 */
const moduleTabs = scanModuleTabs();

const activeTab = ref<ScanSectionKey>(DEFAULT_SCAN_SECTION);

/** 是否是一个合法的页内分区 key */
function isSectionKey(k: string): k is ScanSectionKey {
  return SCAN_SECTIONS.some((s) => s.key === k);
}

/**
 * 切换页内分区：状态 + URL 一起改（URL 是真源，刷新 / 分享链接不丢）
 *
 * 与智慧相册同一套口径（FEAT-112 的双向同步）：用 `replace` 而不是 `push`，
 * 否则在几个分区之间来回切会把历史塞满，按返回键要按好几次才回得到 Home。
 */
function pickTab(key: string) {
  if (!isSectionKey(key)) return;
  activeTab.value = key;
  syncTabQuery(key);
}

function syncTabQuery(key: ScanSectionKey) {
  if (String(route.query.tab ?? "") === key) return;
  router.replace({ path: route.path, query: { ...route.query, tab: key } }).catch(() => {});
}

/**
 * 响应 URL query 变化（不只 onMounted）
 *
 * 相册扫描分组工具子页的页头显示同一排分区：点「入库体检」时它是
 * `router.push({ path: "/scan", query: { tab: "health" } })` —— 若我们本来就在 /scan，
 * vue-router 不会重新挂载组件，只靠 onMounted 读一次就会出现「点了没反应」。
 */
watch(
  () => route.query.tab,
  (tab) => {
    const key = String(tab ?? "");
    if (!isSectionKey(key)) return;
    if (key !== activeTab.value) activeTab.value = key;
  },
);

/* ---------------- 数据带读数 ---------------- */
const totalAlbums = ref(0);
const totalPhotos = ref(0);
const scannedAlbums = ref(0);
const scannedIn = ref(0);
/** 全局扫描任务状态（用于数据带与页头读数） */
const globalRunning = computed(() => contentStore.globalScanJob.running);
const globalDoneCount = computed(
  () => contentStore.globalScanJob.items.filter((i) => i.status === "done").length,
);
const globalFailedCount = computed(
  () => contentStore.globalScanJob.items.filter((i) => i.status === "failed").length,
);

watch(
  () => albumStore.albums,
  (list) => {
    totalPhotos.value = list.reduce((n, a) => n + (a.photo_count || 0), 0);
    totalAlbums.value = list.length;
    scannedAlbums.value = list.filter((a) => (a.scanned_photo_count || 0) > 0).length;
    // BUG-2026-0917-002：此前 scannedIn 声明后从未赋值，第四格「已入库照片」永远显示 0，
    // 与智慧相册页「653 已扫描入库照片」自相矛盾。这里按相册累加已入库照片数。
    scannedIn.value = list.reduce((n, a) => n + (a.scanned_photo_count || 0), 0);
  },
  { immediate: true },
);

/** 数据带：旧 hero 里的五格搬过来。
 *  任务状态单占一格：它是这页唯一"会变"的值，放在带尾不会打乱前面四个静态读数。 */
const statItems = computed<StatItem[]>(() => {
  const jobValue = globalRunning.value
    ? "运行中"
    : globalDoneCount.value || globalFailedCount.value
      ? `${globalDoneCount.value}/${globalDoneCount.value + globalFailedCount.value}`
      : "—";
  return [
    { label: "相册", value: totalAlbums.value, unit: "个" },
    { label: "照片", value: totalPhotos.value, unit: "张" },
    {
      label: "已入库相册",
      value: scannedAlbums.value,
      unit: "个",
      progress: totalAlbums.value ? scannedAlbums.value / totalAlbums.value : 0,
      progressTone: "muted",
    },
    {
      label: "已入库照片",
      value: scannedIn.value,
      unit: "张",
      progress: totalPhotos.value ? scannedIn.value / totalPhotos.value : 0,
    },
    {
      label: "全局扫描任务",
      value: jobValue,
      tone: globalRunning.value ? "up" : "mute",
      note: globalFailedCount.value ? `${globalFailedCount.value} 个失败` : undefined,
    },
  ];
});

/** 页头读数：一行说清这页在干什么 */
const headerMeta = computed(() => {
  const parts: string[] = [];
  if (scannedAlbums.value) parts.push(`${scannedAlbums.value} 个相册已入库`);
  if (globalRunning.value) parts.push("后台扫描中");
  return parts.join(" · ");
});

onMounted(async () => {
  // 读 query 深链（子页跳回来 / 刷新 / 分享链接）；没带或非法值就把 URL 补齐，
  // 这样页头那排分区能正确高亮「当前在哪一块」。
  const q = String(route.query.tab ?? "");
  if (isSectionKey(q)) activeTab.value = q;
  else syncTabQuery(activeTab.value);

  if (!albumStore.albums.length) {
    try {
      await albumStore.fetchAlbums();
    } catch {
      /* 数据带统计不可用也不阻塞 */
    }
  }
});
</script>

<template>
  <div class="scan-page" :style="{ color: 'var(--fg)' }">
    <!-- 大组件页头：组件名 + 本组件内部分区（全局扫描 / 重复清理 / 入库体检 / 扫描分组工具） -->
    <AppHeader
      title="扫描入库"
      icon="📥"
      :meta="headerMeta"
      :tabs="moduleTabs"
      :active="activeTab"
      @pick="pickTab"
    />

    <!-- 数据带：相册 / 照片 / 已入库 / 任务状态 -->
    <StatBand :items="statItems" class="scan-stats" />

    <!-- 当前分区内容：三项都在 Hub 内直接渲染（分组工具走路由，不占这里） -->
    <section class="scan-active-area">
      <GlobalScanPanel v-if="activeTab === 'global'" />
      <DedupePanel v-else-if="activeTab === 'dedupe'" />
      <AlbumHealthPanel v-else-if="activeTab === 'health'" />
    </section>
  </div>
</template>

<style scoped>
/* ---- 页面壳（FEAT-105）----
   只负责左右留白与纵向节奏；宽度不锁 max-width，
   扫描队列表与相册双列在宽屏下应该铺开而不是居中缩窄。 */
.scan-page {
  min-height: 100vh;
  padding-bottom: 48px;
}

/* 数据带（FEAT-107）自带 padding + 下缘发丝线，外层只控纵向节奏。 */
.scan-stats { margin: 0 0 16px; }

/* 内容区左右留白与页头/数据带对齐（--sp-8 = 52px） */
.scan-active-area { padding: 0 var(--sp-8); }

/* 窄窗：页头收窄时内容区同步跟上 */
@media (max-width: 900px) {
  .scan-active-area { padding: 0 var(--sp-5); }
}
</style>
