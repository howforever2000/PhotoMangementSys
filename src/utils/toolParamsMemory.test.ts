/**
 * `utils/toolParamsMemory.ts` 单元测试 —— 创意工坊参数记忆
 *
 * 钉死五条不变量：
 *   1. 白名单外的字段永不落盘、也永不被恢复（肤色这类中性档不许被记住）；
 *   2. 类型不对 / 越界 / 枚举非法 → 一律退回默认值，绝不半信半疑地用；
 *   3. 版本不符或 JSON 坏掉 → 整份丢弃回默认（宁可丢记忆，不可灌脏值）；
 *   4. 写某个工具不覆盖别的工具；
 *   5. 「恢复默认」只清当前工具。
 *
 * 存储全部用注入的假对象，不依赖 Node 里没有的 localStorage。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TOOL_PARAMS_KEY,
  TOOL_PARAMS_VERSION,
  clearToolParams,
  differsFromDefaults,
  loadToolParams,
  normalizeToolParams,
  saveToolParams,
  type ParamsStorage,
} from "./toolParamsMemory.ts";

/** 极简内存版 localStorage（够用即可，不实现 Storage 的全部方法） */
function mkStorage(init: Record<string, string> = {}): ParamsStorage & { dump: () => Record<string, string> } {
  const m = new Map(Object.entries(init));
  return {
    getItem: (k) => m.get(k) ?? null,
    setItem: (k, v) => void m.set(k, v),
    removeItem: (k) => void m.delete(k),
    dump: () => Object.fromEntries(m),
  };
}

const BEAUTY_DEFAULTS = { level: "0.6", texture: 35, skin: "natural" };
const RESHAPE_DEFAULTS = { slim: "0.15", eyes: "0.2", eyeBright: 10 };

test("白名单外的字段不恢复（肤色永远回默认）", () => {
  const got = normalizeToolParams(
    "local_beauty",
    { level: "0.8", texture: 60, skin: "fair" },
    BEAUTY_DEFAULTS,
  );
  assert.equal(got.level, "0.8");
  assert.equal(got.texture, 60);
  assert.equal(got.skin, "natural", "肤色不在白名单，必须回默认");
});

test("类型不对 → 回默认值", () => {
  const got = normalizeToolParams("local_beauty", { texture: "60", level: 7 }, BEAUTY_DEFAULTS);
  assert.equal(got.texture, 35);
  assert.equal(got.level, "0.6");
});

test("数值越界按调用方给的控件范围夹取", () => {
  const opts = { clamp: { texture: [0, 90] as const, eyeBright: [0, 100] as const } };
  assert.equal(normalizeToolParams("local_beauty", { texture: 999 }, BEAUTY_DEFAULTS, opts).texture, 90);
  assert.equal(normalizeToolParams("local_beauty", { texture: -3 }, BEAUTY_DEFAULTS, opts).texture, 0);
});

test("枚举非法 → 回默认；枚举合法 → 保留", () => {
  const allowed = { allowed: { eyes: ["0", "0.2", "0.3", "0.5"] } };
  assert.equal(normalizeToolParams("face_reshape", { eyes: "0.9" }, RESHAPE_DEFAULTS, allowed).eyes, "0.2");
  assert.equal(normalizeToolParams("face_reshape", { eyes: "0.5" }, RESHAPE_DEFAULTS, allowed).eyes, "0.5");
});

test("脏输入（null / 数组 / 数字）一律回默认", () => {
  for (const raw of [null, undefined, 42, "x", []]) {
    const got = normalizeToolParams("face_reshape", raw, RESHAPE_DEFAULTS);
    assert.deepEqual(got, RESHAPE_DEFAULTS);
  }
});

test("写盘只存白名单字段，且不动别的工具", () => {
  const s = mkStorage();
  saveToolParams("local_beauty", { level: "0.8", texture: 60, skin: "fair", outputDir: "/tmp" }, s);
  saveToolParams("face_reshape", { slim: "0.3", eyes: "0.2", eyeBright: 20 }, s);
  const blob = JSON.parse(s.dump()[TOOL_PARAMS_KEY]);
  assert.deepEqual(blob.tools.local_beauty, { level: "0.8", texture: 60 }, "skin / outputDir 不许落盘");
  assert.deepEqual(blob.tools.face_reshape, { slim: "0.3", eyes: "0.2", eyeBright: 20 });
  assert.equal(blob.v, TOOL_PARAMS_VERSION);
});

test("读回时经归一化，可直接与默认值合并", () => {
  const s = mkStorage();
  saveToolParams("face_reshape", { slim: "0.3", eyes: "0.2", eyeBright: 20 }, s);
  const got = loadToolParams("face_reshape", RESHAPE_DEFAULTS, {}, s);
  assert.deepEqual(got, { slim: "0.3", eyes: "0.2", eyeBright: 20 });
});

test("版本不符 → 整份丢弃回默认", () => {
  const s = mkStorage({
    [TOOL_PARAMS_KEY]: JSON.stringify({ v: 999, tools: { face_reshape: { slim: "0.8" } } }),
  });
  assert.deepEqual(loadToolParams("face_reshape", RESHAPE_DEFAULTS, {}, s), RESHAPE_DEFAULTS);
});

test("坏 JSON → 回默认且不抛", () => {
  const s = mkStorage({ [TOOL_PARAMS_KEY]: "{ 这不是 JSON" });
  assert.deepEqual(loadToolParams("local_beauty", BEAUTY_DEFAULTS, {}, s), BEAUTY_DEFAULTS);
});

test("清掉某工具只影响它自己", () => {
  const s = mkStorage();
  saveToolParams("local_beauty", { level: "0.8", texture: 60 }, s);
  saveToolParams("face_reshape", { slim: "0.3", eyes: "0.2", eyeBright: 20 }, s);
  clearToolParams("local_beauty", s);
  assert.deepEqual(loadToolParams("local_beauty", BEAUTY_DEFAULTS, {}, s), BEAUTY_DEFAULTS);
  assert.equal(loadToolParams("face_reshape", RESHAPE_DEFAULTS, {}, s).slim, "0.3", "别的工具不受影响");
});

test("清到一条不剩时把整条键删掉", () => {
  const s = mkStorage();
  saveToolParams("local_beauty", { level: "0.8" }, s);
  assert.ok(s.dump()[TOOL_PARAMS_KEY], "先有记录");
  clearToolParams("local_beauty", s);
  assert.equal(s.dump()[TOOL_PARAMS_KEY], undefined, "空表不该留键");
});

test("没有 localStorage 时全部走默认，不抛异常", () => {
  assert.deepEqual(loadToolParams("local_beauty", BEAUTY_DEFAULTS, {}, null), BEAUTY_DEFAULTS);
  saveToolParams("local_beauty", { level: "0.8" }, null);
  clearToolParams("local_beauty", null);
});

test("全是默认值 → 判定为「没调过」（不落记录）", () => {
  assert.equal(differsFromDefaults("local_beauty", { ...BEAUTY_DEFAULTS }, BEAUTY_DEFAULTS), false);
  assert.equal(differsFromDefaults("local_beauty", { ...BEAUTY_DEFAULTS, texture: 70 }, BEAUTY_DEFAULTS), true);
  // 白名单外的字段变了也不算「调过」（肤色永远回默认，不该触发落盘）
  assert.equal(differsFromDefaults("local_beauty", { ...BEAUTY_DEFAULTS, skin: "fair" }, BEAUTY_DEFAULTS), false);
});
