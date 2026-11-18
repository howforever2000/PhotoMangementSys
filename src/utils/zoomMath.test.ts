/**
 * `utils/zoomMath.ts` 单元测试（FEAT-181）
 *
 * 钉死的语义：
 *   1. 倍率夹取，NaN 不进入 transform；
 *   2. 以光标为锚点缩放时，光标下的图片局部坐标不动；
 *   3. 回到 1× 由组件层清平移（这里只验纯数学）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import { clampScale, wheelFactor, zoomAround } from "./zoomMath.ts";

test("倍率夹取在 [min,max]，NaN 回落到 min", () => {
  assert.equal(clampScale(0.5), 1);
  assert.equal(clampScale(20), 8);
  assert.equal(clampScale(2.5), 2.5);
  assert.equal(clampScale(Number.NaN), 1);
  assert.equal(clampScale(0.1, 0.2, 8), 0.2);
});

test("滚轮方向：向上放大、向下缩小", () => {
  assert.ok(wheelFactor(-100) > 1);
  assert.ok(wheelFactor(100) < 1);
});

test("以光标为锚点缩放，光标下的点保持不动", () => {
  const center = { x: 500, y: 400 };
  const anchor = { x: 560, y: 430 }; // 相对中心 (60, 30)
  const scale = 2;
  const tx = 20;
  const ty = -10;

  const next = 4;
  const p = zoomAround(scale, tx, ty, next, anchor, center);

  // 缩放前：局部坐标 u = (anchor - center - t) / s
  const ux = (anchor.x - center.x - tx) / scale;
  const uy = (anchor.y - center.y - ty) / scale;
  // 缩放后：screen = center + t' + s' × u，必须回到 anchor
  assert.ok(Math.abs(center.x + p.tx + next * ux - anchor.x) < 1e-9);
  assert.ok(Math.abs(center.y + p.ty + next * uy - anchor.y) < 1e-9);
});

test("从 1× 原点放大时，向右上的锚点会把画面往左上推", () => {
  const center = { x: 0, y: 0 };
  const p = zoomAround(1, 0, 0, 2, { x: 50, y: 50 }, center);
  assert.equal(p.tx, -50);
  assert.equal(p.ty, -50);
});
