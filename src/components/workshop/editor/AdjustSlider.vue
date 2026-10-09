<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import type { EditParamSpec } from "../../../types/edit";

/**
 * Lightroom 式滑块行（FEAT-099）
 * ===================================================================
 * 外观与交互对齐 Lightroom 的「基本」面板：
 *
 * - **双极轨道**：中点是一条刻线，向左为减、向右为加；
 * - **数值在轨道中线上**：不需要眼睛在标签与数值之间来回跳；
 * - **双击归零**：这是 Lightroom 用户的肌肉记忆（也是最快的复位方式）；
 * - **按住 Shift 精细调节**：步长从 `step` 降到 `fineStep`；
 * - **滚轮调节**：`wheel` 上调下调，页面不跟着滚。
 *
 * ## 为什么不直接用 `<input type="range">`
 *
 * 原生 range 无法：
 * 1. 把数值显示在轨道中线上（只能固定放在端点外）；
 * 2. 双击归零到「不动点」而非 `min`（色温滑块归零到 6500 而不是 1500）；
 * 3. 双极轨道显示 —— 原生只有「已填充 / 未填充」两态，
 *    用户无法一眼看出「现在是 +0.3 还是 −0.3」；
 * 4. 在拖动时把 tooltip 留在轨道内（原生会跳到页面角落）。
 *
 * 因此这里用 `pointerdown/move/up` 自绘。
 *
 * ## 拖动期间不发历史记录
 *
 * `emit("begin")` / `emit("update:modelValue")`（连续）/ `emit("commit")`（一次）三段式：
 * 一次拖动 = 一步撤销，而不是 50 步。父组件（store）负责记账。
 */
const props = defineProps<{
  /** 滑块定义（来自后端 schema） */
  spec: EditParamSpec;
  /** 当前值（存储单位，非显示单位） */
  modelValue: number;
  /** 拖动中置 true（用于禁用动画/提示「渲染中」） */
  busy?: boolean;
}>();

const emit = defineEmits<{
  /** 拖动中的实时值（不记入撤销栈） */
  (e: "update:modelValue", v: number): void;
  /** 一次离散操作结束（记一步撤销） */
  (e: "commit", v: number): void;
  /** 交互开始（冻结历史记录） */
  (e: "begin"): void;
}>();

const root = ref<HTMLElement | null>(null);
const dragging = ref(false);
const hovering = ref(false);
/** 当前指针位置（0..1），拖动中用于画填充条 */
const dragT = ref(0.5);

/* ---------------- 数值 → 轨道位置 ---------------- */

/** 中点（不动点）在轨道上的位置。 */
const neutralT = computed(
  () => (props.spec.neutral - props.spec.min) / (props.spec.max - props.spec.min),
);

/** 当前值在轨道上的位置（0..1）。 */
const t = computed(() => {
  const s = props.spec;
  return clamp01((props.modelValue - s.min) / (s.max - s.min));
});

/** 实际用于绘制/拖动的位置（拖动中用指针位置，取值更跟手）。 */
const fillT = computed(() => (dragging.value ? dragT.value : t.value));

/** 是否双极（轨道从中点向两侧延伸）。单极参数（0..1）从中点 0 往右。 */
const bipolar = computed(() => props.spec.min < 0 && props.spec.neutral === 0);

/* ---------------- 数值格式化 ---------------- */

/**
 * 按 `spec.format` 显示数值。
 *
 * 色温特殊：6500 附近的变化肉眼几乎看不出（±200K 才勉强可辨），
 * 所以在 4000~9000 之间按 100 取整显示，其余按 50 取整 ——
 * 否则「6504」这种数字会给用户「精度很高」的错觉。
 */
const display = computed(() => {
  const v = props.modelValue;
  switch (props.spec.format) {
    case "ev": {
      const r = Math.round(v * 10) / 10;
      return `${r > 0 ? "+" : ""}${r.toFixed(1)}`;
    }
    case "kelvin": {
      const step = v >= 4000 && v <= 9000 ? 100 : 50;
      return `${Math.round(v / step) * step} K`;
    }
    case "percent": {
      return `${Math.round(v)}`;
    }
    case "percent100": {
      return `${Math.round(v * 100)}%`;
    }
    default:
      return fmtPlain(v);
  }
});

