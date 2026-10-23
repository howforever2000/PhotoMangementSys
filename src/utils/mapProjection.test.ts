/**
 * 地图投影与光点半径（FEAT-103 · P3）
 *
 * 这些断言锁的是"点和底图对不上"这类**只在肉眼下才暴露**的问题：
 * 投影一旦偏移，点位会落在海里或邻省，但代码不会报错。
 *
 * 运行：node --experimental-strip-types --test src/utils/mapProjection.test.ts
 */
import test from "node:test";
import assert from "node:assert/strict";

import {
  CHINA_BBOX,
  createProjection,
  inChina,
  pointRadius,
  ringToPath,
} from "./mapProjection.ts";

const W = 600;

test("投影宽高比符合中国范围（约 1.73，横向长）", () => {
  const p = createProjection(W);
  assert.ok(p.height > 0);
  const ratio = p.width / p.height;
  assert.ok(ratio > 1.6 && ratio < 1.85, `实际 ${ratio.toFixed(3)}`);
});

test("四角锚点：左上角映射到 (0,0)，右下角映射到 (宽,高)", () => {
  const p = createProjection(W, 0);
  const [x0, y0] = p.project(CHINA_BBOX.minLon, CHINA_BBOX.maxLat);
  assert.ok(Math.abs(x0) < 1e-6, `左上 x 应≈0，实际 ${x0}`);
  assert.ok(Math.abs(y0) < 1e-6, `左上 y 应≈0，实际 ${y0}`);

  const [x1, y1] = p.project(CHINA_BBOX.maxLon, CHINA_BBOX.minLat);
  assert.ok(Math.abs(x1 - p.width) < 1e-6, `右下 x 应≈宽，实际 ${x1}`);
  assert.ok(Math.abs(y1 - p.height) < 1e-6, `右下 y 应≈高，实际 ${y1}`);
});

test("北京在东北、广州在南方偏东（同纬度经度越大越靠右）", () => {
  const p = createProjection(W);
  const [bjx, bjy] = p.project(116.4, 39.9); // 北京
  const [gzx, gzy] = p.project(113.26, 23.13); // 广州
  assert.ok(gzy > bjy, "广州应在北京下方");
  assert.ok(bjx > gzx, "北京应在广州右侧");
});

test("纬度越高 Y 越小（等距圆柱的 Y 轴翻转正确）", () => {
  const p = createProjection(W);
  const [, yLow] = p.project(100, 20);
  const [, yHigh] = p.project(100, 50);
  assert.ok(yHigh < yLow, "高纬度的 Y 必须更小（更靠上）");
});

test("等距：经度差 10° 在任意纬度上的像素差恒定", () => {
  const p = createProjection(W);
  const dx1 = p.project(100, 20)[0] - p.project(90, 20)[0];
  const dx2 = p.project(100, 50)[0] - p.project(90, 50)[0];
  assert.ok(Math.abs(dx1 - dx2) < 1e-6, `等距圆柱应当 dx 恒定，实际 ${dx1} vs ${dx2}`);
});

test("pad 会四周内缩，且不改变比例", () => {
  const p0 = createProjection(W, 0);
  const p1 = createProjection(W, 20);
  assert.ok(p1.height > p0.height, "有 padding 时总高应变大");
  const [, y0] = p0.project(100, 40);
  const [, y1] = p1.project(100, 40);
  assert.ok(y1 > y0, "内容应整体下移 pad");
});

test("inChina 边界判定：境内 true，境外 false", () => {
  assert.ok(inChina(116.4, 39.9), "北京应判定在境内");
  assert.ok(inChina(87.6, 43.8), "乌鲁木齐应判定在境内");
  assert.ok(!inChina(139.7, 35.7), "东京不应判定在境内");
  assert.ok(!inChina(-0.1, 51.5), "伦敦不应判定在境内");
  // 边界值本身算境内（闭区间）
  assert.ok(inChina(CHINA_BBOX.minLon, CHINA_BBOX.minLat));
  assert.ok(inChina(CHINA_BBOX.maxLon, CHINA_BBOX.maxLat));
});

test("ringToPath：闭合路径，起点为 M，其余为 L", () => {
  const p = createProjection(W);
  const d = ringToPath(
    [
      [116, 40],
      [117, 40],
      [117, 41],
    ],
    p,
  );
  assert.ok(d.startsWith("M"), `应以 M 开头，实际 ${d.slice(0, 10)}`);
  assert.ok(d.endsWith("Z"), "应以 Z 闭合");
  assert.equal((d.match(/L/g) ?? []).length, 2, "3 点应产生 2 个 L");
});

test("ringToPath：空环返回空串（不产生非法 path）", () => {
  const p = createProjection(W);
  assert.equal(ringToPath([], p), "");
});

test("光点半径随张数单调递增，但增速递减（对数）", () => {
  const r1 = pointRadius(1);
  const r10 = pointRadius(10);
  const r100 = pointRadius(100);
  const r1000 = pointRadius(1000);
  assert.ok(r1 < r10 && r10 < r100 && r100 < r1000, "应严格递增");
  // 对数特性：100→1000 的增量显著小于 1→10 的增量
  assert.ok(r1000 - r100 < r10 - r1, "增速必须递减（否则大城糊成一团）");
});

test("光点半径有上下限：单张照片可见、极大值不溢出", () => {
  assert.ok(pointRadius(1) >= 2.5, "1 张照片也要看得见");
  assert.ok(pointRadius(0) === pointRadius(1), "0 张与 1 张同档（不该消失）");
  assert.ok(pointRadius(1e9) <= 13, "大值必须封顶");
});
