/**
 * 图像缩放 / 平移的纯数学（FEAT-181）
 * ===================================================================
 * 从 `PhotoLightbox.vue` 的「Ctrl+滚轮缩放 + 拖动平移」抽出来，工作台对比框复用同一套手感。
 * 这里只放可单测的纯函数，不碰 Vue / DOM：组件层负责 ref 与 pointer capture。
 */

export interface ZoomPoint {
  x: number;
  y: number;
}

export const ZOOM_MIN = 1;
export const ZOOM_MAX = 8;
/** 每格滚轮的倍率（与 PhotoLightbox 一致：1.15） */
export const ZOOM_WHEEL_FACTOR = 1.15;

/** 夹取倍率；非法值回落 min（NaN 不能进 transform，否则整块画面消失） */
export function clampScale(v: number, min = ZOOM_MIN, max = ZOOM_MAX): number {
  if (!Number.isFinite(v)) return min;
  return Math.min(max, Math.max(min, v));
}

/**
 * 以视口坐标 `anchor` 为不动点缩放，返回新的平移量。
 *
 * 前提：被缩放元素以 `center` 为几何中心（transform-origin: center），
 * `tx/ty` 是屏幕像素平移，缩放不移动元素中心。
 * 推导：缩放前后光标下的图片局部坐标不变 →
 *   anchor = center + t' + s' × u，其中 u = (anchor - center - t) / s。
 */
export function zoomAround(
  scale: number,
  tx: number,
  ty: number,
  next: number,
  anchor: ZoomPoint,
  center: ZoomPoint,
): { tx: number; ty: number } {
  const s = scale > 0 ? scale : 1;
  const ux = (anchor.x - center.x - tx) / s;
  const uy = (anchor.y - center.y - ty) / s;
  return {
    tx: anchor.x - center.x - next * ux,
    ty: anchor.y - center.y - next * uy,
  };
}

/** 滚轮方向 → 倍率（向上放大） */
export function wheelFactor(deltaY: number): number {
  return deltaY < 0 ? ZOOM_WHEEL_FACTOR : 1 / ZOOM_WHEEL_FACTOR;
}
