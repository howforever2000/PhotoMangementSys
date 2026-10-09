<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import type { CropSpec } from "../../../types/edit";

/**
 * 裁剪 / 拉直接取层（FEAT-099）
 * ===================================================================
 * 覆盖在预览图上的交互层，提供 Lightroom 的四类取景方式：
 *
 * - **自由**：拖四个边角与四条边；
 * - **比例**：原图 / 1:1 / 4:3 / 3:2 / 16:9 / 9:16（拖动时锁定比例）；
 * - **拉直**：拖地平线线，图像实时按角度旋转（不裁剪）；
 * - **旋转 90°**：菜单项（配方里用 `angle` 表达的是 ±45° 的拉直，
 *   整转需要额外表达 —— 见下方 `quarterTurn` 的说明）。
 *
 * ## 坐标系
 *
 * 层的尺寸 = 预览图像在屏幕上的显示矩形（由父组件传入的 `frame` 给出）。
 * 裁剪框用**归一化 0..1** 存储，绘制时乘以 `frame` 的宽高 ——
 * 这样窗口缩放时裁剪框不动，且与后端的存储格式一致（无需每次缩放做换算）。
 *
 * ## 90° 整转的存储
 *
 * 后端 `CropSpec` 只有 `angle`（±45° 拉直）。整转 90/180/270° 不在它表达范围内。
 * 折中方案：整转只影响**显示与导出的取景方向**，用「把裁剪框反过来」
 * 的方式在导出时体现 —— 这里只把 UI 上的旋转按钮做成「切换画面方向的预览」，
 * 导出仍按 `angle` 走。若要真正的 90° 导出，需要后端加 `quarter` 字段。
 * 因此本组件**不提供** 90° 按钮（提供半吊子功能比不提供更糟）。
 */
const props = defineProps<{
  crop: CropSpec;
  /** 预览图像在屏幕上的显示矩形（CSS 像素，左上角坐标） */
  frame: { x: number; y: number; w: number; h: number };
  /** 原图宽高（显示「原图 6000×4000」用） */
  srcWidth: number;
  srcHeight: number;
}>();

const emit = defineEmits<{
  (e: "begin"): void;
  (e: "update:crop", c: CropSpec): void;
  (e: "commit:crop", c: CropSpec): void;
}>();

type Mode = "free" | "ratio" | "straighten";
const mode = ref<Mode>("free");
type RatioKey = "original" | "1:1" | "4:3" | "3:2" | "16:9" | "9:16";
const ratio = ref<RatioKey>("original");
type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "move" | "angle";
const dragHandle = ref<Handle | null>(null);

const RATIOS: Record<RatioKey, number> = {
  original: 0, // 0 = 不锁定（按原图）
  "1:1": 1,
  "4:3": 4 / 3,
  "3:2": 3 / 2,
  "16:9": 16 / 9,
  "9:16": 9 / 16,
};

/** 当前锁定的宽高比（`null` = 自由）。 */
const lockedRatio = computed(() => (RATIOS[ratio.value] === 0 ? null : RATIOS[ratio.value]));

/** 裁剪框在屏幕上的矩形（CSS 像素，相对 viewport）。 */
const box = computed(() => {
  const f = props.frame;
  return {
    left: f.x + props.crop.x * f.w,
    top: f.y + props.crop.y * f.h,
    width: Math.max(1, props.crop.w * f.w),
    height: Math.max(1, props.crop.h * f.h),
  };
});

/** 裁剪后的输出尺寸（显示给用户）。 */
const outSize = computed(() => {
  const sw = props.srcWidth || 1;
  const sh = props.srcHeight || 1;
  return {
    w: Math.round(sw * props.crop.w),
    h: Math.round(sh * props.crop.h),
  };
});

/* ---------------- 几何换算 ---------------- */

function pxToNorm(px: number, py: number): { x: number; y: number } {
  const f = props.frame;
  return {
    x: clamp01((px - f.x) / Math.max(1, f.w)),
    y: clamp01((py - f.y) / Math.max(1, f.h)),
  };
}

function clamp01(v: number): number {
  return v < 0 ? 0 : v > 1 ? 1 : v;
}

