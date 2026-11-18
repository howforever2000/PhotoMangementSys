<script setup lang="ts">
/**
 * 工作台步骤链编辑器（FEAT-177）
 * ===================================================================
 * 「自己组合」就是一条有序的算子链：勾选即加入、参数就地改、不要的删掉。
 *
 * 为什么没有排序控件：
 *   现有 5 个本机算子在 `op_catalog.json` 里是**全序依赖**（stage 0→4，
 *   requiresBefore 串成 调色→胶片→形变→磨皮），人工排序没有任何合法位置，
 *   给了就是死控件（同类教训：FEAT-168 里绑定了却不提交的 strength 滑杆）。
 *   所以「勾选」负责选哪些步，顺序由依赖决定，后端 `validate_plan` 不会拒。
 *
 * 为什么不是 graph：一条链已经够用，节点连线只会让「懒人主路径」变重。
 *
 * 纯展示 + 事件：不改自己的 props，所有变更 emit 给页面 → store。
 */
import { computed, ref, watch } from "vue";

import { GROUPS, localToolDefs } from "../../../utils/toolCatalog";
import {
  applySlider,
  chainableOps,
  coerceEnum,
  enumText,
  opGroup,
  opLabel,
  paramViews,
  sliderOf,
  stepSummary,
  type WbChainStep,
} from "../../../utils/workbenchPlan";

const props = defineProps<{
  steps: WbChainStep[];
  disabled?: boolean;
}>();

const emit = defineEmits<{
  add: [op: string];
  remove: [op: string];
  toggle: [op: string];
  param: [op: string, key: string, value: number | string | boolean];
}>();

/** 手风琴：同时只展开一步，步骤多了卡片也不会长到视口外 */
const openOp = ref<string | null>(props.steps[0]?.op ?? null);
const candOpen = ref(false);

/** 手风琴状态与链条保持一致：当前展开的步被删掉时，退回第一步 */
watch(
  () => props.steps.map((s) => s.op),
  (ops) => {
    if (!openOp.value || !ops.includes(openOp.value)) openOp.value = ops[0] ?? null;
  },
);

const ops = computed(() => chainableOps());

/** 候选按 catalog 的展示分组（影调与滤镜 / 人像 / 画质 …），顺序即左栏顺序 */
const groups = computed(() => {
  const map = new Map<string, { label: string; items: { id: string; label: string; added: boolean }[] }>();
  for (const o of ops.value) {
    const gid = opGroup(o.id);
    if (!map.has(gid)) map.set(gid, { label: groupLabel(gid), items: [] });
    map.get(gid)!.items.push({
      id: o.id,
      label: opLabel(o.id),
      added: props.steps.some((s) => s.op === o.id),
    });
  }
  return [...map.values()];
});

/** 分组中文名来自 catalog 的 groups（toolCatalog.GROUPS），不手抄 */
function groupLabel(id: string): string {
  return GROUPS.find((g) => g.id === id)?.label ?? id;
}

/** 本机但**进不了链条**的入口：说明理由，而不是静默消失 */
const excluded = computed(() => {
  const local = localToolDefs();
  const chainableIds = new Set(ops.value.map((o) => o.id));
  const reason: Record<string, string> = {
    ocr: "产物是文本",
    color_transfer: "需要参考图",
    compress: "走压缩管线",
  };
  return local
    .filter((t) => !t.op || !chainableIds.has(t.op))
    .map((t) => ({ label: t.label, why: reason[t.id] ?? "独立工具" }));
});

function toggleOpen(op: string): void {
  openOp.value = openOp.value === op ? null : op;
}

function onRange(step: WbChainStep, key: string, e: Event): void {
  const slider = Number((e.target as HTMLInputElement).value);
  const raw = applySlider(step, key, slider)[key];
  emit("param", step.op, key, raw);
}

function onSelect(step: WbChainStep, key: string, e: Event): void {
  emit("param", step.op, key, coerceEnum(step.op, key, (e.target as HTMLSelectElement).value));
}
</script>

