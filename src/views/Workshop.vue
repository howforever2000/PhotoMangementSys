<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { invoke } from "@tauri-apps/api/core";
import { useThemeStore } from "../stores/theme";

/**
 * 创意工坊（FEAT-063）：图片编辑小组件的统一入口页。
 *
 * 设计取向与相册管理 / 智慧相册的 Hub 一致：
 * 顶部标题区 + 小组件卡片网格。后续图片编辑类小组件都挂在这里，
 * 每个组件保持「单一职责 + 打开即用」，不共享复杂状态。
 */
const router = useRouter();
const route = useRoute();
const theme = useThemeStore();

/**
 * FEAT-065：从灯箱「✏️ 编辑」跳进来时带 ?photo=<原图绝对路径>，
 * 直接打开传统图像处理小组件（默认算子：区域直方图均衡化）并载入该图 —— 打开即用。
 * 读一次即可（工坊页不会在停留期间改变 query）。
 */
const initialPhoto = (() => {
  const p = route.query.photo;
  return typeof p === "string" ? p : "";
})();

const cardStyle = computed(() => theme.cardStyle);

/** 「待开发」徽标：落在玻璃卡上，跟随玻璃等效底色取字色（旧 isDark 二分在浅色卡上融底） */
const badgeStyle = computed(() => ({
  color: theme.subTextColor,
  background: "rgba(120,120,130,.4)",
  border: `1px solid ${theme.cardBorder}`,
}));

/**
 * 工坊小组件清单。
 *
 * FEAT-099 融合后的形态：**一个编辑器页面**，右侧并列两个组件 ——
 * 「基准调色」（Rust 进程内管线）与「传统图像处理」（Python 微服务算子）。
 *
 * 两者不是二选一，而是**叠加**：调色出底图，算子按顺序作用在底图上，
 * 每一步都记进配方（`recipe.ops`），因此重开、导出都能复现同一串处理。
 *
 * 后端仍是分离的：调色要逐帧实时预览（微服务往返做不到），算子参数少、
 * 不追求逐帧（OpenCV 的边界处理不必重写）。
 */
const widgets = [
  {
    id: "studio",
    title: "照片编辑",
    desc: "基准调色（曝光 / 对比度 / 高光阴影 / 色温色调 / 曲线 / 裁剪拉直 / 饱和度 / 锐化）与图像处理算子（区域直方图均衡化 / 高斯·方框·中值·双边模糊）叠加使用。每一步都记录在方案里，随照片存为 sidecar，可撤销或删掉某一步",
    icon: "🎚️",
    ready: true,
  },
  {
    id: "placeholder",
    title: "更多小组件",
    desc: "贴纸文字 / 批量处理……（规划中，逐步上线）",
    icon: "🧩",
    ready: false,
  },
] as const;

/** 当前打开的小组件 id（null = 仅卡片列表；带照片进来时直接进编辑器） */
const activeWidget = ref<string | null>(initialPhoto ? "studio" : null);

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

/**
 * 预热本机处理服务（FEAT-066 体验优化）：算子注册表本身只要 3ms，慢的只有
 * Python 服务冷启动（python + cv2 约 1~2s）。进工坊页即后台 ensure 一次，
 * 用户看完卡片再点开小组件时服务通常已就绪；失败静默忽略（点开组件时会重新 ensure 并给出错误态）。
 * 并发安全由 Rust 侧 ensure 单飞锁保证，这里不需要防抖。
 */
onMounted(() => {
  invoke("studio_ensure").catch(() => {});
});
</script>

<template>
  <div class="workshop-page" :class="{ 'wide-mode': activeWidget === 'studio' }">
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
          class="widget-card glass-card"
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

      <!-- 照片编辑器：调色 + 算子叠加同处一屏，共享一个图片会话 -->
      <PhotoEditor v-if="activeWidget === 'studio'" :initial-path="initialPhoto" />
    </div>
  </div>
</template>

<script lang="ts">
// 工坊小组件按需引入（放在独立 script 块，保持 setup 块聚焦布局逻辑）
import PhotoEditor from "../components/workshop/editor/PhotoEditor.vue";
export default { components: { PhotoEditor } };
</script>

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

/* FEAT-099 基准调色：编辑器是「直方图 + 画布 + 面板」三栏，
   卡片列表那种 1080px 窄容器会把画布压到不足 400px。
   打开编辑器时让容器铺满，只保留左右最小边距。 */
.workshop-page.wide-mode .workshop-content {
  max-width: none;
  padding: 20px 16px 20px;
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
  /* 页面背景上的标题：用 on-bg 对比色（BUG-2026-0919-004） */
  color: var(--color-on-bg, var(--color-text));
}

.page-subtitle {
  margin: 0;
  font-size: 14px;
  color: var(--color-on-bg-2, var(--color-text-2));
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
