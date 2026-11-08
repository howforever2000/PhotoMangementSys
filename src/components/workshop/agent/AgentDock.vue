<script setup lang="ts">
/**
 * AI 创作工位（右栏）：配方栏 + 高级区 + 轨迹。
 *
 * 只做编排，不自己实现表单或轨迹 —— 那两块分别是 `RecipeBar` 与 `AgentTrail`，
 * 这样单独改一块不会牵连另一块的布局（组件轻量性）。
 *
 * 三态齐全：未开启（引导开关）/ 可提交 / 运行中（可取消）。
 */
import { computed, ref, watch } from "vue";
import { useAgentStore } from "../../../stores/agent";
import { useToastStore } from "../../../stores/toast";
import { needsProvider, type Recipe } from "../../../types/recipe";
import type { AgentSnapshot, AgentSubmitRequest } from "../../../types/agent";
import AgentTrail from "./AgentTrail.vue";
import PromptPeek from "./PromptPeek.vue";
import ProviderPanel from "./ProviderPanel.vue";
import RecipeBar from "./RecipeBar.vue";
import RunSettings from "./RunSettings.vue";

const props = defineProps<{ paths: string[] }>();

const store = useAgentStore();
const toast = useToastStore();

/** 本地提交中（防连点；与 job 的 running 分开：提交可能失败但 job 没建起来） */
const submitting = ref(false);
const error = ref("");

/** 运行设置：档位 / 预算 / 端点选择 */
const tier = ref<"draft" | "standard" | "hq">("draft");
const maxCost = ref(1.5);
const providerId = ref("");
const plannerId = ref("");

const busy = computed(() => submitting.value || store.running !== null);

/** 当前展示哪个 job：优先运行中的，其次最近一个 */
const shown = computed<AgentSnapshot | null>(() => store.running ?? store.current);

const shownRecipe = computed<Recipe | null>(
  () => store.recipes.find((r) => r.id === shown.value?.recipeId) ?? null,
);

/** 端点列表到手后给个默认值：没有比“未选择”更能拦住用户的默认值了 */
watch(
  () => store.providers,
  (list) => {
    if (!providerId.value) {
      const p = list.find((x) => x.kind !== "planner");
      if (p) providerId.value = p.id;
    }
    if (!plannerId.value) {
      const p = list.find((x) => x.kind !== "image") ?? list[0];
      if (p) plannerId.value = p.id;
    }
  },
  { immediate: true },
);

async function onStart(req: AgentSubmitRequest) {
  if (busy.value) return;
  submitting.value = true;
  error.value = "";
  try {
    const recipe = store.recipes.find((r) => r.id === req.recipeId);
    const needs = !!recipe && needsProvider(recipe);
    const paths = req.paths ?? [];
    await store.submit({
      ...req,
      tier: tier.value,
      providerId: providerId.value || null,
      plannerId: plannerId.value || null,
      needsProvider: needs,
      budget: {
        maxCost: maxCost.value,
        // 零成本路径：maxCalls=0 是“不可能发起模型请求”的机制保证，不是提示
        maxCalls: needs ? paths.length + 1 : 0,
      },
    });
  } catch (e) {
    const msg = typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
    error.value = msg;
    // 编译失败（如“该预设需要模型服务”）必须说清楚怎么办，不能只弹“出错了”
    toast.error("任务没能开始", msg);
  } finally {
    submitting.value = false;
  }
}

async function onCancel() {
  const job = store.running;
  if (!job) return;
  try {
    await store.cancel(job.jobId);
  } catch (e) {
    toast.error("取消失败", typeof e === "string" ? e : String(e));
  }
}
</script>

<template>
  <section class="dock">
    <!-- 未开启：不做假入口，给出唯一可行动的一步 -->
    <div v-if="!store.enabled" class="dock-off">
      <p class="dock-off-title">AI 创作还没开启。</p>
      <p class="dock-off-body">
        开启后，可以用一句话描述想要的效果（比如「再暖一点」），也可以什么都不写直接按预设出活。
        本地算法的预设不联网、不花钱。
      </p>
      <p class="dock-off-step">在左侧栏勾选「启用 AI 创作」即可。</p>
    </div>

    <template v-else>
      <ProviderPanel />
      <RunSettings
        v-model:tier="tier"
        v-model:max-cost="maxCost"
        v-model:provider-id="providerId"
        v-model:planner-id="plannerId"
        :needs-provider="!!shownRecipe && needsProvider(shownRecipe)"
      />
      <RecipeBar :paths="props.paths" :busy="busy" @start="onStart" @cancel="onCancel" />

      <p v-if="error" class="dock-error">{{ error }}</p>

      <PromptPeek :job="shown" :recipe="shownRecipe" />

      <AgentTrail :job="shown" />
    </template>
  </section>
</template>

<style scoped>
.dock {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 14px 16px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.1));
  border-radius: 10px;
  background: var(--panel, rgba(255, 255, 255, 0.02));
}

.dock-off {
  padding: 6px 2px;
}

.dock-off-title {
  margin: 0 0 6px;
  font-size: 13px;
  font-weight: 600;
  color: var(--fg, #ece7df);
}

.dock-off-body {
  margin: 0 0 8px;
  font-size: 12px;
  line-height: 1.65;
  color: var(--fg-2, #a8a29a);
}

.dock-off-step {
  margin: 0;
  font-size: 12px;
  color: var(--gold, #c8a45c);
}

.dock-error {
  margin: 0;
  padding: 6px 9px;
  border-left: 3px solid #d9534f;
  background: rgba(217, 83, 79, 0.08);
  border-radius: 0 5px 5px 0;
  font-size: 11.5px;
  line-height: 1.55;
  color: #e07974;
}
</style>