<template>
  <div class="wbs">
    <ol class="wbs-list">
      <li
        v-for="(step, i) in steps"
        :key="step.op"
        class="wbs-step"
        :class="{ open: openOp === step.op, off: !step.enabled }"
      >
        <span class="wbs-n">{{ i + 1 }}</span>
        <div class="wbs-body">
          <div class="wbs-top">
            <button
              type="button"
              class="wbs-name"
              :aria-expanded="openOp === step.op"
              @click="toggleOpen(step.op)"
            >
              {{ opLabel(step.op) }}
              <span class="wbs-sum">{{ stepSummary(step) }}</span>
              <span class="wbs-chev">{{ openOp === step.op ? "▾" : "▸" }}</span>
            </button>
            <button
              type="button"
              class="wbs-eye"
              :class="{ off: !step.enabled }"
              :title="step.enabled ? '停用这一步' : '启用这一步'"
              :disabled="disabled"
              @click="emit('toggle', step.op)"
            >
              {{ step.enabled ? "◉" : "○" }}
            </button>
            <button
              type="button"
              class="wbs-x"
              title="移除这一步"
              :disabled="disabled"
              @click="emit('remove', step.op)"
            >
              ✕
            </button>
          </div>

          <div v-if="openOp === step.op" class="wbs-params">
            <div v-for="p in paramViews(step)" :key="p.key" class="wbs-field">
              <div class="wbs-field-h">
                <span>{{ p.label }}</span>
                <span v-if="p.kind === 'range'" class="wbs-val">{{ sliderOf(step, p.key) }}%</span>
                <span v-else class="wbs-val">{{ enumText(step, p.key) }}</span>
              </div>
              <select
                v-if="p.kind === 'select' && p.options.length"
                class="wbs-select"
                :value="p.value"
                :disabled="disabled"
                @change="onSelect(step, p.key, $event)"
              >
                <option v-for="o in p.options" :key="o.value" :value="o.value">{{ o.label }}</option>
              </select>
              <input
                v-else-if="p.kind === 'range'"
                class="wbs-range"
                type="range"
                min="0"
                max="100"
                :value="sliderOf(step, p.key)"
                :disabled="disabled"
                @input="onRange(step, p.key, $event)"
              />
            </div>
            <p v-if="!paramViews(step).length" class="wbs-tip">这一步没有可调参数。</p>
          </div>
        </div>
      </li>
    </ol>

    <p v-if="!steps.length" class="wbs-tip">还没有步骤，点下面「添加步骤」。</p>

    <button type="button" class="wbs-add" :disabled="disabled" @click="candOpen = !candOpen">
      ＋ 添加步骤
    </button>

    <div v-if="candOpen" class="wbs-cands">
      <div v-for="g in groups" :key="g.label" class="wbs-group">
        <h4>{{ g.label }}</h4>
        <button
          v-for="o in g.items"
          :key="o.id"
          type="button"
          class="wbs-cand"
          :disabled="o.added"
          @click="emit('add', o.id)"
        >
          {{ o.label }}
          <em>{{ o.added ? "已添加" : "本机" }}</em>
        </button>
      </div>
      <div v-if="excluded.length" class="wbs-group">
        <h4>不能进链条</h4>
        <button
          v-for="x in excluded"
          :key="x.label"
          type="button"
          class="wbs-cand"
          disabled
          :title="x.why"
        >
          {{ x.label }}
          <em>{{ x.why }}</em>
        </button>
      </div>
    </div>

    <p class="wbs-note">
      勾选即加入，顺序由依赖自动决定（调色 → 胶片 → 形变 → 磨皮 → 放大），不用手动排。
    </p>
  </div>
</template>

