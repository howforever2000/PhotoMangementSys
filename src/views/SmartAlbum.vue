<script setup lang="ts">
import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";
import PersonGallery from "../components/PersonGallery.vue";

/**
 * 智慧相册 —— 分类方式导航壳（组件化）
 *
 * 人脸/人物识别只是第一种分类方式；后续新分类（内容分类 / 地点 / 影调…）
 * 只需在 `tabs` 中注册一个条目并挂载对应的画廊组件即可，无需改壳逻辑。
 */
const router = useRouter();
const theme = useThemeStore();

interface SmartTab {
  key: string;
  label: string;
  icon: string;
  /** 是否已实现（false = 预留占位，显示「待开发」徽章） */
  enabled: boolean;
}

/** 分类方式注册表（预留扩展位） */
const tabs: SmartTab[] = [
  { key: "face", label: "人物", icon: "👥", enabled: true },
  { key: "category", label: "内容分类", icon: "🏞️", enabled: false },
  { key: "location", label: "地点", icon: "📍", enabled: false },
];

const activeTab = ref<string>("face");
const activeDef = computed(() => tabs.find((t) => t.key === activeTab.value));
</script>

<template>
  <div class="smart-page" :style="{ color: theme.textColor }">
    <header class="smart-header">
      <button class="btn smart-back" @click="router.push('/home')">← 返回主页</button>
      <h1 class="smart-title">🧠 智慧相册</h1>
      <p class="smart-subtitle">
        {{ activeDef?.enabled
          ? "同一对象自动归类，支持命名与合并（人物按出现次数排序）"
          : "该分类方式开发中，敬请期待" }}
      </p>
    </header>

    <!-- 分类方式标签栏（预留其他分类显示方法） -->
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

    <!-- 分类内容：当前仅「人物」启用；新分类在此挂载对应画廊组件 -->
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
.smart-header { margin-bottom: 18px; }
.smart-back { margin-bottom: 14px; }

.smart-title {
  font-size: 26px;
  font-weight: 700;
  margin: 0 0 6px;
}

.smart-subtitle {
  color: inherit;
  opacity: 0.72;
  font-size: 13px;
  margin: 0;
}

/* 分类方式标签栏 */
.tab-bar {
  display: flex;
  gap: 8px;
  margin-bottom: 20px;
  border-bottom: 1px solid currentColor;
  border-image: linear-gradient(90deg, rgba(128,138,158,.45), transparent) 1;
  padding-bottom: 10px;
}

.tab-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 1px solid transparent;
  background: transparent;
  box-shadow: inset 0 0 0 1px rgba(128,138,158,.4);
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
</style>
