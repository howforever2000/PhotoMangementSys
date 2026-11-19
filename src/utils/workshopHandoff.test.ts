/**
 * `workshopHandoff` 守卫（FEAT-187）
 *
 * 盯三件事：
 *   ① `|` 必须真的能承载多路径且保序（托盘顺序 = 用户勾选顺序）；
 *   ② 分隔符不能被 Windows 路径合法字符命中（否则多选会裂成错误路径）；
 *   ③ 空输入一律 null / 空串 —— 入口按钮据此禁用，不能送出空 query。
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  WORKSHOP_PATH_SEP,
  dedupePhotoPaths,
  joinPhotoPaths,
  splitPhotoPaths,
  workshopRouteTarget,
} from "./workshopHandoff.ts";

const A = "D:\\Photos\\a.jpg";
const B = "D:\\Photos\\b.jpg";
const C = "D:\\Photos\\c.jpg";

test("分隔符是文件名非法字符，不会出现在真实路径里", () => {
  assert.equal(WORKSHOP_PATH_SEP, "|");
  // Windows 文件名禁止 \ / : * ? " < > |，POSIX 只禁 / 与 NUL
  for (const p of [A, B, C]) assert.ok(!p.includes(WORKSHOP_PATH_SEP));
});

test("joinPhotoPaths 去重且保序", () => {
  assert.equal(joinPhotoPaths([B, A, B]), `${B}|${A}`);
  // 顺序不能变：托盘里第一张就是用户先勾的那张
  assert.deepEqual(splitPhotoPaths(joinPhotoPaths([C, A, B])), [C, A, B]);
});

test("joinPhotoPaths 丢掉空串与纯空白", () => {
  assert.equal(joinPhotoPaths(["", "  ", A]), A);
  assert.equal(joinPhotoPaths([]), "");
});

test("splitPhotoPaths 双向等价，且容忍非字符串", () => {
  assert.deepEqual(splitPhotoPaths(joinPhotoPaths([A, B])), [A, B]);
  assert.deepEqual(splitPhotoPaths(undefined), []);
  assert.deepEqual(splitPhotoPaths(null), []);
  assert.deepEqual(splitPhotoPaths(["a", "b"]), []);
});

test("dedupePhotoPaths 保留首次出现的顺序", () => {
  assert.deepEqual(dedupePhotoPaths([B, A, B, C, A]), [B, A, C]);
});

test("单张与多张走同一条 query 键 photos", () => {
  const one = workshopRouteTarget([A]);
  assert.ok(one);
  assert.equal(one.query.photos, A);
  // 单张不该再发 photo 键：两个键并存时消费端得判优先级，是漂移源
  assert.equal(one.query.photo, undefined);

  const many = workshopRouteTarget([A, B, C]);
  assert.deepEqual(many?.query.photos, joinPhotoPaths([A, B, C]));
});

test("空输入返回 null（入口据此禁用按钮）", () => {
  assert.equal(workshopRouteTarget([]), null);
  assert.equal(workshopRouteTarget(["", " "]), null);
});

test("tool 只在显式传入时进 query", () => {
  assert.equal(workshopRouteTarget([A])?.query.tool, undefined);
  assert.equal(workshopRouteTarget([A], "compress")?.query.tool, "compress");
  // 工位 id 的合法性校验属编排层，纯函数不拦（这里锁住「不校验」的既定分工）
  assert.equal(workshopRouteTarget([A], "not_a_tool")?.query.tool, "not_a_tool");
});

test("路由名与 router 定义一致", () => {
  assert.equal(workshopRouteTarget([A])?.name, "workshop");
});
