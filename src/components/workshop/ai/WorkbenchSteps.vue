<script setup lang="ts">
/**
 * 工作台步骤链编辑器（FEAT-177 / FEAT-197）
 * ===================================================================
 * 「创意组合」是一条有序的原子组件链：点「添加」即加入、参数就地改、不要的删掉。
 *
 * ## FEAT-197 的三处变化
 *
 * ① **同一组件可以多次插入**：每一步有稳定 `uid`，`:key` / 事件 / 手风琴状态全部
 *    按 uid 走（用 op 会触发 Vue 重复 key，且删一步会连坐同名的另一步）。
 * ② **云端组件也能进链**：候选表从「只收本机」放开到全部非 internal 算子；
 *    云端步会标来源并说明「预览里不跑」，避免用户以为滑杆没生效。
 * ③ **上限 10 步**：到顶后「添加」按钮禁用并给出原因，不在提交时才报错
 *    （后端 `validate_plan` 到 10 步会直接拒单）。
 *
 * FEAT-199：来源标签不再一律写「模型」，改按 `utils/modelOrigin` 的四档
 * （本机算法 / 本机模型 / 云端 API / 未配置模型）—— 链条里可以同时存在
 * 「绑在本机 Ollama 的云端步」与「要本地 ONNX 模型的本机步」，一律写「模型」
 * 等于什么都没说。判定是纯函数，`settings` 由页面传入，本组件仍不发请求。
 *
 * 为什么仍然没有排序控件：链条顺序由 catalog 的 `stage` 决定（全序依赖），
 * 人工排序没有任何合法位置，给了就是死控件（同类教训：FEAT-168 的 strength 滑杆）。
 *
 * 纯展示 + 事件：不改自己的 props，所有变更 emit 给页面 → store。
 */
import { computed, ref, watch } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { GROUPS, localToolDefs } from "../../../utils/toolCatalog";
import { originOfOp, type OriginInfo } from "../../../utils/modelOrigin";
import type { AiSettings } from "../../../types/ai";
import {
  applySlider,
  canAddOp,
  chainableOps,
  coerceEnum,
  enumText,
  isCloudOp,
  MAX_CHAIN_STEPS,
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
  /** 已绑定的供应商（FEAT-199）：用于判「云端步」到底是云端 API 还是本机服务 */
  settings?: AiSettings | null;
}>();

const emit = defineEmits<{
  /** 加一步（允许同一组件多次；上限由本组件拦） */
  add: [op: string];
  remove: [uid: string];
  toggle: [uid: string];
  param: [uid: string, key: string, value: number | string | boolean];
}>();

/** 参考图调色步：就地选参考图（路径作为该步的 string 参数进链） */
async function pickReference(step: WbChainStep): Promise<void> {
  const picked = await openDialog({
    multiple: false,
    directory: false,
    title: "选择参考图（调色来源）",
    filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp"] }],
  });
  if (typeof picked === "string") emit("param", step.uid, "reference_path", picked);
}

/** 手风琴：同时只展开一步，步骤多了卡片也不会长到视口外（按 uid） */
const openUid = ref<string | null>(props.steps[0]?.uid ?? null);
const candOpen = ref(false);

/** 手风琴状态与链条保持一致：当前展开的步被删掉时，退回第一步 */
watch(
  () => props.steps.map((s) => s.uid),
  (uids) => {
    if (!openUid.value || !uids.includes(openUid.value)) openUid.value = uids[0] ?? null;
  },
);

const ops = computed(() => chainableOps());
const atLimit = computed(() => !canAddOp(props.steps));

