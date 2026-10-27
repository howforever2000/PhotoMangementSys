/**
 * `utils/customPresets.ts` 单元测试 —— FEAT-118 自定义预设
 *
 * 钉死四条不变量：
 *   1. 上限 6 是硬约束，且满了要「拒绝」而不是「丢最早」（静默丢失最恼火）；
 *   2. 坏数据整条丢弃，绝不半条进入界面（用户外观丢失最难受）；
 *   3. 名字必填落到兜底值、超长截断；
 *   4. 从 Prefs 摘字段时色值/角度/透明度都必须归一化过。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  MAX_CUSTOM_PRESETS,
  addCustomPreset,
  customPresetFromPrefs,
  normalizeCustomPreset,
  normalizeName,
  readCustomPresets,
  removeCustomPreset,
  CUSTOM_PRESETS_KEY,
  type CustomPreset,
} from "./customPresets.ts";
import { DEFAULTS } from "./prefs.ts";

function mk(i: number): CustomPreset {
  return {
    id: `id-${i}`,
    name: `预设${i}`,
    mode: "dark",
    colors: ["#111111", "#222222", "#333333"],
    angle: 165,
    compColor: "#2a2630",
    compAlpha: 0.62,
    createdAt: i,
  };
}

test("customPresetFromPrefs：从当前偏好摘字段并归一化", () => {
  const p = customPresetFromPrefs(
    {
      mode: "light",
      gradFrom: "#FFF8F0",
      gradMid: "#eee",
      gradTo: " #d8bda2 ",
      gradAngle: 999,
      compColor: "#ABCDEF",
      compAlpha: 1.7,
    },
    "  我的 深 绿  ",
    1700000000000,
  );
  assert.equal(p.mode, "light");
  // #rgb 展开、大写归一、首尾空白裁掉
  assert.deepEqual(p.colors, ["#fff8f0", "#eeeeee", "#d8bda2"]);
  assert.equal(p.angle, 360, "角度必须夹到 0~360");
  assert.equal(p.compColor, "#abcdef");
  assert.equal(p.compAlpha, 1, "透明度必须夹到 0~1");
  assert.equal(p.name, "我的 深 绿", "连续空白折叠为单个空格并去首尾");
  assert.ok(p.id.startsWith("cp-"), "id 由时间戳 + 随机段生成");
});

test("normalizeName：空 / 全空白落到兜底值，超长截断", () => {
  assert.equal(normalizeName(""), "未命名预设");
  assert.equal(normalizeName("   "), "未命名预设");
  assert.equal(normalizeName(undefined as unknown as string), "未命名预设");
  assert.equal(normalizeName("一二三四五六七八九十十一十二"), "一二三四五六七八九十十一");
});

test("normalizeCustomPreset：坏数据整条丢弃", () => {
  assert.equal(normalizeCustomPreset(null), null);
  assert.equal(normalizeCustomPreset("x"), null);
  assert.equal(normalizeCustomPreset({}), null, "缺 id / colors");
  assert.equal(
    normalizeCustomPreset({ id: "a", colors: ["#fff", "#000"] }),
    null,
    "colors 必须恰好三段",
  );
  assert.equal(
    normalizeCustomPreset({ id: "a", colors: ["#fff", "#000", "not-a-color"] }),
    null,
    "任一段非法即整条丢弃",
  );
  assert.equal(normalizeCustomPreset({ colors: ["#fff", "#000", "#111"] }), null, "缺 id");
});

test("normalizeCustomPreset：合法数据归一化（未知 mode 回落 dark）", () => {
  const p = normalizeCustomPreset({
    id: " a ",
    name: "  夏日 ",
    mode: "weird",
    colors: ["#FFF", "#000", "#111111"],
    angle: -20,
    compColor: "#abc",
    compAlpha: 0.335,
  });
  assert.ok(p);
  assert.equal(p.id, "a");
  assert.equal(p.name, "夏日");
  assert.equal(p.mode, "dark");
  assert.deepEqual(p.colors, ["#ffffff", "#000000", "#111111"]);
  assert.equal(p.angle, 0);
  assert.equal(p.compColor, "#aabbcc");
  assert.equal(p.compAlpha, 0.34, "透明度取两位小数");
});

test("readCustomPresets：坏 JSON / 非数组 → 空列表，不抛错", () => {
  assert.deepEqual(readCustomPresets(() => "{oops"), []);
  assert.deepEqual(readCustomPresets(() => null), []);
  assert.deepEqual(readCustomPresets(() => '{"a":1}'), []);
  assert.deepEqual(readCustomPresets(() => "[1,2,3]"), [], "非法条目全丢");
});

test("readCustomPresets：去重 id 并截断到上限", () => {
  const many = Array.from({ length: 10 }, (_, i) => mk(i));
  const dup = [mk(1), mk(1), mk(2)];
  assert.equal(readCustomPresets(() => JSON.stringify(many)).length, MAX_CUSTOM_PRESETS);
  const dedup = readCustomPresets(() => JSON.stringify(dup));
  assert.deepEqual(
    dedup.map((p) => p.id),
    ["id-1", "id-2"],
  );
});

test("addCustomPreset：满了拒绝，不丢最早的", () => {
  let list: CustomPreset[] = [];
  for (let i = 0; i < MAX_CUSTOM_PRESETS; i++) {
    const r = addCustomPreset(list, mk(i));
    assert.equal(r.ok, true);
    list = r.list;
  }
  const full = addCustomPreset(list, mk(99));
  assert.equal(full.ok, false, "第 7 个必须被拒绝");
  assert.equal(full.list.length, MAX_CUSTOM_PRESETS);
  assert.equal(full.list[0].id, "id-0", "被拒绝时最早的必须还在");
});

test("removeCustomPreset：按 id 删除且不影响其余顺序", () => {
  const list = [mk(1), mk(2), mk(3)];
  assert.deepEqual(
    removeCustomPreset(list, "id-2").map((p) => p.id),
    ["id-1", "id-3"],
  );
  assert.equal(removeCustomPreset(list, "不存在").length, 3);
});

test("storage key 固定（改动会让老用户的自定义预设消失）", () => {
  assert.equal(CUSTOM_PRESETS_KEY, "pm-theme.customPresets");
});

test("默认偏好也能存成预设（不是只有渐变模式才能存）", () => {
  const p = customPresetFromPrefs(DEFAULTS, "默认");
  assert.equal(p.colors.length, 3);
  assert.equal(p.compColor, DEFAULTS.compColor.toLowerCase());
});