/** 拖动起点（归一化）与当前裁剪框快照。 */
let startPointer = { x: 0, y: 0 };
let startCrop: CropSpec = { ...props.crop };

function onHandleDown(h: Handle, e: PointerEvent): void {
  if (mode.value === "straighten" && h !== "angle") return;
  dragHandle.value = h;
  startPointer = pxToNorm(e.clientX, e.clientY);
  startCrop = { ...props.crop };
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  emit("begin");
  e.preventDefault();
  e.stopPropagation();
}

/** 拉直地平线的拖动。 */
function onAngleDown(e: PointerEvent): void {
  dragHandle.value = "angle";
  const el = e.currentTarget as HTMLElement;
  el.setPointerCapture(e.pointerId);
  startPointer = pxToNorm(e.clientX, e.clientY);
  startCrop = { ...props.crop };
  emit("begin");
  e.preventDefault();
  e.stopPropagation();
}

function onPointerMove(e: PointerEvent): void {
  const h = dragHandle.value;
  if (!h) return;
  const p = pxToNorm(e.clientX, e.clientY);
  const f = props.frame;
  // 屏幕像素 → 归一化的换算系数（比例锁定时需要它按像素而非比例计算）
  const kx = Math.max(1e-6, f.w / Math.max(1, props.srcWidth));
  const ky = Math.max(1e-6, f.h / Math.max(1, props.srcHeight));
  void kx;
  void ky;

  if (h === "angle") {
    // 角度 = 拖动点相对图像中心的连线与水平线的夹角
    const cx = 0.5;
    const cy = 0.5;
    const ang = (Math.atan2(p.y - cy, p.x - cx) * 180) / Math.PI;
    // 直线方向 ±90° 等价，取 [-45, 45] 内的那个
    let a = ang - 90;
    while (a > 45) a -= 180;
    while (a < -45) a += 180;
    emit("update:crop", { ...startCrop, angle: round1(a), enabled: true });
    return;
  }

  const dx = p.x - startPointer.x;
  const dy = p.y - startPointer.y;
  const next = applyHandle(h, startCrop, dx, dy);
  emit("update:crop", next);
}

function onPointerUp(e: PointerEvent): void {
  const h = dragHandle.value;
  if (!h) return;
  dragHandle.value = null;
  try {
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
  } catch {
    /* 已释放 */
  }
  emit("commit:crop", props.crop);
}