/** 归一化滑块按「×100 去掉小数」显示：0.25 → 25，−0.35 → −35。 */
function fmtPlain(v: number): string {
  const r = Math.round(v * 100);
  return `${r > 0 ? "+" : ""}${r}`;
}

/* ---------------- 拖动 ---------------- */

/** 从指针事件算出轨道位置对应的值。 */
function valueFromEvent(e: PointerEvent): number {
  const el = root.value;
  if (!el) return props.modelValue;
  const r = el.getBoundingClientRect();
  if (r.width <= 0) return props.modelValue;
  const pos = clamp01((e.clientX - r.left) / r.width);
  return round(props.spec.min + pos * (props.spec.max - props.spec.min), e.shiftKey);
}

/** 按步长量化。`Shift` 用精细步长。 */
function round(v: number, fine: boolean): number {
  const step = fine ? props.spec.fineStep : props.spec.step;
  const q = Math.round(v / step) * step;
  return clamp(q, props.spec.min, props.spec.max);
}

function onPointerDown(e: PointerEvent): void {
  if (e.button !== 0) return;
  dragging.value = true;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  emit("begin");
  const v = valueFromEvent(e);
  dragT.value = (v - props.spec.min) / (props.spec.max - props.spec.min);
  emit("update:modelValue", v);
  e.preventDefault();
}

function onPointerMove(e: PointerEvent): void {
  if (!dragging.value) return;
  const v = valueFromEvent(e);
  dragT.value = (v - props.spec.min) / (props.spec.max - props.spec.min);
  emit("update:modelValue", v);
}

function onPointerUp(e: PointerEvent): void {
  if (!dragging.value) return;
  dragging.value = false;
  try {
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
  } catch {
    /* 指针已释放 */
  }
  emit("commit", props.modelValue);
}

/** 双击归零到「不动点」—— Lightroom 肌肉记忆。 */
function onDoubleClick(): void {
  emit("begin");
  emit("update:modelValue", props.spec.neutral);
  emit("commit", props.spec.neutral);
}

/** 滚轮调节。`preventDefault` 阻止页面滚动。 */
function onWheel(e: WheelEvent): void {
  e.preventDefault();
  const dir = e.deltaY > 0 ? -1 : 1;
  const step = (e.shiftKey ? props.spec.fineStep : props.spec.step) * dir;
  const v = round(props.modelValue + step, e.shiftKey);
  emit("begin");
  emit("update:modelValue", v);
  emit("commit", v);
}

/** 键盘：←/→ 步进，Shift 微调，Home/End 跳端点，Backspace/Delete 归零。 */
function onKeydown(e: KeyboardEvent): void {
  const s = props.spec;
  let v: number | null = null;
  const step = e.shiftKey ? s.fineStep : s.step;
  switch (e.key) {
    case "ArrowLeft":
    case "ArrowDown":
      v = round(props.modelValue - step, e.shiftKey);
      break;
    case "ArrowRight":
    case "ArrowUp":
      v = round(props.modelValue + step, e.shiftKey);
      break;
    case "PageDown":
      v = round(props.modelValue - step * 5, false);
      break;
    case "PageUp":
      v = round(props.modelValue + step * 5, false);
      break;
    case "Home":
      v = s.min;
      break;
    case "End":
      v = s.max;
      break;
    case "Backspace":
    case "Delete":
      v = s.neutral;
      break;
    default:
      return;
  }
  e.preventDefault();
  emit("begin");
  emit("update:modelValue", v);
  emit("commit", v);
}

onBeforeUnmount(() => {
  // 组件在拖动中被卸载（切页/关弹窗）：必须提交，否则这次拖动不会进历史
  if (dragging.value) emit("commit", props.modelValue);
});

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}
function clamp01(v: number): number {
  return clamp(v, 0, 1);
}
</script>

