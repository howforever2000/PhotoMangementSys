<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";
import { useAlbumStore } from "../stores/album";
import PersonGallery from "../components/PersonGallery.vue";
import type { ContentSearchHit } from "../types/content";
import type { PersonInfo } from "../types/photo";

/**
 * 智慧相册 Hub —— 顶部 Hero（渐变 + 关键统计） + 4 大子模块入口卡
 *
 * - 「人物」：仍是核心 tab（人脸识别聚合）— 选中后下方渲染 PersonGallery
 * - 「回忆」：跳转到 /memories（百度网盘智能相册风格故事页）
 * - 「时间线」：跨相册时间线浏览
 * - 「智能搜索」：半自然语言 + 多维筛选
 * - 其他分类（内容/地点）仍是「待开发」占位
 */
const router = useRouter();
const theme = useThemeStore();
const store = useAlbumStore();

interface SmartTab {
  key: string;
  label: string;
  icon: string;
  enabled: boolean;
}

const tabs: SmartTab[] = [
  { key: "face", label: "人物", icon: "👥", enabled: true },
  { key: "category", label: "内容分类", icon: "🏞️", enabled: false },
  { key: "location", label: "地点", icon: "📍", enabled: false },
];

const activeTab = ref<string>("face");
const activeDef = computed(() => tabs.find((t) => t.key === activeTab.value));

/* -------- Hero 统计：总照片 / 占存储 / 已扫描入库 / 总人物 -------- */
const totalPhotos = ref(0);
const totalBytes = ref(0);
const scannedIn = ref(0);
const totalPersons = ref(0);

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
    // 相册统计（photo_count / size_bytes 来自 get_albums 的 fill_album_stats）
    if (!store.albums.length) await store.fetchAlbums();
    // 已扫描入库数（photo_content_scan 中的照片，跨相册）
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

/** 总照片数 / 占存储：从相册列表聚合（fill_album_stats 已填充 photo_count / size_bytes） */
// 在 albums 变化时刷新，保证 async 拉取后更新
watch(
  () => store.albums,
  (list) => {
    totalPhotos.value = list.reduce((n, a) => n + (a.photo_count || 0), 0);
    totalBytes.value = list.reduce((n, a) => n + (a.size_bytes || 0), 0);
  },
  { immediate: true },
);

onMounted(() => {
  loadStats();
});

interface SubModule {
  icon: string;
  title: string;
  desc: string;
  path?: string; // 跳转路径（与 actionTab 二选一）
  tab?: string; // 选中某 tab
  /** 强调色（渐变） */
  gradient: string;
}
const subModules: SubModule[] = [
  {
    icon: "👥",
    title: "人物",
    desc: "按出现频率自动聚类，支持命名、合并、查看照片",
    tab: "face",
    gradient: "linear-gradient(135deg, #6a8df0 0%, #a764ec 100%)",
  },
  {
    icon: "🌟",
    title: "回忆",
    desc: "智能相册故事：按月 / 按年聚合的精彩瞬间",
    path: "/memories",
    gradient: "linear-gradient(135deg, #ff7e5f 0%, #feb47b 100%)",
  },
  {
    icon: "📅",
    title: "时间线",
    desc: "跨相册按拍摄时间聚合浏览",
    path: "/timeline",
    gradient: "linear-gradient(135deg, #43cea2 0%, #185a9d 100%)",
  },
  {
    icon: "🔎",
    title: "智能搜索",
    desc: "自然语言 + 多维筛选检索照片",
    path: "/search",
    gradient: "linear-gradient(135deg, #f093fb 0%, #f5576c 100%)",
  },
];

function openSub(m: SubModule) {
  if (m.path) router.push(m.path);
  else if (m.tab) activeTab.value = m.tab;
}
</script>

