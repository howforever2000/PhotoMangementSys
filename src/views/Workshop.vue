<script setup lang="ts">
import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";

/**
 * 创意工坊（FEAT-063）：图片编辑小组件的统一入口页。
 *
 * 设计取向与相册管理 / 智慧相册的 Hub 一致：
 * 顶部标题区 + 小组件卡片网格。后续图片编辑类小组件都挂在这里，
 * 每个组件保持「单一职责 + 打开即用」，不共享复杂状态。
 */
const router = useRouter();
const theme = useThemeStore();

const cardStyle = computed(() => theme.cardStyle);

const badgeStyle = computed(() =>
  theme.isDark
    ? { color: "rgba(255,255,255,.85)", background: "rgba(120,120,130,.4)", border: "1px solid rgba(255,255,255,.18)" }
    : { color: "rgba(50,60,80,.85)", background: "rgba(0,0,0,.06)", border: "1px solid rgba(0,0,0,.08)" },
);

/** 工坊小组件清单 */
const widgets = [
  {
    id: "region-eq",
    title: "区域直方图均衡化",
    desc: "框选或画蒙版圈定范围，只对选中区域做均衡化（全局均衡 / CLAHE），支持羽化与强度调节，亮度通道处理不偏色",
    icon: "🌗",
    ready: true,
  },
  {
    id: "placeholder",
    title: "更多小组件",
    desc: "裁剪旋转 / 局部调色 / 贴纸文字……（规划中，逐步上线）",
    icon: "🧩",
    ready: false,
  },
] as const;

/** 当前打开的小组件 id（null = 仅卡片列表） */
const activeWidget = ref<string | null>(null);

function openWidget(w: (typeof widgets)[number]) {
  if (!w.ready) return;
  activeWidget.value = w.id;
}

function closeWidget() {
  activeWidget.value = null;
}

function goBack() {
  if (activeWidget.value) {
    closeWidget();
    return;
  }
  router.back();
}
</script>

<template>
  <div class="workshop-page">
    <div class="workshop-content">
      <header class="workshop-header">
        <button class="back-btn" type="button" @click="goBack">← 返回</button>
        <div class="header-text">
          <h1 class="page-title">创意工坊</h1>
          <p class="page-subtitle">图片编辑小组件 · 打开即用，处理都在本机完成</p>
        </div>
      </header>

      <!-- 小组件卡片列表 -->
      <main v-if="!activeWidget" class="widget-grid">
        <article
          v-for="w in widgets"
          :key="w.id"
          class="widget-card"
          :class="{ 'widget-ready': w.ready, 'widget-pending': !w.ready }"
          :style="cardStyle"
          @click="openWidget(w)"
        >
          <div class="widget-icon">{{ w.icon }}</div>
          <div class="widget-body">
            <h2 class="widget-title">
              {{ w.title }}
              <span v-if="!w.ready" class="pending-badge" :style="badgeStyle">敬请期待</span>
            </h2>
            <p class="widget-desc">{{ w.desc }}</p>
          </div>
          <div class="widget-arrow">{{ w.ready ? "打开 →" : "🔒" }}</div>
        </article>
      </main>

      <!-- 区域直方图均衡化小组件挂载点（FEAT-063，实际组件随后续提交接入） -->
      <template v-else-if="activeWidget === 'region-eq'">
        <div class="widget-host" :style="cardStyle">组件接入中…</div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.workshop-page {
  position: relative;
  min-height: 100vh;
}

.workshop-content {
  position: relative;
  z-index: 2;
  max-width: 1080px;
  margin: 0 auto;
  padding: 40px 24px 64px;
}

.workshop-header {
  display: flex;
  align-items: center;
  gap: 16px;
  margin-bottom: 36px;
}

.back-btn {
  height: 32px;
  padding: 0 14px;
  font-size: 13px;
  color: var(--color-text);
  background: rgba(120, 120, 130, 0.12);
  border: 1px solid rgba(120, 120, 130, 0.22);
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.2s;
}

.back-btn:hover {
  background: rgba(120, 120, 130, 0.22);
}

.header-text {
  text-align: left;
}

.page-title {
  margin: 0 0 6px;
  font-size: 28px;
  font-weight: 700;
  color: var(--color-text);
}

.page-subtitle {
  margin: 0;
  font-size: 14px;
  color: var(--color-text-2);
}

.widget-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 20px;
}

.widget-card {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 22px 20px;
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
  border-radius: var(--radius-lg, 16px);
  transition: transform 0.2s, box-shadow 0.2s, border-color 0.2s;
}

.widget-ready {
  cursor: pointer;
}

.widget-ready:hover {
  transform: translateY(-4px);
  box-shadow: 0 12px 28px rgba(16, 24, 40, 0.16);
  border-color: rgba(140, 180, 255, 0.45);
}

.widget-pending {
  opacity: 0.6;
  cursor: not-allowed;
}

.widget-icon {
  font-size: 34px;
  flex-shrink: 0;
  filter: drop-shadow(0 2px 6px rgba(0, 0, 0, 0.4));
}

.widget-body {
  flex: 1;
}

.widget-title {
  margin: 0 0 6px;
  font-size: 17px;
  font-weight: 600;
  color: var(--color-text);
}

.widget-desc {
  margin: 0;
  font-size: 13px;
  line-height: 1.5;
  color: var(--color-text-2);
}

.pending-badge {
  display: inline-block;
  margin-left: 8px;
  padding: 1px 8px;
  font-size: 11px;
  font-weight: 600;
  border-radius: 10px;
  vertical-align: middle;
}

.widget-arrow {
  font-size: 14px;
  flex-shrink: 0;
  white-space: nowrap;
  color: var(--color-text-2);
}

.widget-host {
  padding: 28px;
  border-radius: var(--radius-lg, 16px);
  color: var(--color-text-2);
  font-size: 14px;
}

@media (max-width: 640px) {
  .widget-grid {
    grid-template-columns: 1fr;
  }
}
</style>