<template>
  <div
    ref="root"
    class="adj-row"
    :class="{ 'is-bipolar': bipolar, 'is-dragging': dragging, 'is-busy': busy }"
    tabindex="0"
    role="slider"
    :aria-label="spec.label"
    :aria-valuemin="spec.min"
    :aria-valuemax="spec.max"
    :aria-valuenow="modelValue"
    :aria-valuetext="display"
    :title="spec.help"
    @pointerdown="onPointerDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerUp"
    @pointercancel="onPointerUp"
    @dblclick="onDoubleClick"
    @wheel="onWheel"
    @keydown="onKeydown"
    @mouseenter="hovering = true"
    @mouseleave="hovering = false"
  >
    <span class="adj-label">{{ spec.label }}</span>

    <div class="adj-track">
      <!-- 轨道底 -->
      <div class="adj-track-bg"></div>

      <!-- 单极：从中点向右填充；双极：从中点向当前值方向填充 -->
      <div
        class="adj-track-fill"
        :class="bipolar ? 'bipolar' : 'unipolar'"
        :style="
          bipolar
            ? {
                left: `${Math.min(neutralT, fillT) * 100}%`,
                width: `${Math.abs(fillT - neutralT) * 100}%`,
              }
            : { left: `${neutralT * 100}%`, width: `${Math.max(0, fillT - neutralT) * 100}%` }
        "
      ></div>

      <!-- 中点刻线（双极才有意义） -->
      <div v-if="bipolar" class="adj-track-neutral" :style="{ left: `${neutralT * 100}%` }"></div>

      <!-- 滑块手柄 -->
      <div class="adj-thumb" :style="{ left: `${fillT * 100}%` }">
        <span class="adj-thumb-value">{{ display }}</span>
      </div>
    </div>

    <!-- 归零提示：值不等于不动点时给一个可点的小圆点 -->
    <button
      v-if="modelValue !== spec.neutral"
      class="adj-reset"
      type="button"
      :title="`复位 ${spec.label}`"
      @click.stop="onDoubleClick"
    >
      ●
    </button>
    <span v-else class="adj-reset-placeholder"></span>
  </div>
</template>

<style scoped>
.adj-row {
  display: grid;
  grid-template-columns: 62px 1fr 18px;
  align-items: center;
  gap: 8px;
  padding: 3px 0;
  border-radius: 6px;
  outline: none;
}
.adj-row:focus-visible {
  box-shadow: 0 0 0 2px var(--color-primary, rgba(120, 170, 255, 0.6));
}

.adj-label {
  font-size: 12px;
  color: var(--pm-label, var(--color-text-2));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* ---------------- 轨道 ---------------- */

.adj-track {
  position: relative;
  height: 22px;
  cursor: ew-resize;
  touch-action: none;
}

.adj-track-bg {
  position: absolute;
  left: 0;
  right: 0;
  top: 9px;
  height: 4px;
  border-radius: 2px;
  background: rgba(128, 128, 138, 0.28);
}

.adj-track-fill {
  position: absolute;
  top: 9px;
  height: 4px;
  border-radius: 2px;
  background: var(--color-primary, rgba(140, 175, 235, 0.95));
  pointer-events: none;
  transition: width 0.05s linear, left 0.05s linear;
}
.adj-row.is-dragging .adj-track-fill {
  transition: none;
}

.adj-track-neutral {
  position: absolute;
  top: 5px;
  width: 1px;
  height: 12px;
  background: rgba(128, 128, 138, 0.55);
  transform: translateX(-0.5px);
  pointer-events: none;
}

.adj-thumb {
  position: absolute;
  top: 0;
  width: 14px;
  height: 22px;
  margin-left: -7px;
  border-radius: 4px;
  background: linear-gradient(180deg, #f2f4f8, #c9cfda);
  border: 1px solid rgba(0, 0, 0, 0.28);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.3);
  pointer-events: none;
  transition: left 0.05s linear;
}
.adj-row.is-dragging .adj-thumb {
  transition: none;
}
.adj-row.is-hover .adj-thumb,
.adj-row:hover .adj-thumb {
  background: linear-gradient(180deg, #ffffff, #d8dee9);
}

/* 数值显示在手柄内部（垂直居中），拖动时始终跟随 */
.adj-thumb-value {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  font-size: 10px;
  line-height: 1;
  color: #1d2430;
  font-variant-numeric: tabular-nums;
  pointer-events: none;
  white-space: nowrap;
}

.adj-reset {
  width: 18px;
  height: 18px;
  padding: 0;
  font-size: 8px;
  line-height: 1;
  color: var(--pm-hint, var(--color-text-2));
  background: transparent;
  border: none;
  cursor: pointer;
  opacity: 0.65;
}
.adj-reset:hover {
  opacity: 1;
  color: var(--color-danger, #e0555a);
}
.adj-reset-placeholder {
  width: 18px;
}
</style>