/**
 * 图像缩放 / 平移组合式（FEAT-181）
 * ===================================================================
 * 手感来自 `PhotoLightbox.vue`（Ctrl+滚轮 1.15 步长、放大后拖动平移、双击 2.5× 复原），
 * 抽成组合式后 PhotoLightbox 与工作台对比框共用同一份实现 —— 不再两处各写一套 ref。
 *
 * 分工：
 *   - `utils/zoomMath.ts`：夹取 / 以光标为锚点缩放的纯数学（可单测）；
 *   - 本文件：ref 状态、pointer capture、wheel 事件策略；
 *   - 组件：只决定「缩放 transform 挂在哪、何时重置」。
 *
 * 默认与图像预览一致：**必须按住 Ctrl** 才缩放，避免普通滚轮误触。
 */
import { computed, ref, type ComputedRef, type Ref } from "vue";

import { clampScale, wheelFactor, zoomAround, type ZoomPoint } from "../utils/zoomMath";

export interface UseImageZoomOptions {
  /** 最小倍率（图像预览允许缩到 20%，工作台以「适应窗口」为 100%） */
  min?: number;
  /** 最大倍率 */
  max?: number;
  /** wheel 是否必须按住 Ctrl（默认 true，与图像预览一致） */
  requireCtrl?: boolean;
  /** 滚轮是否以光标为锚点（默认 false，与图像预览一致） */
  anchorToCursor?: boolean;
  /** 双击/调用 toggleZoom 时的目标倍率 */
  toggleTarget?: number;
}

export interface ImageZoom {
  scale: Ref<number>;
  tx: Ref<number>;
  ty: Ref<number>;
  dragging: Ref<boolean>;
  transform: ComputedRef<string>;
  resetView: () => void;
  /** 直接设倍率；给了 anchor + center 时以该点为不动点 */
  applyScale: (next: number, anchor?: ZoomPoint, center?: ZoomPoint) => void;
  zoomBy: (factor: number, anchor?: ZoomPoint, center?: ZoomPoint) => void;
  /** wheel 处理器；组件负责用非 passive 方式挂载（需要 preventDefault 拦页面缩放） */
  onWheel: (e: WheelEvent, anchor?: ZoomPoint, center?: ZoomPoint) => void;
  /** 返回 false 表示当前倍率不需要平移（调用方可继续处理自己的拖拽） */
  onDragStart: (e: PointerEvent) => boolean;
  onDragMove: (e: PointerEvent) => void;
  onDragEnd: () => void;
  toggleZoom: () => void;
}

export function useImageZoom(opts: UseImageZoomOptions = {}): ImageZoom {
  const min = opts.min ?? 1;
  const max = opts.max ?? 8;
  const requireCtrl = opts.requireCtrl ?? true;
  const anchorToCursor = opts.anchorToCursor ?? false;
  const toggleTarget = opts.toggleTarget ?? 2.5;

  const scale = ref(1);
  const tx = ref(0);
  const ty = ref(0);
  const dragging = ref(false);
  let lastX = 0;
  let lastY = 0;

  /** 回到适应窗口：倍率 1、平移归零 */
  function resetView(): void {
    scale.value = 1;
    tx.value = 0;
    ty.value = 0;
    dragging.value = false;
  }

  function applyScale(next: number, anchor?: ZoomPoint, center?: ZoomPoint): void {
    const n = clampScale(next, min, max);
    if (anchor && center) {
      const p = zoomAround(scale.value, tx.value, ty.value, n, anchor, center);
      tx.value = p.tx;
      ty.value = p.ty;
    }
    scale.value = n;
    // 回到 1:1（图像预览里是 100%）时不保留平移，否则画面会偏在一边
    if (n === 1) {
      tx.value = 0;
      ty.value = 0;
    }
  }

  function zoomBy(factor: number, anchor?: ZoomPoint, center?: ZoomPoint): void {
    applyScale(scale.value * factor, anchor, center);
  }

  function onWheel(e: WheelEvent, anchor?: ZoomPoint, center?: ZoomPoint): void {
    if (requireCtrl && !e.ctrlKey) return;
    e.preventDefault();
    zoomBy(wheelFactor(e.deltaY), anchorToCursor ? anchor : undefined, anchorToCursor ? center : undefined);
  }

  function onDragStart(e: PointerEvent): boolean {
    // 只有放大后才需要平移；1× 及以下把拖拽留给调用方（如对比线）
    if (scale.value <= 1) return false;
    dragging.value = true;
    lastX = e.clientX;
    lastY = e.clientY;
    (e.currentTarget as HTMLElement | null)?.setPointerCapture?.(e.pointerId);
    return true;
  }

  function onDragMove(e: PointerEvent): void {
    if (!dragging.value) return;
    tx.value += e.clientX - lastX;
    ty.value += e.clientY - lastY;
    lastX = e.clientX;
    lastY = e.clientY;
  }

  function onDragEnd(): void {
    dragging.value = false;
  }

  function toggleZoom(): void {
    if (scale.value > 1) resetView();
    else applyScale(toggleTarget);
  }

  const transform = computed(() => `translate(${tx.value}px, ${ty.value}px) scale(${scale.value})`);

  return {
    scale,
    tx,
    ty,
    dragging,
    transform,
    resetView,
    applyScale,
    zoomBy,
    onWheel,
    onDragStart,
    onDragMove,
    onDragEnd,
    toggleZoom,
  };
}