/** 候选按 catalog 的展示分组（影调与滤镜 / 人像 / 画质 / 创作），顺序即左栏顺序 */
const groups = computed(() => {
  const map = new Map<
    string,
    { label: string; items: { id: string; label: string; origin: OriginInfo; count: number }[] }
  >();
  for (const o of ops.value) {
    const gid = opGroup(o.id);
    if (!map.has(gid)) map.set(gid, { label: groupLabel(gid), items: [] });
    map.get(gid)!.items.push({
      id: o.id,
      label: opLabel(o.id),
      origin: originOfOp(o.id, props.settings),
      count: props.steps.filter((s) => s.op === o.id).length,
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
    compress: "走压缩管线",
  };
  return local
    .filter((t) => !t.op || !chainableIds.has(t.op))
    .map((t) => ({ label: t.label, why: reason[t.id] ?? "独立工具" }));
});

function toggleOpen(uid: string): void {
  openUid.value = openUid.value === uid ? null : uid;
}

function onRange(step: WbChainStep, key: string, e: Event): void {
  const slider = Number((e.target as HTMLInputElement).value);
  const raw = applySlider(step, key, slider)[key];
  emit("param", step.uid, key, raw);
}

function onSelect(step: WbChainStep, key: string, e: Event): void {
  emit("param", step.uid, key, coerceEnum(step.op, key, (e.target as HTMLSelectElement).value));
}

function addOp(op: string): void {
  if (atLimit.value) return;
  emit("add", op);
}

/** 参考图按钮里只显示文件名（完整路径放 title 提示） */
function fileNameOf(path: string): string {
  const i = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return i >= 0 ? path.slice(i + 1) : path;
}
</script>

<template>
  <div class="wbs">
    <ol class="wbs-list">
      <li
        v-for="(step, i) in steps"
        :key="step.uid"
        class="wbs-step"
        :class="{ open: openUid === step.uid, off: !step.enabled }"
      >
        <span class="wbs-n">{{ i + 1 }}</span>
        <div class="wbs-body">
          <div class="wbs-top">
            <button
              type="button"
              class="wbs-name"
              :aria-expanded="openUid === step.uid"
              @click="toggleOpen(step.uid)"
            >
              {{ opLabel(step.op) }}
              <span v-if="isCloudOp(step.op)" class="wbs-tag" title="云端模型，按调用计费">模型</span>
              <span class="wbs-sum">{{ stepSummary(step) }}</span>
              <span class="wbs-chev">{{ openUid === step.uid ? "▾" : "▸" }}</span>
            </button>
            <button
              type="button"
              class="wbs-eye"
              :class="{ off: !step.enabled }"
              :title="step.enabled ? '停用这一步' : '启用这一步'"
              :disabled="disabled"
              @click="emit('toggle', step.uid)"
            >
              {{ step.enabled ? "◉" : "○" }}
            </button>
            <button
              type="button"
              class="wbs-x"
              title="移除这一步"
              :disabled="disabled"
              @click="emit('remove', step.uid)"
            >
              ✕
            </button>
          </div>

          <div v-if="openUid === step.uid" class="wbs-params">
            <div v-for="p in paramViews(step)" :key="p.key" class="wbs-field">
              <div class="wbs-field-h">
                <span>{{ p.label }}</span>
                <span v-if="p.kind === 'range'" class="wbs-val">{{ sliderOf(step, p.key) }}%</span>
                <span v-else-if="p.kind === 'select'" class="wbs-val">{{ enumText(step, p.key) }}</span>
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
              <!-- 参考图：路径进不了下拉/滑杆，就地给一个选图按钮 -->
              <div v-else-if="p.kind === 'reference'" class="wbs-ref">
                <span class="wbs-ref-path" :title="p.value || '尚未选择'">
                  {{ p.value ? fileNameOf(p.value) : "尚未选择参考图" }}
                </span>
                <button
                  type="button"
                  class="wbs-ref-btn"
                  :disabled="disabled"
                  @click="pickReference(step)"
                >
                  选择参考图…
                </button>
              </div>
            </div>
            <p v-if="!paramViews(step).length" class="wbs-tip">这一步没有可调参数。</p>
            <p
              v-if="step.op === 'color_transfer' && !String(step.params.reference_path || '').trim()"
              class="wbs-tip"
            >
              这一步需要参考图：先在上面选一张，否则运行时会跳过并提示。
            </p>
            <p v-if="isCloudOp(step.op)" class="wbs-tip">
              云端步：即时预览里不跑（每次预览都是计费调用），出片时照常执行。
            </p>
          </div>
        </div>
      </li>
    </ol>

    <p v-if="!steps.length" class="wbs-tip">还没有步骤，点下面「添加步骤」。</p>

    <button
      type="button"
      class="wbs-add"
      :disabled="disabled || atLimit"
      :title="atLimit ? `最多 ${MAX_CHAIN_STEPS} 步，先删掉一些` : ''"
      @click="candOpen = !candOpen"
    >
      ＋ 添加步骤
      <span class="wbs-count">{{ steps.length }}/{{ MAX_CHAIN_STEPS }}</span>
    </button>
    <p v-if="atLimit" class="wbs-tip">已到 {{ MAX_CHAIN_STEPS }} 步上限。</p>

    <div v-if="candOpen && !atLimit" class="wbs-cands">
      <div v-for="g in groups" :key="g.label" class="wbs-group">
        <h4>{{ g.label }}</h4>
        <button
          v-for="o in g.items"
          :key="o.id"
          type="button"
          class="wbs-cand"
          :disabled="disabled"
          @click="addOp(o.id)"
        >
          {{ o.label }}
          <em>{{ o.count ? `已加 ${o.count}` : isCloudOp(o.id) ? "模型" : "本机" }}</em>
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
      同一个组件可以加多次（各自调参数），顺序由依赖自动决定，不用手动排。
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
.wbs-tag {
  flex: none;
  padding: 0 5px;
  border-radius: 999px;
  font-size: 9px;
  line-height: 14px;
  color: var(--c-accent);
  border: 1px solid color-mix(in srgb, var(--c-accent) 45%, transparent);
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

/* 参考图行：文件名 + 选图按钮，一行放下（与候选按钮同一视觉语言） */
.wbs-ref {
  display: flex;
  align-items: center;
  gap: 6px;
}
.wbs-ref-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.wbs-ref-btn {
  flex: none;
  height: 24px;
  padding: 0 8px;
  border: 1px solid var(--c-rule);
  border-radius: 6px;
  background: none;
  color: var(--fg-2);
  font: inherit;
  font-size: var(--fs-meta);
  cursor: pointer;
}
.wbs-ref-btn:hover:not(:disabled) {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 55%, transparent);
}
.wbs-ref-btn:disabled {
  opacity: 0.4;
  cursor: default;
}

.wbs-add {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
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
.wbs-count {
  font-size: 9.5px;
  color: var(--fg-3, var(--fg-2));
  font-variant-numeric: tabular-nums;
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