<template>
  <div class="smart-page" :style="{ color: theme.textColor }">
    <button class="btn smart-back" @click="router.push('/home')">← 返回主页</button>

    <!-- Hero：渐变背景 + 标题 + 关键统计 -->
    <section class="smart-hero" :style="{ background: 'linear-gradient(135deg, #6a8df0 0%, #a764ec 50%, #f093fb 100%)' }">
      <div class="smart-hero-mask"></div>
      <div class="smart-hero-content">
        <div class="smart-hero-eyebrow">SMART ALBUM</div>
        <h1 class="smart-hero-title">🧠 智慧相册</h1>
        <p class="smart-hero-sub">人物 · 回忆 · 时间线 · 智能搜索，让你的照片库自己讲述过去</p>
        <div class="smart-stats">
          <div class="stat">
            <span class="stat-num">{{ totalPhotos }}</span>
            <span class="stat-label">张照片</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat">
            <span class="stat-num stat-num-size">{{ fmtSize(totalBytes) }}</span>
            <span class="stat-label">占用存储</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat">
            <span class="stat-num">{{ scannedIn }}</span>
            <span class="stat-label">已扫描入库</span>
          </div>
          <div class="stat-divider"></div>
          <div class="stat">
            <span class="stat-num">{{ totalPersons }}</span>
            <span class="stat-label">位人物</span>
          </div>
        </div>
      </div>
    </section>

    <!-- 4 个子模块入口卡（强调：人物 tab 切换、回忆/时间线/搜索跳转） -->
    <section class="smart-subgrid">
      <button
        v-for="m in subModules"
        :key="m.title"
        class="smart-subcard"
        :style="{ background: m.gradient }"
        @click="openSub(m)"
      >
        <span class="smart-subicon">{{ m.icon }}</span>
        <span class="smart-subbody">
          <span class="smart-subtitle-line">{{ m.title }}</span>
          <span class="smart-subdesc">{{ m.desc }}</span>
        </span>
        <span class="smart-subarrow">→</span>
      </button>
    </section>

    <!-- 分类方式标签栏（仅控制下方画廊，Hub 入口不依赖它） -->
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

    <PersonGallery v-if="activeTab === 'face'" />
    <div v-else class="smart-placeholder">
      「{{ activeDef?.label }}」分类展示即将上线 —— 完成对应扫描后即可在此浏览。
    </div>
  </div>
</template>

<style scoped>
.smart-page {
  max-width: 1100px;
  margin: 0 auto;
  padding: 24px 20px 48px;
}
.smart-back { margin-bottom: 14px; }

/* ---- Hero ---- */
.smart-hero {
  position: relative;
  height: 200px;
  border-radius: 20px;
  overflow: hidden;
  margin-bottom: 22px;
  color: #fff;
  box-shadow: 0 10px 30px rgba(106, 141, 240, 0.25);
}
.smart-hero-mask {
  position: absolute;
  inset: 0;
  background: radial-gradient(circle at 80% 20%, rgba(255, 255, 255, 0.25), transparent 60%);
  pointer-events: none;
}
.smart-hero-content {
  position: relative;
  height: 100%;
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 0 32px;
}
.smart-hero-eyebrow {
  font-size: 12px;
  letter-spacing: 3px;
  opacity: 0.85;
  margin-bottom: 6px;
}
.smart-hero-title {
  font-size: 30px;
  margin: 0 0 6px;
  font-weight: 800;
  letter-spacing: 2px;
  text-shadow: 0 2px 12px rgba(0, 0, 0, 0.2);
}
.smart-hero-sub {
  margin: 0 0 16px;
  font-size: 13.5px;
  opacity: 0.92;
}
.smart-stats {
  display: flex;
  align-items: center;
  gap: 16px;
  flex-wrap: wrap;
}
.stat {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 60px;
}
.stat-num {
  font-size: 20px;
  font-weight: 700;
  text-shadow: 0 1px 4px rgba(0, 0, 0, 0.2);
  white-space: nowrap;
}
/* 存储字节数（如 12.3 GB）较长，字号略小避免挤压 */
.stat-num-size {
  font-size: 18px;
}
.stat-label {
  font-size: 12px;
  opacity: 0.9;
}
.stat-divider {
  width: 1px;
  height: 26px;
  background: rgba(255, 255, 255, 0.35);
}

/* ---- 子模块卡（带渐变背景） ---- */
.smart-subgrid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 12px;
  margin: 4px 0 22px;
}
.smart-subcard {
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
.smart-subcard:hover {
  transform: translateY(-3px);
  box-shadow: 0 12px 26px rgba(0, 0, 0, 0.28);
}
.smart-subicon {
  font-size: 26px;
  flex-shrink: 0;
  filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.25));
}
.smart-subbody {
  flex: 1;
  min-width: 0;
}
.smart-subtitle-line {
  font-size: 15px;
  font-weight: 700;
  display: block;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
}
.smart-subdesc {
  font-size: 12px;
  opacity: 0.92;
  display: block;
  margin-top: 2px;
}
.smart-subarrow {
  font-size: 16px;
  opacity: 0.75;
  flex-shrink: 0;
}

/* ---- 分类 tab ---- */
.tab-bar {
  display: flex;
  gap: 8px;
  margin-bottom: 20px;
  border-bottom: 1px solid currentColor;
  border-image: linear-gradient(90deg, rgba(128, 138, 158, 0.45), transparent) 1;
  padding-bottom: 10px;
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

.smart-placeholder {
  text-align: center;
  padding: 60px 20px;
  opacity: 0.65;
  font-size: 14px;
}

@media (max-width: 640px) {
  .smart-page { padding: 16px 12px 32px; }
  .smart-hero { height: 180px; }
  .smart-hero-content { padding: 0 18px; }
  .smart-hero-title { font-size: 24px; }
  .smart-subcard { padding: 12px 14px; }
}
</style>
