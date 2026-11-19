<script setup lang="ts">
/**
 * 配方选择器（①一键出片 与 ②工作台 共用同一个组件，FEAT-168 / FEAT-197）
 *
 * 一行紧凑横向条：不做同尺寸卡片网格（craft-floor 拒绝项），主区视觉主体
 * 仍是照片本身。选中项只用 accent 表达当前状态，不给装饰性彩色。
 *
 * 配方列表直接内联同一份 recipes.json：与 Python 执行器共享唯一真相源，
 * 不新增端点、不在 Python 服务没起来时空白。
 */
import { computed } from "vue";
import recipesJson from "../../../../python/ai_studio/recipes.json" with { type: "json" };

interface RecipeLite {
  id: string;
  label: string;
  desc: string;
  steps: unknown[];
}

const props = defineProps<{
  modelValue: string;
  disabled?: boolean;
}>();
const emit = defineEmits<{ "update:modelValue": [value: string] }>();

const recipes = computed<RecipeLite[]>(
  () => (recipesJson as { recipes: RecipeLite[] }).recipes,
);
const current = computed(() => recipes.value.find((r) => r.id === props.modelValue) ?? recipes.value[0]);

function pick(id: string) {
  if (!props.disabled) emit("update:modelValue", id);
}
</script>

<template>
  <section class="recipe-picker" aria-label="出片配方">
    <div class="rp-list" role="radiogroup" aria-label="配方">
      <button
        v-for="r in recipes"
        :key="r.id"
        type="button"
        class="rp-item"
        :class="{ on: current?.id === r.id }"
        role="radio"
        :aria-checked="current?.id === r.id"
        :disabled="disabled"
        :title="r.desc"
        @click="pick(r.id)"
      >
        <span class="rp-name">{{ r.label }}</span>
        <span class="rp-count">{{ r.steps.length }} 步</span>
      </button>
    </div>
    <p class="rp-desc">{{ current?.desc }}</p>
  </section>
</template>

<style scoped>
.recipe-picker {
  margin: 0 0 14px;
}
.rp-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
/* 复用既有 .ai-seg 视觉语汇，但保持为一行紧凑条，不做卡片网格 */
.rp-item {
  display: inline-flex;
  align-items: baseline;
  gap: 8px;
  height: 32px;
  padding: 0 14px;
  border: none;
  border-radius: var(--r-pill);
  background: none;
  box-shadow: inset 0 0 0 1px var(--c-rule);
  color: var(--fg-2);
  font-size: var(--fs-body);
  cursor: pointer;
  transition:
    color var(--dur-2) var(--ease-out),
    background var(--dur-2) var(--ease-out),
    transform var(--dur-1) var(--ease-out);
}
.rp-item:hover:not(:disabled) {
  color: var(--fg);
  background: var(--hover);
}
.rp-item:active:not(:disabled) {
  transform: scale(0.985);
}
.rp-item:focus-visible {
  outline: 1.5px solid var(--c-accent);
  outline-offset: 2px;
}
.rp-item.on {
  color: var(--fg);
  background: color-mix(in srgb, var(--c-accent) 16%, transparent);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 55%, transparent);
  font-weight: var(--fw-em);
}
.rp-item:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.rp-name {
  font-weight: inherit;
}
.rp-count {
  font-size: var(--fs-label);
  color: var(--fg-2);
  font-variant-numeric: tabular-nums;
}
.rp-desc {
  margin: 8px 0 0;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.5;
}
</style>
