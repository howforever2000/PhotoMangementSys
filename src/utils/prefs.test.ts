/**
 * `utils/prefs.ts` 单元测试 —— FEAT-086 旧数据兼容
 *
 * 钉死三条不变量：
 *   1. 旧结构（无 gradMid/compAlpha/material）→ 自动补齐且视觉等价；
 *   2. 坏值（非法 hex / 越界数值 / 非法枚举）一律回落默认，绝不抛错；
 *   3. 完整新结构原样保留。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { hexToRgb, rgbToHsl } from "./color.ts";
import { DEFAULTS, isDefaultLook, normalizePrefs } from "./prefs.ts";

test("FEAT-094 默认外观 = 启动封面（壁纸 + 落日紫金）", () => {
  // 背景回到「背景图」模式，且默认字就是壁纸；颜色为落日紫金
  assert.equal(DEFAULTS.bgStyle, "image");
  // 素化 C7：组件色调由 #2b2140（S≈0.34，带明显紫）降为 #2a2630（S≈0.14，近中性灰紫）。
  // 这里钉的是「低饱和」这条口径而不是字面量 —— 换壁纸/换底色时不必回来改测试，
  // 但任何人把默认底色调回高饱和都会被这条抓住。
  assert.equal(DEFAULTS.compColor, "#2a2630");
  {
    const [h, s] = rgbToHsl(hexToRgb(DEFAULTS.compColor));
    assert.ok(s <= 0.2, `默认组件色饱和度 ${s.toFixed(2)} > 0.2（素化口径：卡片底应近中性）`);
    assert.ok(h > 240 || h < 60, `默认组件色色相 ${h.toFixed(0)} 应落在中性紫区间（260~300 或其对侧）`);
  }
  assert.equal(DEFAULTS.compAlpha, 0.64);
  assert.equal(DEFAULTS.bgOpacity, 0.55);
  assert.equal(DEFAULTS.material, "frosted");
});

test("FEAT-094 isDefaultLook：空输入归一化后命中默认，任一可调项被改即掉高亮", () => {
  // 空输入 → 归一化结果必须**就是**默认外观（gradMid 若不等首尾中点，这条会红）
  assert.equal(isDefaultLook(normalizePrefs({}), false), true);
  // 用户自己选过背景图 → 一律不算默认
  assert.equal(isDefaultLook(normalizePrefs({}), true), false);
  const perturbations = [
    { bgStyle: "color" },
    { bgStyle: "gradient" },
    { bgColor: "#000000" },
    { gradFrom: "#000000" },
    { compColor: "#16443a" },
    { compAlpha: 0.42 },
    { bgOpacity: 0.45 },
    { material: "liquid" },
  ];
  for (const patch of perturbations) {
    assert.equal(isDefaultLook(normalizePrefs(patch), false), false, `patch=${JSON.stringify(patch)}`);
  }
  // 饱和度不算「脱离默认」（它只改浓淡不改结构，否则拖一下滑块默认卡就掉高亮）
  assert.equal(isDefaultLook(normalizePrefs({ saturation: 0.6 }), false), true);
  // 同理适用于色温（Q3）：它也只改色相不改结构
  assert.equal(isDefaultLook(normalizePrefs({ temperature: 0.6 }), false), true);
  assert.equal(isDefaultLook(normalizePrefs({ temperature: -1 }), false), true);
});

test("FEAT-087 saturation：越界被夹到 0.4~1.5，坏值回落 1", () => {
  assert.equal(normalizePrefs({}).saturation, 1);
  assert.equal(normalizePrefs({ saturation: 0.1 }).saturation, 0.4);
  assert.equal(normalizePrefs({ saturation: 9 }).saturation, 1.5);
  assert.equal(normalizePrefs({ saturation: "xx" }).saturation, 1);
  assert.equal(normalizePrefs({ saturation: 1.35 }).saturation, 1.35);
});

test("Q3 temperature：字段保留但模块已停用 —— 界面实际只可能取到 0", () => {
  assert.equal(DEFAULTS.temperature, 0);
  // 老 localStorage 里没有这个字段 → 补 0 ⇒ 外观逐像素不变
  assert.equal(normalizePrefs({}).temperature, 0);

  /* 停用后**不再**对读入的值做归一化（theme store 的 temperature 是常量 0，
     prefsSnapshot 也恒写 0）。这里只锁两条与「不丢数据」直接相关的不变量：
     ① 字段仍在结构里（否则 normalizePrefs 会把老 localStorage 打回默认，
        背景图/预设全丢且不可逆）；
     ② 值合法（否则将来若恢复功能，一个坏值会直接进管线）。 */
  assert.ok("temperature" in DEFAULTS, "Prefs 结构必须保留 temperature 字段");
  assert.ok("temperature" in normalizePrefs({}), "归一化结果必须仍带该字段");
  // 字段类型/量纲：即便停用，读进来的值仍应落在 [-1, 1] 这个既定区间内
  for (const raw of [{ temperature: 0.35 }, { temperature: -9 }, { temperature: 9 }, { temperature: "warm" }]) {
    const t = normalizePrefs(raw).temperature;
    assert.ok(
      typeof t === "number" && Number.isFinite(t) && t >= -1 && t <= 1,
      `temperature=${JSON.stringify(raw.temperature)} 归一化后越界/非数: ${t}`,
    );
  }
});

