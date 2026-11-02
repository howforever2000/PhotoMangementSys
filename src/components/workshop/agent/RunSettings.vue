<script setup lang="ts">
/**
 * 运行设置：档位 + 预算上限 + 用哪个端点。
 *
 * 只有"需要模型"的预设才显示端点与预算 —— 本地算法预设显示这些只会让用户
 * 以为"是不是要花钱"。所以这一块由父组件按 recipe 决定要不要渲染。
 */
import { computed } from "vue";
import { useAgentStore } from "../../../stores/agent";

const props = defineProps<{
  /** 预设是否需要模型（`needsProvider(recipe)`） */
  needsProvider: boolean;
}>();
// template 里用的是 props.needsProvider（defineProps 的返回值不必再解构）
void props;

const tier = defineModel<"draft" | "standard" | "hq">("tier", { default: "draft" });
const maxCost = defineModel<number>("maxCost", { default: 1.5 });
const providerId = defineModel<string>("providerId", { default: "" });
const plannerId = defineModel<string>("plannerId", { default: "" });

const store = useAgentStore();

const imageOptions = computed(() => store.providers.filter((p) => p.kind !== "planner"));
const plannerOptions = computed(() => store.providers.filter((p) => p.kind !== "image"));

const estimate = computed(() => {
  const p = store.providers.find((x) => x.id === providerId.value);
  return p && p.unitPrice > 0 ? p.unitPrice : 0;
});

const overBudget = computed(() => estimate.value > 0 && maxCost.value > 0 && estimate.value > maxCost.value);
</script>

<template>
  <div class="rs">
    <label class="rs-row">
      <span class="rs-k">出图端点</span>
      <select v-model="providerId" class="rs-input">
        <option value="">（未选择）</option>
        <option v-for="p in imageOptions" :key="p.id" :value="p.id">
          {{ p.label }} · {{ p.model }}{{ p.unitPrice > 0 ? ` · ¥${p.unitPrice}/张` : "" }}
        </option>
      </select>
    </label>

    <label class="rs-row">
      <span class="rs-k">规划端点</span>
      <select v-model="plannerId" class="rs-input">
        <option value="">（未选择）</option>
        <option v-for="p in plannerOptions" :key="p.id" :value="p.id">
          {{ p.label }} · {{ p.model }}
        </option>
      </select>
    </label>

    <label class="rs-row">
      <span class="rs-k">档位</span>
      <select v-model="tier" class="rs-input">
        <option value="draft">草稿（便宜先看效果）</option>
        <option value="standard">成品</option>
        <option value="hq">高清（最贵）</option>
      </select>
    </label>

    <label class="rs-row">
      <span class="rs-k">花费上限</span>
      <input v-model.number="maxCost" class="rs-input rs-num" type="number" min="0" step="0.1" />
      <span class="rs-unit">元（0 = 不限制）</span>
    </label>

    <p v-if="overBudget" class="rs-warn">
      单价 ¥{{ estimate }} 已超过上限 ¥{{ maxCost }}，提交会被直接拦下。请调高上限或换更便宜的端点。
    </p>
    <p v-else-if="needsProvider && !imageOptions.length" class="rs-warn">
      还没有出图端点。展开上面的「模型服务」添加一个（地址 + Key + 模型名）。
    </p>
  </div>
</template>

<style scoped>
.rs {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.rs-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.rs-k {
  flex: none;
  width: 56px;
  font-size: 11.5px;
  color: var(--fg-2, #a8a29a);
}

.rs-input {
  flex: 1;
  min-width: 0;
  padding: 4px 7px;
  border: 1px solid var(--line, rgba(255, 255, 255, 0.14));
  border-radius: 5px;
  background: var(--field, rgba(255, 255, 255, 0.04));
  color: var(--fg, #ece7df);
  font-size: 11.5px;
  font-family: inherit;
}

.rs-num {
  flex: none;
  width: 84px;
}

.rs-unit {
  font-size: 10.5px;
  color: var(--fg-3, #6f6a63);
}

.rs-warn {
  margin: 0;
  padding: 5px 8px;
  border-left: 3px solid var(--gold, #c8a45c);
  background: rgba(200, 164, 92, 0.08);
  border-radius: 0 5px 5px 0;
  font-size: 11px;
  line-height: 1.5;
  color: var(--fg, #ece7df);
}
</style>
