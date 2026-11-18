/**
 * `utils/workbenchQc.ts` 单元测试
 *
 * 这里钉死的是「质检结论不能骗人」的几条语义：
 *   1. unknown 绝不许显示成通过（后端 critic 在指标无法证明退化时返回 unknown）；
 *   2. 只要有一张 fail，整批就不能报「通过」；
 *   3. issues 必须去重 —— 一批 200 张同类问题不该刷出 200 行。
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import { emptyQc, qcHeadline, qcTone, summarizeQc, verdictLabel } from "./workbenchQc.ts";
import type { Verification } from "../types/ai.ts";

const v = (verdict: Verification["verdict"], issues: string[] = []): Verification => ({
  verdict,
  issues,
});

test("空输入返回空摘要", () => {
  assert.deepEqual(summarizeQc(undefined), emptyQc());
  assert.deepEqual(summarizeQc([]), emptyQc());
  assert.deepEqual(summarizeQc(null), emptyQc());
});

test("按 verdict 分别计数", () => {
  const s = summarizeQc([v("pass"), v("pass"), v("fail"), v("unknown")]);
  assert.equal(s.total, 4);
  assert.equal(s.pass, 2);
  assert.equal(s.fail, 1);
  assert.equal(s.unknown, 1);
});

test("issues 去重且保序", () => {
  const s = summarizeQc([
    v("fail", ["单通道截断增加", "肤色偏移"]),
    v("fail", ["单通道截断增加"]),
    v("pass", []),
  ]);
  assert.deepEqual(s.issues, ["单通道截断增加", "肤色偏移"]);
});

test("未知 verdict 计入 unknown 而非 pass", () => {
  const s = summarizeQc([{ verdict: "weird" as never, issues: [] }]);
  assert.equal(s.unknown, 1);
  assert.equal(s.pass, 0);
});

test("全 pass 才是通过", () => {
  const s = summarizeQc([v("pass"), v("pass")]);
  assert.equal(qcHeadline(s), "全部通过");
  assert.equal(qcTone(s), "ok");
});

test("有 fail 就是异常", () => {
  const s = summarizeQc([v("pass"), v("fail")]);
  assert.equal(qcHeadline(s), "1 张指标异常");
  assert.equal(qcTone(s), "warn");
});

test("unknown 不显示为通过", () => {
  const s = summarizeQc([v("unknown"), v("unknown")]);
  assert.equal(qcHeadline(s), "部分待核");
  assert.equal(qcTone(s), "muted");
});

test("无数据是 idle", () => {
  assert.equal(qcHeadline(emptyQc()), "尚未出片");
  assert.equal(qcTone(emptyQc()), "idle");
});

test("verdictLabel 三态映射", () => {
  assert.equal(verdictLabel(v("pass")), "通过");
  assert.equal(verdictLabel(v("fail")), "异常");
  assert.equal(verdictLabel(v("unknown")), "待核");
  assert.equal(verdictLabel(null), "");
});
