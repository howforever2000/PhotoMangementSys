<script setup lang="ts">
/**
 * 高级区：目标提示词 / 执行参数预览（默认折叠）。
 *
 * 按引擎类型切换展示（docs/v5.md §17 第 4 条）：
 * - **remote 引擎**：显示渲染出的目标提示词（只读灰）；
 * - **local 引擎**：**不显示提示词**，显示执行参数（色温/高光/饱和/强度）
 *   并说明"本预设使用本地算法，不涉及模型提示词"；
 * - **不消费提示词**的预设（人像/卡片/压缩）：显示参数（强度档/版式/页数）。
 *
 * 这条规则不是审美问题：给 local 预设显示一个提示词框，用户会以为改它有用。
 */
import { computed, ref } from "vue";
import type { AgentSnapshot } from "../../../types/agent";
import type { Recipe } from "../../../types/recipe";

const props = defineProps<{ job: AgentSnapshot | null; recipe: Recipe | null }>();

const open = ref(false);

const preview = computed(() => props.job?.preview ?? null);
const params = computed(() => Object.entries(preview.value?.params ?? {}));
const whatSteps = computed(() => preview.value?.steps ?? []);
const isLocal = computed(() => (preview.value?.engine ?? props.recipe?.runSpec.style?.engine) === "local");
const hasPrompt = computed(() => !!props.job?.prompt);

const sourceLabel = computed(() => {
  const s = props.job?.promptSource;
  return (
    {
      template: "来自模板渲染",
      user_edited: "你编辑的",
      rule_mapped: "本地规则映射（无提示词）",
      none: "该预设不使用提示词",
    } as Record<string, string>
  )[s ?? ""] ?? "";
});
</script>

<template>
  <details class="peek" :open="open" @toggle="open = ($event.target as HTMLDetailsElement).open">
    <summary class="peek-sum">高级：这次会做什么</summary>

    <!-- 人话版执行步骤（始终可见，即便还没跑） -->
    <ul v-if="whatSteps.length" class="peek-steps">
      <li v-for="(s, i) in whatSteps" :key="i">{{ s }}</li>
    </ul>

    <!-- 目标提示词 / 参数预览：两者互斥 -->
    <template v-if="hasPrompt">
      <p class="peek-k">目标提示词 <span class="peek-src">{{ sourceLabel }}</span></p>
      <pre class="peek-prompt">{{ job?.prompt }}</pre>
    </template>

    <template v-else>
      <p v-if="params.length" class="peek-k">执行参数预览</p>
      <dl v-if="params.length" class="peek-params">
        <template v-for="[k, v] in params" :key="k">
          <dt>{{ k }}</dt>
          <dd>{{ v }}</dd>
        </template>
      </dl>
      <p v-if="isLocal" class="peek-note">
        本预设使用本地算法，不涉及模型提示词。如需 AI 重绘，请切换到模型类预设或使用完全自定义。
      </p>
      <p v-else-if="!params.length" class="peek-note">选好预设后，这里会显示本次的执行参数。</p>
    </template>
  </details>
</template>

<style scoped>
.peek {
  font-size: 11.5px;
  color: var(--fg-2, #a8a29a);
  border-top: 1px solid var(--line, rgba(255, 255, 255, 0.1));
  padding-top: 7px;
}

.peek-sum {
  cursor: pointer;
  font-size: 11px;
  letter-spacing: 0.04em;
  color: var(--fg-2, #a8a29a);
}

.peek-sum:hover {
  color: var(--fg, #ece7df);
}

.peek-steps {
  margin: 7px 0 0;
  padding-left: 16px;
  line-height: 1.7;
  color: var(--fg, #ece7df);
}

.peek-k {
  margin: 9px 0 4px;
  font-size: 11px;
  color: var(--fg-3, #6f6a63);
}

.peek-src {
  margin-left: 6px;
  color: var(--gold, #c8a45c);
}

/* 提示词用等宽：用户要能一眼看出锚点在哪、槽位填了什么 */
.peek-prompt {
  margin: 0;
  padding: 8px 10px;
  border-radius: 6px;
  background: var(--field, rgba(255, 255, 255, 0.04));
  color: var(--fg-2, #a8a29a);
  font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
  font-size: 11px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 200px;
  overflow: auto;
}

.peek-params {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 3px 10px;
  margin: 0;
}

.peek-params dt {
  color: var(--fg-3, #6f6a63);
}

.peek-params dd {
  margin: 0;
  color: var(--fg, #ece7df);
  font-variant-numeric: tabular-nums;
}

.peek-note {
  margin: 8px 0 0;
  font-size: 11px;
  line-height: 1.6;
  color: var(--fg-3, #6f6a63);
}
</style>
