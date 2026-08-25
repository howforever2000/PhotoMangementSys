<script setup lang="ts">
import { computed, ref } from "vue";
import type { Toast } from "../stores/toast";
import { useToastStore } from "../stores/toast";
import { useThemeStore } from "../stores/theme";

const props = defineProps<{ toast: Toast }>();
const toastStore = useToastStore();
const theme = useThemeStore();

const icons: Record<string, string> = {
  success: "✓",
  warning: "⚠",
  error: "✕",
  info: "ℹ",
};

const typeColor = computed(() => {
  switch (props.toast.type) {
    case "success":
      return "#2f9e44";
    case "warning":
      return "#e8a03c";
    case "error":
      return "#e5484d";
    case "info":
      return "#396cd8";
    default:
      return "#396cd8";
  }
});

/** 主题自适应：弹层背景、文字、边框、阴影 */
const panelStyle = computed(() => {
  const dark = theme.isDark;
  return {
    background: dark ? "rgba(30,34,46,.96)" : "rgba(255,255,255,.98)",
    border: `1px solid ${dark ? "rgba(255,255,255,.12)" : "rgba(0,0,0,.08)"}`,
    boxShadow: dark ? "0 18px 48px rgba(0,0,0,.5)" : "0 18px 48px rgba(16,24,40,.18)",
    color: dark ? "#f5f7ff" : "#1f2733",
  };
});
const subStyle = computed(() => ({
  color: theme.isDark ? "rgba(214,221,240,.72)" : "rgba(60,70,90,.75)",
}));

function close() {
  toastStore.remove(props.toast.id);
}

/** hover 时暂停计时与进度动画，离开后恢复（不要让用户错过关键信息） */
const progressPaused = ref(false);
function onEnter() {
  if (!props.toast.duration || props.toast.duration <= 0) return;
  progressPaused.value = true;
  toastStore.pauseDismiss(props.toast.id);
}
function onLeave() {
  if (!props.toast.duration || props.toast.duration <= 0) return;
  progressPaused.value = false;
  toastStore.resumeDismiss(props.toast.id);
}
</script>

<template>
  <div
    class="toast"
    :style="panelStyle"
    role="status"
    :aria-live="toast.type === 'error' ? 'assertive' : 'polite'"
    @mouseenter="onEnter"
    @mouseleave="onLeave"
  >
    <span class="toast-icon" :style="{ color: typeColor }">{{ icons[toast.type] || "ℹ" }}</span>
    <div class="toast-body">
      <div class="toast-title">{{ toast.title }}</div>
      <div v-if="toast.message" class="toast-message" :style="subStyle">{{ toast.message }}</div>
      <div v-if="toast.actions && toast.actions.length" class="toast-actions">
        <button
          v-for="(a, i) in toast.actions"
          :key="i"
          class="toast-btn"
          :class="`toast-btn-${a.style || 'primary'}`"
          @click="a.onClick(); close()"
        >
          {{ a.label }}
        </button>
      </div>
    </div>
    <button class="toast-close" title="关闭" @click="close">×</button>
    <div
      v-if="toast.duration && toast.duration > 0"
      class="toast-progress"
      :class="{ paused: progressPaused }"
      :style="{ animationDuration: toast.duration + 'ms' }"
    ></div>
  </div>
</template>

<style scoped>
.toast {
  position: relative;
  display: flex;
  align-items: flex-start;
  gap: 10px;
  width: min(360px, calc(100vw - 32px));
  padding: 12px 14px 12px 12px;
  border-radius: 12px;
  overflow: hidden;
  transition: opacity 0.2s ease, transform 0.2s ease;
}
.toast-icon {
  flex-shrink: 0;
  font-size: 16px;
  line-height: 1.3;
  font-weight: 700;
  width: 20px;
  text-align: center;
}
.toast-body {
  flex: 1;
  min-width: 0;
}
.toast-title {
  font-size: 14px;
  font-weight: 600;
  line-height: 1.4;
}
.toast-message {
  font-size: 12.5px;
  line-height: 1.5;
  margin-top: 2px;
  white-space: pre-wrap;
  word-break: break-word;
}
.toast-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 8px;
}
.toast-btn {
  padding: 5px 12px;
  font-size: 12.5px;
  border-radius: 7px;
  cursor: pointer;
  border: 1px solid transparent;
  transition: opacity 0.15s;
}
.toast-btn:hover {
  opacity: 0.85;
}
.toast-btn-secondary {
  background: rgba(120, 120, 130, 0.14);
  color: inherit;
}
.toast-btn-primary {
  background: #396cd8;
  color: #fff;
}
.toast-btn-danger {
  background: #e5484d;
  color: #fff;
}
.toast-close {
  flex-shrink: 0;
  margin-left: 2px;
  font-size: 18px;
  line-height: 1;
  color: inherit;
  opacity: 0.55;
  cursor: pointer;
  padding: 0 2px;
}
.toast-close:hover {
  opacity: 1;
}
.toast-progress {
  position: absolute;
  left: 0;
  bottom: 0;
  height: 3px;
  width: 100%;
  background: currentColor;
  transform-origin: left;
  opacity: 0.25;
  animation-name: toast-shrink;
  animation-timing-function: linear;
  animation-fill-mode: forwards;
}
.toast-progress.paused {
  animation-play-state: paused;
}
@keyframes toast-shrink {
  from {
    transform: scaleX(1);
  }
  to {
    transform: scaleX(0);
  }
}
</style>
