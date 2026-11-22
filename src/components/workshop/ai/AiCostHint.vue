<script setup lang="ts">
/**
 * 运行前「计费预估」提示（FEAT-152 / FEAT-157 / FEAT-199）
 * 四个云端工具共用：按该工具所用模型的单价 × 张数显示预估金额与调用次数。
 *
 * FEAT-199 起先判来源再说钱：
 *   - 本机（纯算法 / 自带模型 / 走回环地址的 API）→ 直接说「免费」，不再提示「去填单价」
 *     —— 对免费链路要求填价只会让用户以为哪里没配好；
 *   - 未配置模型 → 说清「现在运行会报未配置」，而不是让用户去填单价；
 *   - API 模型 · 云端 → 保持原口径（已填单价显示预估，未填提示去填）。
 * 判据来自 `useAiBilling.estimate`，与工坊角标同源，不在两处各判一次。
 */
import { computed } from "vue";
import type { AiSettings, AiTool } from "../../../types/ai";
import { useAiBilling } from "../../../composables/useAiBilling";

const props = defineProps<{ tool: AiTool; count: number; settings?: AiSettings | null }>();
const billing = useAiBilling();
const cost = computed(() => billing.estimate(props.tool, props.count, props.settings));
const local = computed(() => !cost.value.origin.leavesDevice && cost.value.origin.origin !== "unconfigured");
</script>

<template>
  <p v-if="cost.origin.origin === 'unconfigured'" class="ai-hint ai-billing-hint">
    ⚠ {{ cost.origin.detail }}
  </p>
  <p v-else-if="local" class="ai-hint ai-billing-hint">
    ✓ {{ cost.origin.detail }}
  </p>
  <p v-else-if="cost.configured" class="ai-hint ai-billing-hint">
    💡 本次 {{ cost.count }} 张 × {{ billing.formatMoney(cost.unit, cost.currency) }}/张 ≈
    <strong>{{ billing.formatMoney(cost.total, cost.currency) }}</strong>（预估，实际以服务商账单为准）
  </p>
  <p v-else class="ai-hint ai-billing-hint">
    💡 未配置该模型单价，可在「模型设置」对应模型卡片下展开「计费」填写后显示费用预估
  </p>
</template>
