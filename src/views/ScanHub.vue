<script setup lang="ts">
/**
 * 图片扫描板块 Hub（FEAT-038）
 *
 * 模仿「智慧相册」SmartAlbum 的 Hub 模式：
 *  - 顶部 Hero：渐变背景 + 关键统计（相册数 / 总照片 / 已扫描入库 / 已入库相册 / 后台任务状态）
 *  - 4 个子模块入口卡：渐变区分，点击切换下方内容
 *    - 「全局照片扫描入库」：默认 tab，直接在 Hub 内渲染 GlobalScanPanel 子组件
 *    - 「扫描测试工具」：跳转子页面 /scan/test（按年·地点组织移动）
 *    - 「按年·地点浏览」：占位（待开发）
 *    - 「重复扫描清理」：占位（待开发）
 *  - 下方区域：渲染当前选中 tab 的子组件（默认 GlobalScanPanel）
 *
 * 全局扫描任务状态在 Pinia store 中独立存活，切换 tab / 离开 Hub 不中断扫描。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import GlobalScanPanel from "../components/GlobalScanPanel.vue";

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
    label: "扫描测试工具",
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
  },
  { immediate: true },
);

onMounted(async () => {
  if (!albumStore.albums.length) {
    try {
      await albumStore.fetchAlbums();
    } catch {
      /* Hero 统计不可用也不阻塞 */
    }
  }
});

/* ---------------- 子模块入口卡（智慧相册风格） ---------------- */
interface SubModule {
  tab: string;
  icon: string;
  title: string;
  desc: string;
  gradient: string;
}
const subModules: SubModule[] = [
  {
    tab: "global",
    icon: "🗂️",
    title: "全局照片扫描入库",
    desc: "勾选相册批量扫描入库，支持全选、启停、进度与后台执行",
    gradient: "linear-gradient(135deg, #396cd8 0%, #5a8bf7 100%)",
  },
  {
    tab: "by-time-place",
    icon: "🗓️",
    title: "按年·地点浏览",
    desc: "把扫描结果按年 / 地点组织聚合展示（待开发）",
    gradient: "linear-gradient(135deg, #43cea2 0%, #185a9d 100%)",
  },
  {
    tab: "dedupe",
    icon: "🧹",
    title: "重复扫描清理",
    desc: "基于哈希找出已入库但重复扫描的照片，支持一键清理（待开发）",
    gradient: "linear-gradient(135deg, #f093fb 0%, #f5576c 100%)",
  },
  {
    tab: "test",
    icon: "🧪",
    title: "扫描测试工具",
    desc: "扫描任意文件夹，按年·地点组织移动（独立子页面）",
    gradient: "linear-gradient(135deg, #ff7e5f 0%, #feb47b 100%)",
  },
];

function openSub(m: SubModule) {
  const def = tabs.find((t) => t.key === m.tab);
  if (!def || !def.enabled) return;
  if (def.path) {
    router.push(def.path);
    return;
  }
  activeTab.value = m.tab;
}
</script>