/** 按手柄计算新的裁剪框。 */
function applyHandle(h: Handle, c: CropSpec, dx: number, dy: number): CropSpec {
  const ar = lockedRatio.value;
  let { x, y, w, h: hh } = c;

  if (h === "move") {
    x = clamp01(x + dx);
    y = clamp01(y + dy);
    return { ...c, x: Math.min(x, 1 - w), y: Math.min(y, 1 - hh), enabled: true };
  }

  if (h.includes("w")) {
    const nx = clamp01(x + dx);
    w = x + w - nx;
    x = nx;
  }
  if (h.includes("e")) {
    w = clamp(1 - x, 0.04, 1 - x);
  }
  if (h.includes("n")) {
    const ny = clamp01(y + dy);
    hh = y + hh - ny;
    y = ny;
  }
  if (h.includes("s")) {
    hh = clamp(1 - y, 0.04, 1 - y);
  }

  // 保证最小尺寸
  w = Math.max(w, 0.04);
  hh = Math.max(hh, 0.04);
  // 防止越界
  if (x + w > 1) {
    if (h.includes("w")) x = 1 - w;
    else w = 1 - x;
  }
  if (y + hh > 1) {
    if (h.includes("n")) y = 1 - hh;
    else hh = 1 - y;
  }

  // 比例锁定：以「角点手柄」为准，另一个维度跟着算
  if (ar !== null) {
    const isCorner = h.length === 2;
    if (isCorner) {
      // 用当前拖动方向更大的那个维度作为基准
      const useW = h === "nw" || h === "ne" || h === "sw" || h === "se";
      if (useW) hh = w / ar;
      else w = hh * ar;
    }
    // 边中点手柄：只动一个维度就按比例调整另一个
    if (!isCorner) {
      if (h === "n" || h === "s") w = hh * ar;
      else hh = w / ar;
    }
    if (x + w > 1) {
      w = 1 - x;
      hh = w / ar;
    }
    if (y + hh > 1) {
      hh = 1 - y;
      w = hh * ar;
    }
  }

  return { ...c, x: clamp01(x), y: clamp01(y), w: clamp01(w), h: clamp01(hh), enabled: true };
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

function round1(v: number): number {
  return Math.round(v * 10) / 10;
}

/* ---------------- 比例切换 ---------------- */

function setRatio(k: RatioKey): void {
  ratio.value = k;
  if (k === "original") return;
  const ar = RATIOS[k];
  const f = props.frame;
  // 画布本身的宽高比
  const imgAr = f.w / Math.max(1e-6, f.h);
  let w = props.crop.w;
  let hh = props.crop.h;
  if (ar > imgAr) {
    w = props.crop.h * ar;
  } else {
    hh = props.crop.w / ar;
  }
  w = Math.min(w, 1);
  hh = Math.min(hh, 1);
  emit("begin");
  emit(
    "commit:crop",
    {
      ...props.crop,
      w,
      h: hh,
      x: Math.min(props.crop.x, 1 - w),
      y: Math.min(props.crop.y, 1 - hh),
      enabled: true,
    },
  );
}

/** 恢复整幅（清除裁剪，但保留拉直角度）。 */
function resetFrame(): void {
  emit("begin");
  emit("commit:crop", { ...props.crop, x: 0, y: 0, w: 1, h: 1, angle: 0, enabled: false });
}

/** 居中铺满（比例切换后把裁剪框移到中央）。 */
function centerFrame(): void {
  emit("begin");
  emit(
    "commit:crop",
    { ...props.crop, x: (1 - props.crop.w) / 2, y: (1 - props.crop.h) / 2, enabled: true },
  );
}

const modeOptions: { key: Mode; label: string; hint: string }[] = [
  { key: "free", label: "自由", hint: "拖动边角自由裁剪" },
  { key: "ratio", label: "比例", hint: "按预设比例裁剪" },
  { key: "straighten", label: "拉直", hint: "拖动地平线校正透视" },
];

const ratioOptions: { key: RatioKey; label: string }[] = [
  { key: "original", label: "原图" },
  { key: "1:1", label: "1:1" },
  { key: "4:3", label: "4:3" },
  { key: "3:2", label: "3:2" },
  { key: "16:9", label: "16:9" },
  { key: "9:16", label: "9:16" },
];

/** 直方图/暗角遮罩的样式（裁剪框外压暗）。 */
const shadeStyle = computed(() => ({
  position: "absolute" as const,
  pointerEvents: "none" as const,
  inset: "0",
}));

function onUnmount(): void {
  dragHandle.value = null;
}
onBeforeUnmount(onUnmount);
</script>

<template>
  <div class="crop-overlay" :class="`mode-${mode}`">
    <!-- 裁剪框外的压暗（四块矩形围出中间的空洞） -->
    <template v-if="mode !== 'straighten'">
      <div
        :style="{
          ...shadeStyle,
          left: 0,
          top: 0,
          right: 0,
          height: `${box.top - frame.y}px`,
        }"
      ></div>
      <div
        :style="{
          ...shadeStyle,
          left: 0,
          top: `${box.top + box.height}px`,
          right: 0,
          bottom: 0,
        }"
      ></div>
      <div
        :style="{
          ...shadeStyle,
          left: 0,
          top: `${box.top}px`,
          width: `${box.left - frame.x}px`,
          height: `${box.height}px`,
        }"
      ></div>
      <div
        :style="{
          ...shadeStyle,
          left: `${box.left + box.width}px`,
          top: `${box.top}px`,
          right: 0,
          height: `${box.height}px`,
        }"
      ></div>
    </template>

    <!-- 拉直线 -->
    <div
      v-if="mode === 'straighten'"
      class="angle-line"
      :style="{
        left: `${frame.x}px`,
        top: `${frame.y}px`,
        width: `${frame.w}px`,
        height: `${frame.h}px`,
        transform: `rotate(${crop.angle}deg)`,
      }"
      @pointerdown="onAngleDown"
      @pointermove="onPointerMove"
      @pointerup="onPointerUp"
    >
      <span class="angle-grip"></span>
    </div>

    <!-- 裁剪框 -->
    <div
      v-if="mode !== 'straighten'"
      class="crop-box"
      :class="{ moving: dragHandle === 'move' }"
      :style="{
        left: `${box.left}px`,
        top: `${box.top}px`,
        width: `${box.width}px`,
        height: `${box.height}px`,
      }"
      @pointerdown="onHandleDown('move', $event)"
      @pointermove="onPointerMove"
      @pointerup="onPointerUp"
    >
      <!-- 三分线 -->
      <div class="thirds">
        <span class="tv" style="left: 33.333%"></span>
        <span class="tv" style="left: 66.667%"></span>
        <span class="th" style="top: 33.333%"></span>
        <span class="th" style="top: 66.667%"></span>
      </div>

      <!-- 八个手柄 -->
      <span
        v-for="h in (['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'] as Handle[])"
        :key="h"
        class="handle"
        :class="`h-${h}`"
        @pointerdown="onHandleDown(h, $event)"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
      ></span>

      <!-- 输出尺寸 -->
      <span class="crop-size">{{ outSize.w }} × {{ outSize.h }}</span>
    </div>

    <!-- 工具条 -->
    <div class="crop-tools" @pointerdown.stop>
      <div class="ct-row">
        <button
          v-for="m in modeOptions"
          :key="m.key"
          type="button"
          class="ct-btn"
          :class="{ active: mode === m.key }"
          :title="m.hint"
          @click="mode = m.key"
        >
          {{ m.label }}
        </button>
      </div>

      <div v-if="mode === 'ratio'" class="ct-row">
        <button
          v-for="r in ratioOptions"
          :key="r.key"
          type="button"
          class="ct-btn sm"
          :class="{ active: ratio === r.key }"
          @click="setRatio(r.key)"
        >
          {{ r.label }}
        </button>
      </div>

      <div v-if="mode === 'straighten'" class="ct-row">
        <span class="ct-label">角度</span>
        <input
          class="ct-range"
          type="range"
          min="-45"
          max="45"
          step="0.1"
          :value="crop.angle"
          @input="emit('begin')"
          @change="
            emit('commit:crop', {
              ...crop,
              angle: Number(($event.target as HTMLInputElement).value),
              enabled: true,
            })
          "
        />
        <span class="ct-num">{{ crop.angle.toFixed(1) }}°</span>
      </div>

      <div class="ct-row">
        <button type="button" class="ct-btn sm" title="裁剪框居中" @click="centerFrame">居中</button>
        <button type="button" class="ct-btn sm" title="恢复整幅" @click="resetFrame">恢复整幅</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.crop-overlay {
  position: absolute;
  inset: 0;
  pointer-events: none;
}
.crop-overlay > div {
  background: rgba(0, 0, 0, 0.52);
}

