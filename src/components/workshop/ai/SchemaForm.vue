<script setup lang="ts">
/**
 * 参数面板由 schema 渲染（FEAT-168）
 *
 * 取代每个工具面板手写 slider / segment：字段、范围、枚举、默认值全部来自
 * `op_catalog.json` 的 `params`，不再出现「控件绑了值、提交时没进 options」这类
 * 死控件（原 AiWorkbench 的 wb.strength 就是这么坏的）。
 *
 * 只做渲染与取值，不做提交、不碰作业状态（编排在 useToolJob / 视图入口层）。
 */
import { watch } from "vue";
import type { ParamSpec } from "../../../utils/toolCatalog";
import { defaultParams } from "../../../utils/toolCatalog";

const props = defineProps<{
  params?: Record<string, ParamSpec>;
  disabled?: boolean;
}>();
const model = defineModel<Record<string, unknown>>({ default: () => ({}) });

function seed() {
  const d = defaultParams({ params: props.params });
  model.value = { ...d, ...model.value };
  // 丢掉 schema 里已删除的字段，避免旧值被提交给后端
  const keys = new Set(Object.keys(d));
  for (const k of Object.keys(model.value)) if (!keys.has(k)) delete model.value[k];
}
watch(() => props.params, seed, { immediate: true, deep: true });
</script>

<template>
  <div v-if="params && Object.keys(params).length" class="schema-form">
    <div v-for="(p, key) in params" :key="key" class="ai-field">
      <label class="ai-label" :for="`sf-${key}`">{{ p.label }}</label>

      <!-- 枚举：沿用既有 .ai-seg 分段控件，不发明新控件 -->
      <div v-if="p.type === 'enum'" class="ai-seg" role="group" :aria-label="p.label">
        <button
          v-for="v in p.values ?? []"
          :key="String(v)"
          type="button"
          class="ai-seg-btn"
          :class="{ on: String(model[key]) === String(v) }"
          :disabled="disabled"
          @click="model[key] = v"
        >
          {{ v }}
        </button>
      </div>

      <!-- 数值：原生 range + 读数，最小值/步长由 schema 给 -->
      <div v-else-if="p.type === 'range'" class="sf-range">
        <input
          :id="`sf-${key}`"
          v-model.number="model[key]"
          type="range"
          :min="p.min"
          :max="p.max"
          :step="p.step ?? 0.05"
          :disabled="disabled"
        />
        <span class="sf-val">{{ model[key] }}</span>
      </div>

      <input
        v-else-if="p.type === 'string'"
        :id="`sf-${key}`"
        v-model="model[key]"
        class="ai-inp ai-inp-full"
        type="text"
        :disabled="disabled"
      />
      <label v-else class="sf-check">
        <input v-model="model[key]" type="checkbox" :disabled="disabled" />
        启用
      </label>
    </div>
  </div>
</template>

<style scoped>
.schema-form {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 12px 18px;
}
@media (max-width: 1080px) {
  .schema-form {
    grid-template-columns: minmax(0, 1fr);
  }
}
.sf-range {
  display: flex;
  align-items: center;
  gap: 10px;
}
.sf-range input[type="range"] {
  flex: 1;
  min-width: 0;
  accent-color: var(--c-accent);
}
.sf-val {
  min-width: 40px;
  text-align: right;
  font-size: var(--fs-meta);
  color: var(--fg);
  font-variant-numeric: tabular-nums;
}
.sf-check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-body);
  color: var(--fg-2);
}
</style>