<template>
  <div class="scan-page" :style="{ color: theme.textColor }">
    <button class="btn scan-back" @click="router.push('/home')">← 返回主页</button>

    <!-- Hero：渐变背景 + 标题 + 关键统计 + 任务状态 -->
    <section
      class="scan-hero"
      :style="{ background: 'linear-gradient(135deg, #396cd8 0%, #5a8bf7 50%, #7eb6ff 100%)' }"
    >
      <div class="scan-hero-mask"></div>
      <div class="scan-hero-content">
        <div class="scan-hero-eyebrow">SCAN HUB</div>
        <h1 class="scan-hero-title">🔍 图片扫描</h1>
        <p class="scan-hero-sub">
          聚合照片扫描相关子功能：批量扫描入库、按年·地点浏览、重复清理与扫描测试
        </p>
        <div class="scan-stats">
          <div class="stat">
            <span class="stat-num">{{ totalAlbums }}</span>
            <span class="stat-label">个相册</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat">
            <span class="stat-num">{{ totalPhotos }}</span>
            <span class="stat-label">张照片</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat">
            <span class="stat-num">{{ scannedAlbums }}</span>
            <span class="stat-label">个已入库相册</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat">
            <span class="stat-num">{{ scannedIn }}</span>
            <span class="stat-label">已入库照片</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat scan-task-stat" :class="{ running: globalRunning }">
            <span class="stat-num">
              <template v-if="globalRunning">⏳ 后台运行中</template>
              <template v-else-if="globalDoneCount || globalFailedCount">
                {{ globalDoneCount }} / {{ globalDoneCount + globalFailedCount }} 完成
              </template>
              <template v-else>—</template>
            </span>
            <span class="stat-label">全局扫描任务</span>
          </div>
        </div>
      </div>
    </section>

    <!-- 4 个子模块入口卡（强调：点击切换下方内容） -->
    <section class="scan-subgrid">
      <button
        v-for="m in subModules"
        :key="m.tab"
        class="scan-subcard"
        :class="{ 'scan-subcard-active': activeTab === m.tab, 'scan-subcard-disabled': !tabs.find((t) => t.key === m.tab)?.enabled }"
        :style="{ background: m.gradient }"
        @click="openSub(m)"
      >
        <span class="scan-subicon">{{ m.icon }}</span>
        <span class="scan-subbody">
          <span class="scan-subtitle-line">{{ m.title }}</span>
          <span class="scan-subdesc">{{ m.desc }}</span>
        </span>
        <span class="scan-subarrow">{{ tabs.find((t) => t.key === m.tab)?.enabled ? "→" : "🔒" }}</span>
      </button>
    </section>

    <!-- 分类方式标签栏（与 SmartAlbum 风格一致） -->
    <nav class="tab-bar" role="tablist">
      <button
        v-for="t in tabs"
        :key="t.key"
        class="tab-item"
        :class="{ active: activeTab === t.key, disabled: !t.enabled }"
        role="tab"
        :aria-selected="activeTab === t.key"
        @click="t.enabled && (activeTab = t.key)"
      >
        <span class="tab-icon">{{ t.icon }}</span>
        {{ t.label }}
        <span v-if="!t.enabled" class="tab-pending">待开发</span>
      </button>
    </nav>

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
.scan-page {
  max-width: 1100px;
  margin: 0 auto;
  padding: 24px 20px 48px;
}
.scan-back { margin-bottom: 14px; }

/* ---- Hero ---- */
.scan-hero {
  position: relative;
  height: 220px;
  border-radius: 20px;
  overflow: hidden;
  margin-bottom: 22px;
  color: #fff;
  box-shadow: 0 10px 30px rgba(57, 108, 216, 0.25);
}
.scan-hero-mask {
  position: absolute;
  inset: 0;
  background: radial-gradient(circle at 80% 20%, rgba(255, 255, 255, 0.25), transparent 60%);
  pointer-events: none;
}
.scan-hero-content {
  position: relative;
  height: 100%;
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 0 32px;
}
.scan-hero-eyebrow {
  font-size: 12px;
  letter-spacing: 3px;
  opacity: 0.85;
  margin-bottom: 6px;
}
.scan-hero-title {
  font-size: 30px;
  margin: 0 0 6px;
  font-weight: 800;
  letter-spacing: 2px;
  text-shadow: 0 2px 12px rgba(0, 0, 0, 0.2);
}
.scan-hero-sub {
  margin: 0 0 16px;
  font-size: 13.5px;
  opacity: 0.92;
}
.scan-stats {
  display: flex;
  align-items: center;
  gap: 14px;
  column-gap: 20px;
  flex-wrap: wrap;
}
.stat {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 52px;
}
.stat-num {
  font-size: 20px;
  font-weight: 700;
  text-shadow: 0 1px 4px rgba(0, 0, 0, 0.2);
  white-space: nowrap;
}
.stat-label {
  font-size: 11.5px;
  opacity: 0.9;
  white-space: nowrap;
}
.stat-divider {
  width: 1px;
  height: 26px;
  background: rgba(255, 255, 255, 0.35);
}
.scan-task-stat.running .stat-num {
  animation: scan-task-pulse 1.6s ease-in-out infinite;
}
@keyframes scan-task-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.7; }
}

