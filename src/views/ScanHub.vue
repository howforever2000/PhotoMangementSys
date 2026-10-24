<script setup lang="ts">
/**
 * 扫描入库模块 Hub（FEAT-038，FEAT-105 重做）
 *
 * FEAT-105 改了什么：旧版顶部是「彩虹渐变 Hero + 4 张渐变入口卡 + 胶囊 tab」，
 * 同一组功能被写了两遍（卡一遍、tab 一遍），而且渐变吃掉了页面的注意力。
 * 现在：模块页头（你在哪） + 数据带（有多少） + ModuleTabs（看哪块） + 内容。
 *
 * 子功能（全在本模块内，不再有跨模块导航）：
 *   · 全局扫描入库：默认 tab，直接在 Hub 内渲染 GlobalScanPanel
 *   · 按年·地点浏览：待开发（置灰可见）
 *   · 重复扫描清理：待开发（置灰可见）
 *   · 相册扫描分组工具：独立子页 /scan/test
 *
 * 全局扫描任务状态在 Pinia store 中独立存活，切换 tab / 离开 Hub 不中断扫描。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import GlobalScanPanel from "../components/GlobalScanPanel.vue";
import AppHeader, { type HeaderTab } from "../components/ui/AppHeader.vue";
import StatBand, { type StatItem } from "../components/ui/StatBand.vue";

const router = useRouter();
const theme = useThemeStore();
const albumStore = useAlbumStore();
const contentStore = useContentStore();

interface ScanTab {
  key: string;
  label: string;
  icon: string;
  /** true = 在 Hub 内直接渲染组件；false = 跳转子页面（path） */
  inline: boolean;
  /** 跳转路径（inline=false 时必填） */
  path?: string;
  enabled: boolean;
}

const tabs: ScanTab[] = [
  {
    key: "global",
    label: "全局照片扫描入库",
    icon: "🗂️",
    inline: true,
    enabled: true,
  },
  {
    key: "by-time-place",
    label: "按年·地点浏览",
    icon: "🗓️",
    inline: false,
    enabled: false,
  },
  {
    key: "dedupe",
    label: "重复扫描清理",
    icon: "🧹",
    inline: false,
    enabled: false,
  },
  {
    key: "test",
    label: "相册扫描分组工具",
    icon: "🧪",
    inline: false,
    path: "/scan/test",
    enabled: true,
  },
];

const activeTab = ref<string>("global");
const activeDef = computed(() => tabs.find((t) => t.key === activeTab.value));

/* ---------------- Hero 统计 ---------------- */
const totalAlbums = ref(0);
const totalPhotos = ref(0);
const scannedAlbums = ref(0);
const scannedIn = ref(0);
/** 全局扫描任务状态（用于 Hero 副标题与卡片 badge） */
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
    // BUG-2026-0917-002：此前 scannedIn 声明后从未赋值，Hero 第四格「已入库照片」永远显示 0，
    // 与智慧相册页「653 已扫描入库照片」自相矛盾。这里按相册累加已入库照片数。
    scannedIn.value = list.reduce((n, a) => n + (a.scanned_photo_count || 0), 0);
  },
  { immediate: true },
);

/* ---------------- 数据带 / 二级导航 / 页头读数（FEAT-105） ---------------- */

/** 数据带：把旧 hero 里的五格搬到带里。
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

/** 组件内部分区：四项全属于「扫描入库」 */
const moduleTabs = computed<HeaderTab[]>(() =>
  tabs.map((t) => ({
    key: t.key,
    label: t.label.replace("全局照片扫描入库", "全局扫描"),
    to: t.inline ? undefined : t.path,
    disabled: !t.enabled,
    why: t.enabled ? undefined : "功能待开发：扫描写库后即可用",
  })),
);

/** 页头读数：一行说清这页在干什么 */
const headerMeta = computed(() => {
  const parts: string[] = [];
  if (scannedAlbums.value) parts.push(`${scannedAlbums.value} 个相册已入库`);
  if (globalRunning.value) parts.push("后台扫描中");
  return parts.join(" · ");
});

onMounted(async () => {
  if (!albumStore.albums.length) {
    try {
      await albumStore.fetchAlbums();
    } catch {
      /* Hero 统计不可用也不阻塞 */
    }
  }
});
</script>

<template>
  <div class="scan-page" :style="{ color: theme.textColor }">
    <!-- 大组件页头：组件名 + 本组件内部分区（全局扫描 / 按年·地点 / 重复清理 / 分组工具） -->
    <AppHeader
      title="扫描入库"
      icon="📥"
      :meta="headerMeta"
      :tabs="moduleTabs"
      :active="activeTab"
      @pick="(k) => (activeTab = k)"
    />

    <!-- 数据带：相册 / 照片 / 已入库 / 任务状态 -->
    <StatBand :items="statItems" class="scan-stats" />

    <!-- 当前 tab 对应的子内容区（FEAT-038：默认 tab 即在 Hub 内渲染 GlobalScanPanel） -->
    <section class="scan-active-area">
      <div v-if="activeTab === 'global'">
        <GlobalScanPanel />
      </div>
      <div v-else-if="activeDef?.path" class="scan-jump-placeholder">
        <p>该子功能以独立页面形式提供，点击下方按钮进入：</p>
        <button class="btn btn-primary" @click="activeDef.path && router.push(activeDef.path)">
          打开「{{ activeDef.label }}」→
        </button>
      </div>
      <div v-else class="scan-placeholder">
        「{{ activeDef?.label }}」即将上线 —— 完成对应扫描后即可在此浏览与操作。
      </div>
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

/* 数据带：与顶栏保持 16px（板底缘与内容之间要留空气，否则读不出板的厚度） */
.scan-stats { margin: 16px 0 18px; }

/* 内容区左右留白与页头/数据带对齐（--sp-8 = 52px） */
.scan-active-area { padding: 0 var(--sp-8); }

.scan-placeholder,
.scan-jump-placeholder {
  text-align: center;
  padding: 60px 20px;
  color: var(--c-fg-2);
  font-size: var(--fs-body);
}
.scan-jump-placeholder .btn { margin-top: 14px; }

/* 窄窗：页头收窄时内容区同步跟上 */
@media (max-width: 900px) {
  .scan-active-area { padding: 0 var(--sp-5); }
}
</style>
