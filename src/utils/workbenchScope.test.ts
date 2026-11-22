/**
 * `utils/workbenchScope.ts` 单元测试
 *
 * 钉死的语义（BUG-2026-1122-008）：
 *   1. 导入不自动勾选 —— 勾选是用户对「哪些照片要处理、要不要花钱」的表态，
 *      程序不替他做；
 *   2. 没勾选 = 全量（这条路没断，只是不该由程序替他选）；
 *   3. 两种情况说法必须不同：「已选 N 张」vs「全部 N 张」——
 *      都只写「N 张」的话，用户会以为只处理了勾选的那些。
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import { scopeText, scopeWord, targetCount, targetItems } from "./workbenchScope.ts";

const items = (n: number) =>
  Array.from({ length: n }, (_, i) => ({ path: `D:/p${i}.jpg`, name: `p${i}.jpg` }));

test("没勾选 = 全量", () => {
  assert.equal(targetItems(items(5), []).length, 5);
  assert.equal(targetCount(items(5), []), 5);
});

test("有勾选 = 只取勾选的那些", () => {
  const its = items(5);
  assert.deepEqual(targetItems(its, ["D:/p1.jpg", "D:/p3.jpg"]).map((i) => i.path), [
    "D:/p1.jpg",
    "D:/p3.jpg",
  ]);
  assert.equal(targetCount(its, ["D:/p1.jpg", "D:/p3.jpg"]), 2);
});

test("勾选了不存在的路径不算数（不静默扩大范围）", () => {
  const its = items(3);
  assert.equal(targetCount(its, ["D:/nope.jpg"]), 0);
});

test("说法随勾选状态切换", () => {
  assert.equal(scopeWord(0), "全部");
  assert.equal(scopeWord(3), "已选");
});

test("确认框的说明句：没勾选要点明「全量」", () => {
  assert.match(scopeText(0, 12), /全部 12 张/);
  assert.match(scopeText(0, 12), /没勾选 = 全量/);
  assert.match(scopeText(3, 12), /3 张是你勾选的/);
  // 已勾选时不该再说「全部」—— 那是另一套范围
  assert.doesNotMatch(scopeText(3, 12), /全部/);
});

test("导入不自动勾选：selected 为空", () => {
  // store 的 addPaths 只 push items，不碰 selected —— 这里用纯函数侧写死契约：
  // 新导入的路径不出现在任何"已选"集合里时，范围仍是全量而非"0 张"
  const its = items(3);
  assert.equal(targetCount(its, []), 3);
});