<style scoped>
.wbs {
  min-width: 0;
}
.wbs-list {
  margin: 0;
  padding: 0;
  list-style: none;
}
.wbs-step {
  display: flex;
  gap: 8px;
  padding: 5px 4px 5px 0;
  border-radius: 6px;
}
.wbs-step:hover {
  background: var(--hover);
}
.wbs-step.off {
  opacity: 0.55;
}
.wbs-n {
  flex: none;
  width: 16px;
  height: 16px;
  margin-top: 2px;
  border-radius: 50%;
  display: grid;
  place-items: center;
  font-size: 9.5px;
  color: var(--c-accent);
  background: color-mix(in srgb, var(--c-accent) 14%, transparent);
  font-variant-numeric: tabular-nums;
}
.wbs-body {
  flex: 1;
  min-width: 0;
}
.wbs-top {
  display: flex;
  align-items: center;
  gap: 4px;
}
.wbs-name {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 0;
  border: none;
  background: none;
  font: inherit;
  font-size: var(--fs-body);
  color: var(--fg);
  text-align: left;
  cursor: pointer;
}
.wbs-sum {
  margin-left: auto;
  font-size: 10.5px;
  color: var(--fg-3, var(--fg-2));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 110px;
}
.wbs-chev {
  flex: none;
  font-size: 10px;
  color: var(--fg-2);
}
.wbs-eye,
.wbs-x {
  flex: none;
  width: 20px;
  height: 20px;
  border: none;
  border-radius: 4px;
  background: none;
  color: var(--fg-2);
  font-size: 11px;
  cursor: pointer;
}
.wbs-eye:hover,
.wbs-x:hover {
  color: var(--fg);
  background: var(--hover);
}
.wbs-eye.off {
  color: var(--fg-3, var(--fg-2));
}
.wbs-x:hover {
  color: var(--c-danger, #e2706d);
}
.wbs-eye:disabled,
.wbs-x:disabled {
  opacity: 0.4;
  cursor: default;
}

.wbs-params {
  padding: 4px 0 2px;
}
.wbs-field {
  margin: 6px 0;
}
.wbs-field-h {
  display: flex;
  justify-content: space-between;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  margin-bottom: 4px;
}
.wbs-val {
  color: var(--c-accent);
  font-variant-numeric: tabular-nums;
}
.wbs-range {
  width: 100%;
  accent-color: var(--c-accent);
}
.wbs-select {
  width: 100%;
  height: 26px;
  padding: 0 7px;
  border-radius: 6px;
  border: 1px solid var(--c-rule);
  /* 跟随主题卡片底色：深色微亮 / 浅色微暗 */
  background: color-mix(in srgb, var(--fg) 8%, transparent);
  color: var(--fg);
  font: inherit;
  font-size: var(--fs-meta);
}

.wbs-add {
  width: 100%;
  height: 27px;
  margin-top: 4px;
  border: 1px dashed var(--c-rule);
  border-radius: 6px;
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-body);
  cursor: pointer;
}
.wbs-add:hover:not(:disabled) {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 55%, transparent);
}
.wbs-add:disabled {
  opacity: 0.4;
  cursor: default;
}

.wbs-cands {
  margin-top: 6px;
  padding: 6px;
  border: 1px solid var(--c-rule);
  border-radius: 6px;
  background: color-mix(in srgb, var(--fg) 6%, transparent);
}
.wbs-group h4 {
  margin: 6px 4px 3px;
  font-size: var(--fs-label);
  letter-spacing: 0.12em;
  text-transform: uppercase;
  font-weight: 600;
  color: var(--fg-2);
}
.wbs-cand {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  height: 26px;
  padding: 0 8px;
  border: none;
  border-radius: 5px;
  background: none;
  color: var(--fg-2);
  font: inherit;
  font-size: var(--fs-meta);
  text-align: left;
  cursor: pointer;
}
.wbs-cand:hover:not(:disabled) {
  color: var(--fg);
  background: var(--hover);
}
.wbs-cand:disabled {
  opacity: 0.42;
  cursor: default;
}
.wbs-cand em {
  margin-left: auto;
  font-style: normal;
  font-size: 9.5px;
  color: var(--fg-3, var(--fg-2));
}
.wbs-tip,
.wbs-note {
  margin: 6px 0 0;
  font-size: 10.5px;
  line-height: 1.55;
  color: var(--fg-3, var(--fg-2));
}
</style>




