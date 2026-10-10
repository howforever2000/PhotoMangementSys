<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useEditStore } from "../../../stores/edit";
import { useNotify } from "../../../composables/useNotify";
import { cloneRecipe } from "../../../types/edit";
import type { OpStep } from "../../../types/edit";
import type { OpDef, OpParam } from "../useStudioOps";

/**
 * 算子面板（融合版）——把「传统图像处理」并入基准调色编辑器
 * ====================================================================
 * ## 语义：每一步都记录在图像上
 *
 * 融合前是两个独立小组件，各自载入图片、各自保存。融合后算子步骤**进配方**
 * （`recipe.ops`），与调色参数一起存进 sidecar。因此：
 *
 * - 关掉应用再打开，算子还在（不是内存里的临时状态）；
 * - 导出时按同一条链重跑，预览与导出必然一致；
 * - 每步可单独禁用/删除/调参，且都在撤销栈里。
 *
 * 之前这个面板是「烘焙」模型 —— 点一次算子出一个 blob，改任何调色参数就
 * 过期、也不进 sidecar。那不满足「记录在图像上」，已改掉。
 *
 * ## 为什么算子还留在 Python 侧
 *
 * 算子是「算子形态」的滤镜（均衡化、四种模糊）：参数少、用户不追求逐帧
 * 实时预览、而 OpenCV 的实现是几十年积累的边界处理。搬到 Rust 要重写并
 * 逐个对齐行为，收益只是省掉几百毫秒。调色恰好相反 —— 拖滑块要每帧出图，
 * 微服务的往返根本没法用。两类需求形态不同，不必强行统一到同一个后端。
 *
 * ## 编排：Rust 出底图 → Python 按序加工
 *
 * ```
 *   原图 ──Rust 管线(含蒙版)──▶ 底图 JPEG ──▶ op₁ ──▶ op₂ ──▶ … ──▶ 结果
 * ```
 *
 * 关键点是**输入是 Rust 的渲染结果**：算子作用在「已调色」的画面上，与用户
 * 眼前的预览一致。若作用在调色前的原图上，会出现「预览里有效果、导出没有」
 * 的错位 —— 因为调色会把算子效果盖掉。
 */
const props = defineProps<{
  /** 算子列表（由 useStudioOps 提供） */
  ops: OpDef[];
  /** 加载态 */
  loading: boolean;
  /** 加载错误 */
  error: string;
  /** 取当前「Rust 渲染出的底图」字节 —— 算子链的输入 */
  getInput: () => Promise<Blob | null>;
  /**
   * 取当前蒙版 PNG —— 与 Rust 侧共用同一份。
   *
   * 但**是否使用**蒙版由每一步自己决定：宿主按「本步骤勾了只用选区」来
   * 决定传不传，因此这里恒返回当前蒙版，由宿主在调用 `apply` 时决定
   * 传 `mask` 还是 `null`。组件不传 flag，避免两处各判一次判出不同结果。
   */
  getMask: () => Promise<Blob | null>;
  /** 执行单个算子 */
  apply: (
    opId: string,
    values: Record<string, number | string | boolean>,
    image: Blob,
    mask: Blob | null,
  ) => Promise<Blob>;
  /** 蒙版是否启用（决定算子是否传蒙版） */
  maskActive: boolean;
  /** 整条链的执行结果（由宿主跑完后回填） */
  resultBlob?: Blob | null;
  /** 链是否正在跑 */
  running?: boolean;
  /** 链执行失败信息 */
  runError?: string;
}>();

const emit = defineEmits<{
  /** 配方里的步骤变了，需要宿主重跑整条链 */
  (e: "changed"): void;
}>();

const store = useEditStore();
const notify = useNotify();

const DEFAULT_OP = "region-equalize";
const opId = ref<string>(DEFAULT_OP);
const values = ref<Record<string, number | string | boolean>>({});
/** 是否用蒙版限制这一步（默认跟随「有蒙版就带上」） */
const useMask = ref(true);

