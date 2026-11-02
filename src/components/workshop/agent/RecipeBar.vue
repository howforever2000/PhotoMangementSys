<script setup lang="ts">
/**
 * 配方栏：下拉选预设 + 附加描述 + 参数控件 + 开始/取消。
 *
 * 交互纪律（docs/v5.md §17）：
 * - 下拉里**只放能跑的**：planned 灰掉标「规划中」、needs_key 标「需配置模型服务」，
 *   不做假入口；
 * - 按钮文案随状态变（按预设 / 按你的描述），用户一眼知道这次有没有花钱让模型理解；
 * - 「开始」在没素材、任务运行中、配方不可用时都必须 disabled（防重复提交）。
 */
import { computed, ref, watch } from "vue";
import { useAgentStore } from "../../../stores/agent";
import { groupLabel, needsProvider, statusHint, type Recipe } from "../../../types/recipe";
import type { AgentSubmitRequest } from "../../../types/agent";

const props = defineProps<{ paths: string[]; busy: boolean }>();
const emit = defineEmits<{
  (e: "start", req: AgentSubmitRequest): void;
  (e: "cancel"): void;
}>();

const store = useAgentStore();

const recipeId = ref("");
const userMsg = ref("");
const targetKb = ref(200);
const intensityLevel = ref<"subtle" | "moderate" | "strong">("moderate");
const title = ref("");
const subtitle = ref("");

const recipe = computed<Recipe | null>(
  () => store.recipes.find((r) => r.id === recipeId.value) ?? null,
);

// 目录到手后默认选第一个能跑的（不给用户一个"点不动"的空下拉）
watch(
  () => store.recipes,
  (list) => {
    if (!recipeId.value && list.length) {
      recipeId.value = (list.find((r) => r.status === "ready") ?? list[0]).id;
    }
  },
  { immediate: true },
);

const LEVEL_VALUE: Record<string, number> = { subtle: 0.3, moderate: 0.6, strong: 0.9 };

// 换预设时控件回到该预设的默认值（不要让上一个预设的数字留在界面上）
watch(recipe, (r) => {
  if (!r) return;
  targetKb.value = r.runSpec.export?.targetKb || 200;
  const i = r.runSpec.style?.intensity ?? 0.6;
  intensityLevel.value = i < 0.45 ? "subtle" : i < 0.78 ? "moderate" : "strong";
  title.value = r.runSpec.layout?.title ?? "";
  subtitle.value = r.runSpec.layout?.subtitle ?? "";
});

const hasStyle = computed(() => !!recipe.value?.runSpec.style);
const hasTargetKb = computed(() => !!recipe.value?.runSpec.export?.targetKb);
const hasLayout = computed(() => !!recipe.value?.runSpec.layout);
const blockedByProvider = computed(() => !!recipe.value && needsProvider(recipe.value));

const startLabel = computed(() => {
  if (props.busy) return "正在执行…";
  if (userMsg.value.trim()) return "开始（按你的描述）";
  return "开始（按预设）";
});

const canStart = computed(
  () =>
    !!recipe.value &&
    recipe.value.status === "ready" &&
    !blockedByProvider.value &&
    !props.busy &&
    props.paths.length > 0,
);

const startHint = computed(() => {
  if (!props.paths.length) return "先把照片加入素材托盘";
  if (!recipe.value) return "先选一个预设";
  if (recipe.value.status !== "ready") return statusHint(recipe.value);
  if (blockedByProvider.value) return "该预设需要模型服务，请先在设置里填写地址与 Key";
  if (recipe.value.engine_behavior === "local_rule_mapped") {
    return "本地算法：描述只用于调整强度，不联网、不花钱";
  }
  if (recipe.value.engine_behavior === "no_prompt") {
    return "该预设不读文字描述：参数用上面的控件调";
  }
  return "";
});

function start() {
  const r = recipe.value;
  if (!r || !canStart.value) return;
  // 只发白名单字段（Python 侧 validators 会再校验一遍）
  const override: Record<string, Record<string, unknown>> = {};
  if (hasTargetKb.value) override.export = { targetKb: targetKb.value };
  if (hasStyle.value) override.style = { intensity: LEVEL_VALUE[intensityLevel.value] };
  if (hasLayout.value) override.layout = { title: title.value, subtitle: subtitle.value };
  emit("start", {
    recipeId: r.id,
    userMsg: userMsg.value.trim(),
    paths: props.paths,
    runSpecOverride: override,
  });
}
</script>

