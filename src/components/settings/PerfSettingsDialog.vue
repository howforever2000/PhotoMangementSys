<script setup lang="ts">
/**
 * 「⚙ 性能设置 / 📦 模型管理」弹窗（两个扫描入口共用同一份）
 *
 * 为什么做成一个共享组件：相册扫描（ScanPanel）与全局扫描（GlobalScanPanel）
 * 原来只有后者有设置入口，前者只有一个裸「检测 GPU」按钮 —— 用户在相册里
 * 改不了加速/线程。现在两边都渲染本组件，能力与外观完全一致。
 *
 * 两个页签把「与硬件有关的性能开关」和「与网络有关的模型下载」彻底分开：
 *   - ⚙ 性能设置：只放这台机器怎么跑得更快的开关（依赖识别服务）
 *   - 📦 模型管理：下载/镜像源（**不依赖**识别服务，服务挂了也能下）
 *
 * 交互：Esc 关闭 · 点遮罩关闭（点内容不关闭）· 页签有焦点态 · 关闭按钮可键盘聚焦。
 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useThemeStore } from "../../stores/theme";
import { useContentStore } from "../../stores/content";
import ModelGpuSettings from "../ModelGpuSettings.vue";
import ModelManageSection from "./ModelManageSection.vue";

const open = defineModel<boolean>({ required: true });
const theme = useThemeStore();
const store = useContentStore();

type Tab = "perf" | "model";
const tab = ref<Tab>("perf");

/** 有必需模型未下载 → 在「模型管理」页签上给个可视提示，避免用户不知道要下 */
const dlPending = computed(() =>
  store.modelDownloads.some((d) => d.required && !d.done),
);

function onKey(e: KeyboardEvent) {
  if (e.key !== "Escape" || !open.value) return;
  // 同时阻止默认与传播：否则 App.vue 的全局 ESC 会在关弹窗后**再**路由返回上一级
  // （实现要点：必须挂 document 冒泡阶段，与 ConfirmDialog 同一口径 ——
  //  document 的回调先于 window 执行，全局 onGlobalEsc 才看得到 defaultPrevented）
  e.preventDefault();
  e.stopPropagation();
  open.value = false;
}
onMounted(() => {
  document.addEventListener("keydown", onKey);
  // 下载状态只在本弹窗内关心，挂载时拉一次即可
  void store.listModelDownloads();
});
onUnmounted(() => document.removeEventListener("keydown", onKey));

// 切到模型管理页签时补一次最新进度（进度由 model-dl-progress 事件持续更新）
watch(open, (v) => {
  if (v) void store.listModelDownloads();
});
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="psd-mask" role="presentation" @click.self="open = false">
      <div
        class="psd-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="性能设置与模型管理"
        :style="theme.cardStyle"
      >
        <div class="psd-head">
          <nav class="psd-tabs" aria-label="设置分区">
            <button
              class="psd-tab"
              :class="{ on: tab === 'perf' }"
              :aria-current="tab === 'perf' ? 'page' : undefined"
              @click="tab = 'perf'"
            >
              ⚙ 性能设置
            </button>
            <button
              class="psd-tab"
              :class="{ on: tab === 'model' }"
              :aria-current="tab === 'model' ? 'page' : undefined"
              @click="tab = 'model'"
            >
              📦 模型管理
              <span v-if="dlPending" class="psd-dot" title="有必需模型未下载"></span>
            </button>
          </nav>
          <button class="psd-close" aria-label="关闭设置" @click="open = false">✕</button>
        </div>

        <div class="psd-body">
          <ModelGpuSettings v-if="tab === 'perf'" />
          <ModelManageSection v-else />
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.psd-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(16, 24, 40, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}
.psd-dialog {
  width: min(680px, 96vw);
  max-height: min(86vh, 860px);
  display: flex;
  flex-direction: column;
  border-radius: var(--radius-lg, 14px);
  box-shadow: 0 20px 60px rgba(16, 24, 40, 0.28);
  overflow: hidden;
}
.psd-head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 16px;
  border-bottom: 1px solid var(--color-border);
  flex-shrink: 0;
}
.psd-tabs {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.psd-tab {
  position: relative;
  min-height: 34px;
  padding: 7px 14px;
  border: 1px solid transparent;
  border-radius: var(--radius-sm, 8px);
  background: transparent;
  color: var(--color-text-2);
  font-size: 13.5px;
  font-weight: 600;
  cursor: pointer;
  transition: background 0.15s, color 0.15s, border-color 0.15s;
}
.psd-tab:hover {
  background: var(--color-soft-accent);
  color: var(--color-text);
}
.psd-tab:focus-visible {
  outline: 2px solid rgba(57, 108, 216, 0.55);
  outline-offset: 2px;
}
.psd-tab.on {
  background: var(--color-primary-soft);
  border-color: #b9cdf5;
  color: var(--color-primary);
}
.psd-dot {
  display: inline-block;
  width: 7px;
  height: 7px;
  margin-left: 6px;
  border-radius: 50%;
  background: var(--color-warn);
  vertical-align: middle;
}
.psd-close {
  margin-left: auto;
  width: 34px;
  height: 34px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
  color: var(--color-text-2);
  font-size: 15px;
  cursor: pointer;
  flex-shrink: 0;
}
.psd-close:hover {
  border-color: var(--color-danger);
  color: var(--color-danger);
}
.psd-close:focus-visible {
  outline: 2px solid rgba(57, 108, 216, 0.55);
  outline-offset: 2px;
}
.psd-body {
  padding: 16px;
  overflow-y: auto;
}
</style>