const currentOp = computed<OpDef | null>(() => props.ops.find((o) => o.id === opId.value) ?? null);
const steps = computed<OpStep[]>(() => store.recipe.ops);

/** 算子 label（步骤列表展示用）。找不到时退回 id，不隐藏信息。 */
function labelOf(id: string): string {
  return props.ops.find((o) => o.id === id)?.label ?? id;
}

/**
 * 参数摘要：「半径=12 · 强度=1 · 选区」。
 *
 * `mask` 是编排开关不是算子参数，但显示出来有价值 —— 用户要能一眼看出
 * 这一步是整图还是局部，否则「为什么这一步只作用了一半」无从排查。
 */
function summarize(s: OpStep): string {
  const parts = Object.entries(s.params)
    .filter(([k]) => k !== "mask" && k !== "feather" && k !== "strength")
    .map(([k, v]) => `${k}=${v}`);
  if (s.params.mask !== false) parts.push("选区");
  return parts.join(" · ");
}

/** 按 schema 默认值初始化参数。 */
function applyDefaults(): void {
  const init: Record<string, number | string | boolean> = {};
  for (const p of currentOp.value?.params ?? []) init[p.name] = p.default;
  values.value = init;
}

watch(currentOp, applyDefaults, { immediate: true });

/** 参数显示：step<1 的按百分比，其余原值。 */
function displayValue(p: OpParam): string {
  const v = values.value[p.name];
  if (p.type === "number" && (p.step ?? 1) < 1) return `${Math.round(Number(v) * 100)}%`;
  return String(v);
}

/**
 * 把这一步追加进配方（末尾），然后让宿主重跑整条链。
 *
 * `mask` 混在 `params` 里一起存：它是这一步处理方式的一部分，必须随步骤
 * 进 sidecar。宿主发请求前会把它摘掉（Python 侧不认识这个键）。
 */
function addStep(): void {
  const op = currentOp.value;
  if (!op || !store.hasImage) return;
  const next = cloneRecipe(store.recipe);
  next.ops = [
    ...next.ops,
    { op: op.id, params: { ...values.value, mask: useMask.value }, enabled: true },
  ];
  void store.applyRecipe(next, true);
  emit("changed");
}

/** 改某一步的参数（拖动中不记撤销，commit 时记）。 */
function updateStep(i: number, patch: Partial<OpStep>, commit: boolean): void {
  const next = cloneRecipe(store.recipe);
  const s = next.ops[i];
  if (!s) return;
  next.ops[i] = { ...s, ...patch, params: { ...s.params, ...(patch.params ?? {}) } };
  void store.applyRecipe(next, commit);
  if (commit) emit("changed");
}

/** 删除某一步。 */
function removeStep(i: number): void {
  const next = cloneRecipe(store.recipe);
  next.ops = next.ops.filter((_, k) => k !== i);
  void store.applyRecipe(next, true);
  emit("changed");
}

/** 上下移动一步：顺序决定结果，调顺序必须能撤销。 */
function moveStep(i: number, dir: -1 | 1): void {
  const j = i + dir;
  const list = store.recipe.ops;
  if (j < 0 || j >= list.length) return;
  const next = cloneRecipe(store.recipe);
  const arr = [...next.ops];
  [arr[i], arr[j]] = [arr[j], arr[i]];
  next.ops = arr;
  void store.applyRecipe(next, true);
  emit("changed");
}

/** 清空全部步骤。 */
async function clearAll(): Promise<void> {
  if (!steps.value.length) return;
  const ok = await notify.confirm(
    "清空全部算子步骤？",
    `${steps.value.length} 步都会移除，可撤销。调色参数不受影响。`,
  );
  if (!ok) return;
  const next = cloneRecipe(store.recipe);
  next.ops = [];
  void store.applyRecipe(next, true);
  emit("changed");
}
</script>

