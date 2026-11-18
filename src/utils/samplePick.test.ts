/**
 * `utils/samplePick.ts` 单元测试
 *
 * 钉死的语义：等距抽样首尾必覆盖、不重复（前 N 张连拍会让样片失真）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import { pickSample } from "./samplePick.ts";

test("不足样本数时全取", () => {
  assert.deepEqual(pickSample(["a", "b"], 5), ["a", "b"]);
  assert.deepEqual(pickSample([], 5), []);
});

test("等距抽样首尾覆盖、不重复", () => {
  const paths = Array.from({ length: 268 }, (_, i) => `p${i}`);
  const s = pickSample(paths, 5);
  assert.equal(s.length, 5);
  assert.equal(s[0], "p0"); // 首张必在
  assert.equal(s[4], "p267"); // 尾张必在
  assert.equal(new Set(s).size, 5); // 不重复
  // 中间落点确实散开（不是前 5 张）
  assert.equal(s[2], paths[Math.round((2 * 267) / 4)]);
});

test("恰等于样本数时原样返回", () => {
  const paths = ["a", "b", "c", "d", "e"];
  assert.deepEqual(pickSample(paths, 5), paths);
});
