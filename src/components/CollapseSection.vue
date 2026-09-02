<script setup lang="ts">
import { ref } from "vue";

/**
 * 可折叠区块（通用组件）。
 * 相册详情页的「照片搜索 / 组合扫描 / 缩略图浏览」共用同一套折叠形式：
 * 统一的头部（箭头 + 标题 + 副标题），点击头部展开/收起。
 * - 内容用 v-show 切换：收起不销毁，扫描进度、缩略图缓存都保留
 * - 传入 storageKey 时折叠状态持久化到 localStorage，下次进入保持一致
 */
const props = withDefaults(
  defineProps<{
    /** 折叠头标题 */
    title: string;
    /** 头部右侧的补充说明（可选） */
    subtitle?: string;
    /** 折叠状态持久化 key（可选） */
    storageKey?: string;
    /** 无持久化记录时的默认状态（默认折叠） */
    defaultOpen?: boolean;
  }>(),
  { subtitle: "", defaultOpen: false },
);

function initialOpen(): boolean {
  if (!props.storageKey) return props.defaultOpen;
  try {
    const saved = localStorage.getItem(`pm-collapse:${props.storageKey}`);
    if (saved !== null) return saved === "1";
  } catch {
    /* 忽略 */
  }
  return props.defaultOpen;
}

const open = ref(initialOpen());

function toggle() {
  open.value = !open.value;
  if (props.storageKey) {
    try {
      localStorage.setItem(`pm-collapse:${props.storageKey}`, open.value ? "1" : "0");
    } catch {
      /* 忽略 */
    }
  }
}
</script>

<template>
  <section class="collapse-section">
    <header class="cs-head" @click="toggle">
      <svg viewBox="0 0 16 16" class="cs-chevron" :class="{ open }" aria-hidden="true">
        <path
          d="M3 6l5 5 5-5"
          stroke="currentColor"
          stroke-width="2"
          fill="none"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      </svg>
      <h3 class="cs-title">{{ title }}</h3>
      <span v-if="subtitle" class="cs-subtitle">{{ subtitle }}</span>
      <span class="cs-action-hint">{{ open ? "点击折叠" : "点击展开" }}</span>
    </header>
    <div v-show="open" class="cs-body">
      <slot />
    </div>
  </section>
</template>

<style scoped>
.collapse-section {
  background: var(--color-surface, #fff);
  border: 1px solid var(--color-border, #e2e6ee);
  border-radius: 14px;
  box-shadow: 0 4px 18px rgba(31, 51, 102, 0.06);
  overflow: hidden;
  color: var(--color-text);
}

.collapse-section + .collapse-section {
  margin-top: 16px;
}

.cs-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 14px 18px;
  cursor: pointer;
  user-select: none;
  transition: background 0.15s;
}

.cs-head:hover {
  background: var(--color-primary-soft, #f6f8fc);
}

.cs-chevron {
  width: 16px;
  height: 16px;
  color: var(--color-text-2, #6a7690);
  flex-shrink: 0;
  transition: transform 0.2s ease;
}

.cs-chevron.open {
  transform: rotate(180deg);
}

.cs-title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
  color: var(--color-text);
}

.cs-subtitle {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  color: var(--color-text-2);
}

.cs-action-hint {
  margin-left: auto;
  flex-shrink: 0;
  font-size: 12px;
  color: var(--color-text-2);
}

.cs-subtitle + .cs-action-hint,
.cs-title + .cs-action-hint:not(:last-child) {
  margin-left: 12px;
}

.cs-body {
  padding: 4px 18px 18px;
}
</style>
