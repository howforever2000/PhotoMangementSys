<script setup lang="ts">
/**
 * 运行前「计费预估」提示（FEAT-152 / FEAT-157）
 * 四个 AI 工具共用：按该工具所用模型的单价 × 张数显示预估金额与调用次数。
 * 单价未配置时提示去「模型设置」对应模型卡片下展开「计费」填写。
 */
import { computed } from "vue";
import type { AiTool } from "../../../types/ai";
import { useAiBilling } from "../../../composables/useAiBilling";

const props = defineProps<{ tool: AiTool; count: number }>();
const billing = useAiBilling();
const cost = computed(() => billing.estimate(props.tool, props.count));
</script>

<template>
  <p v-if="cost.configured" class="ai-hint ai-billing-hint">
    💡 本次 {{ cost.count }} 张 × {{ billing.formatMoney(cost.unit, cost.currency) }}/张 ≈
    <strong>{{ billing.formatMoney(cost.total, cost.currency) }}</strong>（预估，实际以服务商账单为准）
  </p>
  <p v-else class="ai-hint ai-billing-hint">
    💡 未配置该模型单价，可在「模型设置」对应模型卡片下展开「计费」填写后显示费用预估
  </p>
</template>