<template>
  <section class="ops-panel">
    <header class="ops-head">
      <span class="ops-title">传统图像处理</span>
      <span class="ops-sub">输入 = 上方调色后的画面，每步都会记录</span>
    </header>

    <!-- 已记录的步骤：这是「记录在图像上」的可视化 -->
    <ol v-if="steps.length" class="ops-steps">
      <li
        v-for="(s, i) in steps"
        :key="`${i}-${s.op}`"
        class="ops-step"
        :class="{ off: !s.enabled }"
      >
        <div class="ops-step-main">
          <label class="ops-step-check" :title="s.enabled ? '点击跳过这一步' : '点击启用'">
            <input type="checkbox" :checked="s.enabled" @change="updateStep(i, { enabled: !s.enabled }, true)" />
          </label>
          <span class="ops-step-name">{{ labelOf(s.op) }}</span>
          <span class="ops-step-idx">{{ i + 1 }}</span>
        </div>
        <p class="ops-step-params">{{ summarize(s) || "无参数" }}</p>
        <div class="ops-step-acts">
          <button class="ops-mini" type="button" :disabled="i === 0" title="上移" @click="moveStep(i, -1)">↑</button>
          <button
            class="ops-mini"
            type="button"
            :disabled="i === steps.length - 1"
            title="下移"
            @click="moveStep(i, 1)"
          >
            ↓
          </button>
          <button class="ops-mini danger" type="button" title="删除这一步" @click="removeStep(i)">✕</button>
        </div>
      </li>
    </ol>
    <p v-else class="ops-hint">还没有算子步骤 —— 调好下面的参数后点「添加步骤」。</p>

    <p v-if="props.runError" class="ops-error">{{ props.runError }}</p>

    <!-- 新增一步的参数表单 -->
    <div class="ops-add">
      <label class="ops-row">
        <span>算子</span>
        <select v-model="opId" class="ops-select" :disabled="!store.hasImage || !props.ops.length">
          <option v-for="o in props.ops" :key="o.id" :value="o.id">{{ o.label }}</option>
        </select>
      </label>

      <template v-if="props.ops.length">
        <label v-for="p in currentOp?.params ?? []" :key="p.name" class="ops-row">
          <span>{{ p.label }}</span>
          <template v-if="p.type === 'enum'">
            <select v-model="values[p.name]" class="ops-select" :disabled="!store.hasImage">
              <option v-for="c in p.choices ?? []" :key="c.value" :value="c.value">{{ c.label }}</option>
            </select>
          </template>
          <template v-else>
            <input
              v-model.number="values[p.name]"
              type="range"
              :disabled="!store.hasImage"
              :min="p.min"
              :max="p.max"
              :step="p.step ?? 1"
            />
            <b>{{ displayValue(p) }}</b>
          </template>
        </label>

        <label v-if="currentOp?.supportsMask" class="ops-check">
          <input v-model="useMask" type="checkbox" :disabled="!props.maskActive" />
          <span>只处理选区内{{ props.maskActive ? "" : "（当前没有蒙版，先点「🎯 局部」画一个）" }}</span>
        </label>

        <div class="ops-actions">
          <button class="ops-btn primary" type="button" :disabled="!store.hasImage" @click="addStep">
            + 添加步骤
          </button>
          <button v-if="steps.length" class="ops-btn" type="button" @click="clearAll">清空全部</button>
        </div>
      </template>

      <p v-else-if="props.loading" class="ops-hint">算子列表加载中…</p>
      <p v-else-if="props.error" class="ops-error">
        {{ props.error }}
      </p>
      <p v-else class="ops-hint">本机处理服务未返回任何算子。</p>
    </div>

    <p v-if="props.running" class="ops-hint">正在重跑整条链…</p>
  </section>
</template>

<style scoped>
.ops-panel {
  padding: 12px 14px;
  border-bottom: 1px solid rgba(128, 128, 138, 0.18);
}

.ops-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 10px;
}

.ops-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text);
}

.ops-sub {
  font-size: 11px;
  color: var(--color-text-2);
}

/* ---------------- 已记录的步骤 ---------------- */

.ops-steps {
  margin: 0 0 12px;
  padding: 0;
  list-style: none;
}