/* ---- 子模块卡（带渐变背景） ---- */
.scan-subgrid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 12px;
  margin: 4px 0 22px;
}
.scan-subcard {
  display: flex;
  align-items: center;
  gap: 12px;
  text-align: left;
  padding: 14px 16px;
  border-radius: 14px;
  cursor: pointer;
  border: none;
  color: #fff;
  transition: transform 0.18s ease, box-shadow 0.18s ease;
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
}
.scan-subcard:hover:not(.scan-subcard-disabled) {
  transform: translateY(-3px);
  box-shadow: 0 12px 26px rgba(0, 0, 0, 0.28);
}
.scan-subcard-active {
  outline: 3px solid #fff;
  outline-offset: 1px;
  transform: translateY(-3px);
}
.scan-subcard-disabled {
  opacity: 0.55;
  cursor: not-allowed;
}
.scan-subicon {
  font-size: 26px;
  flex-shrink: 0;
  filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.25));
}
.scan-subbody {
  flex: 1;
  min-width: 0;
}
.scan-subtitle-line {
  font-size: 15px;
  font-weight: 700;
  display: block;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}
.scan-subdesc {
  font-size: 12px;
  opacity: 0.92;
  display: block;
  margin-top: 2px;
}
.scan-subarrow {
  font-size: 16px;
  opacity: 0.75;
  flex-shrink: 0;
}

/* ---- 分类 tab ---- */
.tab-bar {
  display: flex;
  gap: 8px;
  margin-bottom: 18px;
  border-bottom: 1px solid currentColor;
  border-image: linear-gradient(90deg, rgba(128, 138, 158, 0.45), transparent) 1;
  padding-bottom: 10px;
  flex-wrap: wrap;
}
.tab-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 1px solid transparent;
  background: transparent;
  box-shadow: inset 0 0 0 1px rgba(128, 138, 158, 0.4);
  border-radius: 999px;
  padding: 6px 16px;
  font-size: 13px;
  cursor: pointer;
  color: inherit;
  opacity: 0.85;
  transition: all 0.15s;
}
.tab-item:hover:not(.disabled) { border-color: #396cd8; color: #396cd8; opacity: 1; }
.tab-item.active {
  background: #396cd8;
  border-color: #396cd8;
  box-shadow: none;
  color: #fff;
  opacity: 1;
}
.tab-item.disabled { opacity: 0.45; cursor: not-allowed; }
.tab-icon { font-size: 14px; }
.tab-pending {
  font-size: 10px;
  background: rgba(138, 146, 163, 0.2);
  border-radius: 999px;
  padding: 1px 6px;
}
.tab-item.active .tab-pending { background: rgba(255, 255, 255, 0.25); color: #fff; }

/* ---- 下方内容区 ---- */
.scan-active-area { margin-top: 4px; }

.scan-placeholder,
.scan-jump-placeholder {
  text-align: center;
  padding: 60px 20px;
  opacity: 0.7;
  font-size: 14px;
}
.scan-jump-placeholder .btn { margin-top: 14px; }

@media (max-width: 640px) {
  .scan-page { padding: 16px 12px 32px; }
  .scan-hero { height: auto; padding: 18px 0; }
  .scan-hero-content { padding: 0 18px; }
  .scan-hero-title { font-size: 24px; }
  .scan-subcard { padding: 12px 14px; }
}
</style>
