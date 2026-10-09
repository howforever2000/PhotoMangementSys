/**
 * 调整配方的结构与读写（FEAT-099）
 *
 * 覆盖的是「与 Rust 侧 `recipe.rs` 的契约」：
 * - 缺字段必须补默认值（老 sidecar / 旧版本后端）
 * - 未知/非法字段必须被丢弃而不是让 UI 崩
 * - `readParam`/`writeParam` 的路径语义
 * - 深拷贝必须真的深（撤销栈存的是快照，共享引用会让撤销失效）
 *
 * 运行：node --experimental-strip-types --test src/types/edit.test.ts
 */
import test from "node:test";
import assert from "node:assert/strict";

import {
  cloneRecipe,
  emptyRecipe,
  isPristineRecipe,
  normalizeRecipe,
  readParam,
  writeParam,
  type AdjustmentRecipe,
} from "./edit.ts";

test("空配方与 Rust 侧 default() 逐项对应", () => {
  const r = emptyRecipe();
  assert.equal(r.version, 1);
  assert.equal(r.basic.exposure, 0);
  assert.equal(r.basic.temperature, 6500, "色温默认必须是 D65 锚点");
  assert.equal(r.color.saturation, 0);
  assert.equal(r.sharpen.amount, 0, "锐化默认关闭（JPEG 已被相机锐化过）");
  assert.equal(r.sharpen.masking, 0.25);
  assert.equal(r.crop.w, 1);
  assert.equal(r.crop.enabled, false);
  assert.deepEqual(r.curveRgb.points, []);
});

test("空配方是 pristine 的", () => {
  assert.equal(isPristineRecipe(emptyRecipe()), true);
});

test("normalizeRecipe 对空输入返回空配方", () => {
  assert.deepEqual(normalizeRecipe(null), emptyRecipe());
  assert.deepEqual(normalizeRecipe(undefined), emptyRecipe());
  assert.deepEqual(normalizeRecipe("garbage"), emptyRecipe());
  assert.deepEqual(normalizeRecipe(42), emptyRecipe());
});

test("normalizeRecipe 补齐缺失字段", () => {
  const r = normalizeRecipe({ basic: { exposure: 1.5 } });
  assert.equal(r.basic.exposure, 1.5);
  assert.equal(r.basic.temperature, 6500);
  assert.equal(r.basic.tint, 0);
  assert.equal(r.sharpen.radius, 0.5);
  assert.equal(r.crop.h, 1);
  assert.deepEqual(r.curveLuma.points, []);
});

test("normalizeRecipe 丢弃非数值字段", () => {
  const r = normalizeRecipe({
    basic: { exposure: "1.5", contrast: NaN, highlights: Infinity, tint: -20 },
    color: { saturation: null },
  });
  assert.equal(r.basic.exposure, 0, "字符串不是数字，必须回落默认");
  assert.equal(r.basic.contrast, 0, "NaN 必须回落默认");
  assert.equal(r.basic.highlights, 0, "Infinity 必须回落默认");
  assert.equal(r.basic.tint, -20);
  assert.equal(r.color.saturation, 0);
});

test("normalizeRecipe 兼容 snake_case 字段名", () => {
  const r = normalizeRecipe({
    curve_rgb: { points: [{ x: 0.2, y: 0.3 }] },
    curve_luma: { points: [{ x: 0.4, y: 0.5 }] },
  });
  assert.equal(r.curveRgb.points.length, 1);
  assert.equal(r.curveLuma.points.length, 1);
});

test("normalizeRecipe 把曲线控制点按 x 排序并过滤越界", () => {
  const r = normalizeRecipe({
    curveRgb: {
      points: [
        { x: 0.8, y: 0.9 },
        { x: 0.2, y: 0.1 },
        { x: 5, y: 0.5 },
        { x: 0.5, y: -1 },
        { x: 0.6, y: 0.6 },
      ],
    },
  });
  // x=5 与 y=-1 的两点被整条丢弃（不是只丢一个分量 —— 半合法的控制点
  // 会在插值时产生 NaN），剩下 0.2/0.6/0.8 且已排序
  assert.deepEqual(
    r.curveRgb.points.map((p) => p.x),
    [0.2, 0.6, 0.8],
  );
  assert.ok(r.curveRgb.points.every((p) => p.y >= 0 && p.y <= 1));
});