test("旧结构（FEAT-084 时代）→ 补出 gradMid 中点 + 默认 compAlpha/material", () => {
  const legacy = {
    mode: "dark",
    compColor: "#123456",
    bgStyle: "gradient",
    bgColor: "#0e211b",
    gradFrom: "#000000",
    gradTo: "#ffffff",
    gradAngle: 90,
    bgOpacity: 0.5,
  };
  const p = normalizePrefs(legacy);
  // 中点 = mix(black, white, 0.5) = #808080
  assert.equal(p.gradMid, "#808080");
  assert.equal(p.compAlpha, DEFAULTS.compAlpha);
  assert.equal(p.material, "frosted");
  // FEAT-087：旧结构无 saturation → 补 1（饱和度管线默认不干预旧外观）
  assert.equal(p.saturation, 1);
  assert.equal(p.compColor, "#123456");
  assert.equal(p.gradAngle, 90);
});

test("空对象 / null / 坏 JSON 字段 → 全量回落默认，不抛错", () => {
  for (const input of [null, undefined, {}, { compColor: "not-a-color", compAlpha: "xx", material: "wood" }]) {
    const p = normalizePrefs(input);
    assert.deepEqual(p, DEFAULTS, `input=${JSON.stringify(input)}`);
  }
});

test("越界数值被夹紧到合法区间", () => {
  const p = normalizePrefs({ compAlpha: 5, gradAngle: -100, bgOpacity: 0 });
  assert.equal(p.compAlpha, 0.9);
  assert.equal(p.gradAngle, 0);
  assert.equal(p.bgOpacity, 0.05);

  const q = normalizePrefs({ compAlpha: 0.01, bgOpacity: 3 });
  // FEAT-094：透明度下限由 0.15 放开到 0 —— 「能不能用 0」取决于背景是否够均匀，
  // 该由用户看着弹窗里的实测读数自己决定，不该被一个拍脑袋的下限拦住。
  // 0.01 现在处于合法区间内，原样保留（旧行为会被下限顶到 0.15）。
  assert.equal(q.compAlpha, 0.01);
  assert.equal(normalizePrefs({ compAlpha: -3 }).compAlpha, 0);
  assert.equal(normalizePrefs({ compAlpha: 0 }).compAlpha, 0);
  assert.equal(q.bgOpacity, 1);
});

test("完整新结构原样保留（含 liquid 材质与三段渐变）", () => {
  const full = {
    mode: "light",
    compColor: "#abcdeg", // 非法 → 回落
    compAlpha: 0.6,
    material: "liquid",
    bgStyle: "gradient",
    bgColor: "#101010",
    gradFrom: "#ff0000",
    gradMid: "#00ff00",
    gradTo: "#0000ff",
    gradAngle: 45,
    bgOpacity: 0.8,
  };
  const p = normalizePrefs(full);
  assert.equal(p.mode, "light");
  assert.equal(p.compColor, DEFAULTS.compColor);
  assert.equal(p.compAlpha, 0.6);
  assert.equal(p.material, "liquid");
  assert.equal(p.gradMid, "#00ff00");
  assert.equal(p.gradAngle, 45);
  assert.equal(p.bgOpacity, 0.8);
});
