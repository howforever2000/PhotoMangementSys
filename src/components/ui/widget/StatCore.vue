<script setup lang="ts">
/**
 * StatCore —— 大读数芯（FEAT-110）
 * 原子职责：一个读数 + 单位 + 可选 Δ + 可选脚注 + 可选底部进度条。
 * 不自己取数、不发请求；所有值由父级传入，保证跨页面可复用。
 */
withDefaults(
  defineProps<{
    value: string | number;
    unit?: string;
    /** 小一号读数（放入 r1 薄板时用） */
    small?: boolean;
    /** 右侧变化值，如 “+12” */
    delta?: string;
    deltaTone?: "up" | "mute";
    /** 脚注（支持纯文本；需高亮请用 #note 插槽） */
    note?: string;
    /** 底部进度条 0~1；不传不画 */
    progress?: number | null;
    /** 进度条加粗 */
    progressLg?: boolean;
  }>(),
  {
    unit: "",
    small: false,
    delta: "",
    deltaTone: "up",
    note: "",
    progress: null,
    progressLg: false,
  },
);
</script>

<template>
  <div class="w-body-core">
    <div class="w-stat-row">
      <span class="w-stat-num" :class="{ sm: small }">{{ value }}</span>
      <span v-if="unit" class="w-stat-unit">{{ unit }}</span>
      <span v-if="delta" class="w-stat-delta" :class="deltaTone">{{ delta }}</span>
    </div>
    <p v-if="note || $slots.note" class="w-stat-note">
      <slot name="note">{{ note }}</slot>
    </p>
    <slot />
    <div v-if="progress != null" class="w-pbar" :class="{ lg: progressLg }">
      <i :style="{ width: `${Math.min(100, Math.max(0, progress * 100))}%` }"></i>
    </div>
  </div>
</template>

<style scoped>
.w-body-core {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
</style>
