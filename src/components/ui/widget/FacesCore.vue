<script setup lang="ts">
/**
 * FacesCore —— 头像簇芯（FEAT-110）
 * 原子职责：重叠圆排列。前 N 个真实头像，余下折叠为 “+n”。
 * items：{ src?, text? }（text 用于无头像时显示姓名末字/占位 emoji）。
 */
withDefaults(
  defineProps<{
    items: { src?: string; text?: string }[];
    /** 直接展示的头像数；超出的折叠为一个 +n 圆 */
    max?: number;
    /** 折叠圆里的文字（默认按超出数量自动生成） */
    overflowText?: string;
  }>(),
  { max: 5, overflowText: "" },
);
const emit = defineEmits<{ (e: "pick", index: number): void }>();
</script>

<template>
  <div class="w-faces">
    <template v-for="(p, idx) in items.slice(0, max)" :key="idx">
      <button type="button" class="w-face" @click="emit('pick', idx)">
        <img v-if="p.src" :src="p.src" loading="lazy" alt="" />
        <span v-else>{{ p.text || "?" }}</span>
      </button>
    </template>
    <div v-if="items.length > max" class="w-face w-face-more">
      <span>{{ overflowText || `+${items.length - max}` }}</span>
    </div>
  </div>
</template>

<style scoped>
.w-faces {
  flex: 1;
  display: flex;
  align-items: center;
  min-height: 0;
  margin-top: 8px;
}
.w-face {
  width: 46px;
  height: 46px;
  border-radius: 50%;
  border: 2px solid var(--color-surface-2);
  margin-left: -12px;
  padding: 0;
  overflow: hidden;
  background: rgba(128, 128, 128, 0.25);
  color: var(--color-text-2);
  box-shadow: 0 3px 8px rgba(0, 0, 0, 0.35);
  cursor: pointer;
}
.w-face:first-child {
  margin-left: 0;
}
.w-face img,
.w-face span {
  width: 100%;
  height: 100%;
  display: grid;
  place-items: center;
  font-size: 15px;
  object-fit: cover;
}
.w-face-more {
  display: grid;
  place-items: center;
  font-size: 13px;
  cursor: default;
}
</style>