.crop-box {
  position: absolute;
  border: 1px solid rgba(255, 255, 255, 0.92);
  box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.45);
  pointer-events: auto;
  touch-action: none;
  cursor: move;
}

/* 三分线 */
.thirds {
  position: absolute;
  inset: 0;
  pointer-events: none;
}
.tv,
.th {
  position: absolute;
  background: rgba(255, 255, 255, 0.35);
}
.tv {
  top: 0;
  bottom: 0;
  width: 1px;
}
.th {
  left: 0;
  right: 0;
  height: 1px;
}

/* 手柄：角上 18×18，边中点 22×8 */
.handle {
  position: absolute;
  background: transparent;
  pointer-events: auto;
  touch-action: none;
}
.h-nw,
.h-ne,
.h-se,
.h-sw {
  width: 20px;
  height: 20px;
}
.h-n {
  left: 50%;
  top: 0;
  width: 26px;
  height: 8px;
  transform: translateX(-50%);
  cursor: ns-resize;
}
.h-s {
  left: 50%;
  bottom: 0;
  width: 26px;
  height: 8px;
  transform: translateX(-50%);
  cursor: ns-resize;
}
.h-e {
  top: 50%;
  right: 0;
  width: 8px;
  height: 26px;
  transform: translateY(-50%);
  cursor: ew-resize;
}
.h-w {
  top: 50%;
  left: 0;
  width: 8px;
  height: 26px;
  transform: translateY(-50%);
  cursor: ew-resize;
}
.h-nw,
.h-se {
  cursor: nwse-resize;
}
.h-ne,
.h-sw {
  cursor: nesw-resize;
}
.handle::after {
  content: "";
  position: absolute;
  background: #fff;
  box-shadow: 0 0 2px rgba(0, 0, 0, 0.6);
}
.h-nw::after,
.h-ne::after {
  left: 50%;
  top: 0;
  width: 2px;
  height: 14px;
  transform: translateX(-50%);
}
.h-sw::after,
.h-se::after {
  left: 50%;
  bottom: 0;
  width: 2px;
  height: 14px;
  transform: translateX(-50%);
}
.h-nw::before,
.h-sw::before {
  content: "";
  position: absolute;
  top: 50%;
  left: 0;
  width: 14px;
  height: 2px;
  transform: translateY(-50%);
}
.h-ne::before,
.h-se::before {
  content: "";
  position: absolute;
  top: 50%;
  right: 0;
  width: 14px;
  height: 2px;
  transform: translateY(-50%);
}
.h-n::after,
.h-s::after {
  left: 50%;
  top: 50%;
  width: 18px;
  height: 2px;
  transform: translate(-50%, -50%);
}
.h-e::after,
.h-w::after {
  top: 50%;
  left: 50%;
  width: 2px;
  height: 18px;
  transform: translate(-50%, -50%);
}