test("normalizeRecipe 只在显式 true 时才认为裁剪生效", () => {
  assert.equal(normalizeRecipe({ crop: { enabled: true } }).crop.enabled, true);
  assert.equal(normalizeRecipe({ crop: { enabled: 1 } }).crop.enabled, false);
  assert.equal(normalizeRecipe({ crop: { enabled: "true" } }).crop.enabled, false);
  assert.equal(normalizeRecipe({}).crop.enabled, false);
});

test("readParam / writeParam 按点号路径读写", () => {
  const r = emptyRecipe();
  assert.equal(readParam(r, "basic.exposure"), 0);
  writeParam(r, "basic.exposure", 2.5);
  assert.equal(r.basic.exposure, 2.5);
  assert.equal(readParam(r, "basic.exposure"), 2.5);

  writeParam(r, "sharpen.masking", 0.9);
  assert.equal(r.sharpen.masking, 0.9);
  writeParam(r, "color.vibrance", -0.3);
  assert.equal(r.color.vibrance, -0.3);
});

test("readParam 对不存在的路径返回 0 而不是崩", () => {
  const r = emptyRecipe();
  assert.equal(readParam(r, "basic.notExist"), 0);
  assert.equal(readParam(r, "notAGroup.field"), 0);
  assert.equal(readParam(r, ""), 0);
});

test("writeParam 对不存在的路径静默忽略", () => {
  const r = emptyRecipe();
  const before = JSON.stringify(r);
  writeParam(r, "basic.notExist", 1);
  writeParam(r, "nope.x", 1);
  assert.equal(JSON.stringify(r), before, "未知路径不应产生副作用");
});

test("cloneRecipe 是真深拷贝（撤销栈依赖这一点）", () => {
  const r = emptyRecipe();
  r.basic.exposure = 1.5;
  r.curveRgb.points = [{ x: 0.2, y: 0.3 }];
  const c = cloneRecipe(r);

  c.basic.exposure = -2;
  c.curveRgb.points[0].y = 0.9;
  c.curveRgb.points.push({ x: 0.8, y: 0.9 });
  c.crop.angle = 12;

  assert.equal(r.basic.exposure, 1.5, "基本参数被共享了 → 撤销会失效");
  assert.equal(r.curveRgb.points.length, 1, "曲线数组被共享了");
  assert.equal(r.curveRgb.points[0].y, 0.3, "曲线控制点对象被共享了");
  assert.equal(r.crop.angle, 0, "裁剪被共享了");
});

test("改动任一字段都会让 pristine 变 false", () => {
  const r = emptyRecipe();
  assert.equal(isPristineRecipe(r), true);
  r.basic.exposure = 0.0001;
  assert.equal(isPristineRecipe(r), false);
});

test("裁剪框改动也会让 pristine 变 false", () => {
  const r = emptyRecipe();
  r.crop.enabled = true;
  assert.equal(isPristineRecipe(r), false);
});

test("配方能安全地 JSON 往返（import/export 走这条路）", () => {
  const r: AdjustmentRecipe = emptyRecipe();
  r.basic.exposure = 1.25;
  r.basic.shadows = -0.4;
  r.curveRgb.points = [
    { x: 0.25, y: 0.2 },
    { x: 0.75, y: 0.9 },
  ];
  r.crop = { x: 0.1, y: 0.2, w: 0.6, h: 0.5, angle: 3, enabled: true };
  const back = normalizeRecipe(JSON.parse(JSON.stringify(r)));
  assert.deepEqual(back, r);
});

test("空配方序列化后能解析回空配方", () => {
  const back = normalizeRecipe(JSON.parse(JSON.stringify(emptyRecipe())));
  assert.equal(isPristineRecipe(back), true);
});