<template>
  <section class="rb">
    <label class="rb-row">
      <span class="rb-k">预设</span>
      <select v-model="recipeId" class="rb-select" :disabled="busy">
        <optgroup v-for="g in store.recipesByGroup" :key="g.group" :label="groupLabel(g.group)">
          <option v-for="r in g.items" :key="r.id" :value="r.id" :disabled="r.status !== 'ready'">
            {{ r.label }}{{ r.status === "ready" ? "" : `（${statusHint(r)}）` }}
          </option>
        </optgroup>
      </select>
    </label>

    <p v-if="recipe?.desc" class="rb-desc">{{ recipe.desc }}</p>

    <!-- 参数控件：都由 RunSpec 决定，不让用户靠写文字改数值 -->
    <div v-if="hasTargetKb" class="rb-row">
      <span class="rb-k">目标体积</span>
      <input v-model.number="targetKb" class="rb-num" type="number" min="20" max="20000" step="10" :disabled="busy" />
      <span class="rb-unit">KB</span>
    </div>

    <div v-if="hasStyle" class="rb-row">
      <span class="rb-k">强度</span>
      <div class="rb-seg" role="group" aria-label="强度">
        <button
          v-for="lv in (['subtle', 'moderate', 'strong'] as const)"
          :key="lv"
          type="button"
          class="rb-seg-btn"
          :class="{ on: intensityLevel === lv }"
          :disabled="busy"
          @click="intensityLevel = lv"
        >
          {{ lv === "subtle" ? "轻" : lv === "moderate" ? "中" : "强" }}
        </button>
      </div>
    </div>

    <template v-if="hasLayout">
      <label class="rb-row">
        <span class="rb-k">标题</span>
        <input v-model="title" class="rb-text" type="text" maxlength="40" placeholder="如：杭州 · 2023" :disabled="busy" />
      </label>
      <label class="rb-row">
        <span class="rb-k">副标题</span>
        <input v-model="subtitle" class="rb-text" type="text" maxlength="40" placeholder="如：秋" :disabled="busy" />
      </label>
    </template>

    <label class="rb-row rb-row-top">
      <span class="rb-k">描述</span>
      <textarea
        v-model="userMsg"
        class="rb-area"
        rows="2"
        maxlength="200"
        :disabled="busy"
        placeholder="可选。比如「再暖一点」「别改人脸」"
      />
    </label>

    <div class="rb-actions">
      <button v-if="!busy" class="rb-start" type="button" :disabled="!canStart" @click="start">
        {{ startLabel }}
      </button>
      <button v-else class="rb-cancel" type="button" @click="emit('cancel')">取消</button>
      <span class="rb-hint">{{ startHint }}</span>
    </div>
  </section>
</template>

<style scoped>
.rb {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.rb-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.rb-row-top {
  align-items: flex-start;
}

/* 标签列固定宽度：一行行读下来字段名对齐，比逐个括号省事 */
.rb-k {
  flex: none;
  width: 56px;
  font-size: 11.5px;
  color: var(--fg-2, #a8a29a);
}

.rb-select,
.rb-text,
.rb-num,
.rb-area {
  flex: 1;
  min-width: 0;
  padding: 5px 8px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.14));
  border-radius: 6px;
  background: var(--field, rgba(255, 255, 255, 0.04));
  color: var(--fg, #ece7df);
  font-size: 12.5px;
  font-family: inherit;
}

.rb-num {
  flex: none;
  width: 84px;
}

.rb-area {
  resize: vertical;
  line-height: 1.5;
}

.rb-select:disabled,
.rb-text:disabled,
.rb-num:disabled,
.rb-area:disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.rb-unit {
  font-size: 11.5px;
  color: var(--fg-2, #a8a29a);
}

.rb-seg {
  display: flex;
  gap: 4px;
}

.rb-seg-btn {
  min-width: 40px;
  padding: 4px 10px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.14));
  border-radius: 6px;
  background: none;
  color: var(--fg-2, #a8a29a);
  font-size: 12px;
  cursor: pointer;
  transition: color 0.15s ease, border-color 0.15s ease;
}

.rb-seg-btn:hover:not(:disabled) {
  color: var(--fg, #ece7df);
}

.rb-seg-btn.on {
  color: var(--gold, #c8a45c);
  border-color: var(--gold, #c8a45c);
}

.rb-seg-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.rb-desc {
  margin: 0;
  padding-left: 64px;
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--fg-2, #a8a29a);
}

.rb-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 2px;
}

.rb-start,
.rb-cancel {
  flex: none;
  padding: 7px 16px;
  border-radius: 7px;
  border: 1px solid var(--gold, #c8a45c);
  background: var(--gold, #c8a45c);
  color: #1b1713;
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
}

.rb-start:disabled {
  background: none;
  color: var(--fg-3, #6f6a63);
  border-color: var(--line, rgba(255, 255, 255, 0.14));
  cursor: not-allowed;
}

.rb-cancel {
  background: none;
  color: var(--fg, #ece7df);
  border-color: var(--line, rgba(255, 255, 255, 0.22));
}

.rb-hint {
  font-size: 11.5px;
  color: var(--fg-2, #a8a29a);
  line-height: 1.45;
}
</style>