.crop-size {
  position: absolute;
  left: 50%;
  bottom: -22px;
  transform: translateX(-50%);
  padding: 1px 7px;
  font-size: 11px;
  color: #fff;
  background: rgba(0, 0, 0, 0.6);
  border-radius: 9px;
  white-space: nowrap;
  pointer-events: none;
}

/* 拉直线 */
.angle-line {
  position: absolute;
  pointer-events: auto;
  cursor: grab;
  touch-action: none;
  transform-origin: center;
}
.angle-line::before {
  content: "";
  position: absolute;
  left: 0;
  right: 0;
  top: 50%;
  height: 1px;
  background: rgba(255, 214, 120, 0.95);
  box-shadow: 0 0 3px rgba(0, 0, 0, 0.6);
}
.angle-grip {
  position: absolute;
  left: 50%;
  top: 50%;
  width: 16px;
  height: 16px;
  margin: -8px 0 0 -8px;
  border-radius: 50%;
  background: rgba(255, 214, 120, 0.95);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.6);
}

/* 工具条 */
.crop-tools {
  position: absolute;
  left: 50%;
  bottom: 12px;
  transform: translateX(-50%);
  padding: 6px 8px;
  border-radius: 10px;
  background: rgba(18, 20, 26, 0.82);
  backdrop-filter: blur(6px);
  pointer-events: auto;
  display: flex;
  flex-direction: column;
  gap: 5px;
}
.ct-row {
  display: flex;
  align-items: center;
  gap: 5px;
  justify-content: center;
}
.ct-btn {
  padding: 3px 10px;
  font-size: 11px;
  color: #e6e9f0;
  background: rgba(255, 255, 255, 0.08);
  border: 1px solid rgba(255, 255, 255, 0.14);
  border-radius: 6px;
  cursor: pointer;
}
.ct-btn.sm {
  padding: 2px 8px;
  font-size: 10px;
}
.ct-btn:hover {
  background: rgba(255, 255, 255, 0.18);
}
.ct-btn.active {
  background: rgba(255, 214, 120, 0.9);
  color: #20242c;
  border-color: rgba(255, 214, 120, 0.9);
}
.ct-label {
  font-size: 11px;
  color: #c7ccd8;
}
.ct-range {
  width: 120px;
}
.ct-num {
  font-size: 11px;
  color: #e6e9f0;
  font-variant-numeric: tabular-nums;
  min-width: 38px;
}
</style>