.ops-step {
  position: relative;
  padding: 6px 8px;
  margin-bottom: 5px;
  border: 1px solid rgba(128, 128, 138, 0.2);
  border-radius: 7px;
  background: rgba(120, 170, 255, 0.07);
}

.ops-step.off {
  opacity: 0.45;
  background: transparent;
}

.ops-step-main {
  display: flex;
  align-items: center;
  gap: 6px;
}

.ops-step-check {
  display: flex;
  align-items: center;
}

.ops-step-name {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ops-step-idx {
  flex-shrink: 0;
  font-size: 10px;
  color: var(--color-text-2);
  font-variant-numeric: tabular-nums;
}

.ops-step-params {
  margin: 2px 0 4px 22px;
  font-size: 11px;
  color: var(--color-text-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ops-step-acts {
  display: flex;
  gap: 4px;
  margin-left: 22px;
}

.ops-mini {
  width: 22px;
  height: 20px;
  padding: 0;
  font-size: 11px;
  color: var(--color-text);
  background: rgba(120, 120, 130, 0.16);
  border: 1px solid rgba(128, 128, 138, 0.22);
  border-radius: 5px;
  cursor: pointer;
}

.ops-mini:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

.ops-mini.danger {
  color: #d4483b;
}

/* ---------------- 新增步骤 ---------------- */

.ops-add {
  padding-top: 8px;
  border-top: 1px dashed rgba(128, 128, 138, 0.22);
}

.ops-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  font-size: 12px;
  color: var(--color-text-2);
}

.ops-row > span {
  flex-shrink: 0;
  width: 62px;
}

.ops-row input[type="range"] {
  flex: 1;
  min-width: 0;
}

.ops-row b {
  flex-shrink: 0;
  min-width: 38px;
  text-align: right;
  font-variant-numeric: tabular-nums;
  color: var(--color-text);
}

/* select 必须显式重置 appearance。
   WebKitGTK（Linux 上的 Tauri 后端）默认给 <select> 画原生控件外观：
   文字用系统前景色、与页面主题无关，于是浅色主题下选项文字白字白底，
   整个下拉看起来「空的」—— 这正是算子列表看不见的原因。 */
.ops-select {
  flex: 1;
  min-width: 0;
  height: 26px;
  padding: 0 24px 0 6px;
  font-size: 12px;
  color: var(--color-text);
  background-color: rgba(120, 120, 130, 0.14);
  border: 1px solid rgba(128, 128, 138, 0.22);
  border-radius: 6px;
  -webkit-appearance: none;
  appearance: none;
  /* 自绘下拉箭头（appearance:none 后原生箭头会消失） */
  background-image: linear-gradient(45deg, transparent 50%, currentColor 50%),
    linear-gradient(135deg, currentColor 50%, transparent 50%);
  background-position: calc(100% - 14px) 11px, calc(100% - 9px) 11px;
  background-size: 5px 5px, 5px 5px;
  background-repeat: no-repeat;
}

/* 展开的选项列表由浏览器绘制，不吃组件样式，需显式配色 */
.ops-select option {
  color: #1f2733;
  background: #ffffff;
}

.ops-check {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 4px 0 10px;
  font-size: 11px;
  color: var(--color-text-2);
  cursor: pointer;
}

.ops-actions {
  display: flex;
  gap: 8px;
  margin-top: 10px;
}

.ops-btn {
  height: 28px;
  padding: 0 12px;
  font-size: 12px;
  color: var(--color-text);
  background: rgba(120, 120, 130, 0.14);
  border: 1px solid rgba(128, 128, 138, 0.22);
  border-radius: 7px;
  cursor: pointer;
}

.ops-btn.primary {
  background: rgba(120, 170, 255, 0.24);
  border-color: rgba(120, 170, 255, 0.42);
}

.ops-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.ops-hint,
.ops-error {
  margin: 8px 0 0;
  font-size: 11px;
  line-height: 1.5;
}

.ops-hint {
  color: var(--color-text-2);
}

.ops-error {
  color: #d4483b;
}
</style>