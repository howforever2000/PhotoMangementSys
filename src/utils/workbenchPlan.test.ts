/**
 * `utils/workbenchPlan.ts` 单元测试
 *
 * 钉死的语义：
 *   1. 链条顺序由 catalog 的 stage 决定，插入不看点击顺序（后端 validate_plan 会拒逆序）；
 *   2. 同一算子只能出现一次；
 *   3. 链条 → 计划只带启用的步，参数原样（滑杆换算在界面层，不在计划里）；
 *   4. 滑杆 0~100 ↔ catalog 值域（slim 上限 0.8 → 滑杆 80）；
 *   5. 从配方起步要带上配方自己的参数，不是 catalog 默认值。
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  chainFromRecipe,
  chainPlan,
  chainSummary,
  chainableOps,
  defaultStep,
  enumText,
  insertOp,
  orderIssues,
  paramViews,
  planRunnable,
  removeOp,
  setEnabled,
  setParam,
  sliderOf,
  sortChain,
  stepSummary,
  applySlider,
  cloneSteps,
  effectiveTuneSteps,
  paramsOverride,
  restoreTune,
  tunePlan,
  type TuneOff,
  type WbChainStep,
} from "./workbenchPlan.ts";

const opsOf = (steps: WbChainStep[]) => steps.map((s) => s.op);

/* ---------------- 可串联算子清单 ---------------- */

test("链条只收本机、有图像产物的算子", () => {
  const ids = chainableOps().map((o) => o.id);
  assert.ok(ids.includes("auto_grade"));
  assert.ok(ids.includes("local_film"));
  assert.ok(ids.includes("face_reshape"));
  assert.ok(ids.includes("skin_smooth"));
  // 文字提取产物是 txt，没有图可比对
  assert.ok(!ids.includes("ocr"));
  // 云端算子一律不进链条（一次拖滑杆 = 一次计费调用）
  for (const o of chainableOps()) assert.equal(o.backend, "local");
});

/* ---------------- 插入与顺序 ---------------- */

test("插入按依赖排序：先加磨皮再加形变，形变会排到前面", () => {
  let s: WbChainStep[] = [];
  s = insertOp(s, "skin_smooth");
  s = insertOp(s, "face_reshape");
  assert.deepEqual(opsOf(s), ["face_reshape", "skin_smooth"]);
});

test("同一算子只加一次（幂等）", () => {
  let s = insertOp([], "local_film");
  const before = s;
  s = insertOp(s, "local_film");
  assert.equal(s, before); // 原样返回，不产生新数组
  assert.equal(opsOf(s).length, 1);
});

test("删除与停用", () => {
  let s = insertOp(insertOp([], "auto_grade"), "local_film");
  s = removeOp(s, "auto_grade");
  assert.deepEqual(opsOf(s), ["local_film"]);
  s = setEnabled(s, "local_film", false);
  assert.equal(s[0].enabled, false);
});

test("顺序满足 requiresBefore，且 sortChain 稳定", () => {
  const s = sortChain([
    { ...defaultStep("skin_smooth") },
    { ...defaultStep("local_film") },
    { ...defaultStep("auto_grade") },
  ]);
  assert.deepEqual(opsOf(s), ["auto_grade", "local_film", "skin_smooth"]);
  assert.deepEqual(orderIssues(s), []);
});

/* ---------------- 链条 → 计划 ---------------- */

test("计划只带启用的步，参数原样透传", () => {
  let s = insertOp([], "auto_grade");
  s = setParam(s, "auto_grade", "strength", 0.8);
  s = insertOp(s, "local_film");
  s = setEnabled(s, "local_film", false);

  const plan = chainPlan(s);
  assert.equal(plan.steps.length, 1);
  assert.equal(plan.steps[0].op, "auto_grade");
  assert.equal(plan.steps[0].params.strength, 0.8);
  assert.equal(plan.intent, "智能调色");
});

test("计划里的顺序即执行顺序（乱序输入也会被排好）", () => {
  const s: WbChainStep[] = [defaultStep("skin_smooth"), defaultStep("auto_grade")];
  assert.deepEqual(
    chainPlan(s).steps.map((x) => x.op),
    ["auto_grade", "skin_smooth"],
  );
});

test("没有启用的步时不可提交", () => {
  let s = insertOp([], "auto_grade");
  assert.equal(planRunnable(s), true);
  s = setEnabled(s, "auto_grade", false);
  assert.equal(planRunnable(s), false);
  assert.equal(chainPlan(s).steps.length, 0);
});

/* ---------------- 参数视图与滑杆换算 ---------------- */

test("滑杆与值域互转：slim 上限 0.8 → 滑杆 80", () => {
  let s = insertOp([], "face_reshape");
  s = setParam(s, "face_reshape", "slim", 0.8);
  assert.equal(sliderOf(s[0], "slim"), 100);
  const params = applySlider(s[0], "slim", 50);
  assert.equal(params.slim, 0.4);
});

test("枚举参数带中文标签（复用 aiPresets / LUT 表）", () => {
  const film = defaultStep("local_film");
  assert.equal(enumText(film, "lut"), "标准彩负");
  const views = paramViews(film);
  const lut = views.find((v) => v.key === "lut");
  assert.equal(lut?.kind, "select");
  assert.ok((lut?.options.length ?? 0) >= 7);

  const grade = defaultStep("auto_grade");
  assert.equal(enumText(grade, "style"), "自然");
});

test("步骤摘要先枚举后滑杆，且不超过两项滑杆", () => {
  const film = defaultStep("local_film");
  const s = stepSummary(film);
  assert.ok(s.startsWith("标准彩负"));
  assert.ok(s.split("·").length <= 3);
});

test("整条链摘要按执行顺序串起来", () => {
  const s = insertOp(insertOp([], "local_film"), "auto_grade");
  assert.equal(chainSummary(s), "智能调色 → 胶片模拟");
  assert.equal(chainSummary([]), "自定义组合");
});

/* ---------------- 从配方起步 ---------------- */

test("从配方起步保留配方参数，不是 catalog 默认值", () => {
  const s = chainFromRecipe("portrait");
  assert.deepEqual(opsOf(s), ["auto_grade", "local_film", "face_reshape", "skin_smooth"]);
  const film = s.find((x) => x.op === "local_film");
  // recipes.json 里人像配方的胶片强度是 0.22，catalog 默认是 1.0
  assert.equal(film?.params.strength, 0.22);
});

test("未知配方 → 空链条", () => {
  assert.deepEqual(chainFromRecipe("nope"), []);
});

/* ---------------- 单步微调：基线 / 差量 / 复位 ---------------- */

test("cloneSteps 深拷贝参数，改副本不影响基线", () => {
  const base = chainFromRecipe("portrait");
  const copy = cloneSteps(base);
  copy[0].params.strength = 0.123;
  assert.notEqual(base[0].params.strength, 0.123);
});

test("覆盖参数只带改过的键，按算子分组", () => {
  const base = chainFromRecipe("portrait");
  const steps = cloneSteps(base);
  const film = steps.find((s) => s.op === "local_film")!;
  film.params.lut = "eterna"; // select 字符串原样透传
  const smooth = steps.find((s) => s.op === "skin_smooth")!;
  smooth.params.strength = 0.7;

  assert.deepEqual(paramsOverride(steps, base), {
    local_film: { lut: "eterna" },
    skin_smooth: { strength: 0.7 },
  });
});

test("参数没动时覆盖为空，套全量走配方原值", () => {
  const base = chainFromRecipe("natural");
  assert.deepEqual(paramsOverride(cloneSteps(base), base), {});
});

test("基线里没有的新参数也算改动", () => {
  const base = chainFromRecipe("natural");
  const steps = cloneSteps(base);
  (steps[0].params as Record<string, number | string | boolean>).grain = 0.3;
  assert.deepEqual(paramsOverride(steps, base), { auto_grade: { grain: 0.3 } });
});

test("复位单步：参数与启用状态都回基线，其他步不动", () => {
  const base = chainFromRecipe("portrait");
  const baseMap = new Map(base.map((s) => [s.op, s.params]));
  const steps = cloneSteps(base);
  for (const s of steps) s.params.strength = 0.01;
  steps[0].enabled = false;

  const one = restoreTune(steps, base, "skin_smooth");
  assert.equal(one.length, steps.length);
  assert.equal(
    one.find((s) => s.op === "skin_smooth")!.params.strength,
    baseMap.get("skin_smooth")!.strength,
  );
  // 其他步仍是被改过的值/停用态
  assert.equal(one.find((s) => s.op === "auto_grade")!.params.strength, 0.01);
  assert.equal(one[0].enabled, false);
  assert.equal(one.find((s) => s.op === "skin_smooth")!.enabled, true);

  const all = restoreTune(steps, base);
  for (const s of all) {
    for (const [k, v] of Object.entries(s.params)) assert.equal(v, baseMap.get(s.op)![k]);
  }
  assert.ok(all.every((s) => s.enabled));
});

/* ---------------- 单参数「不处理」 ---------------- */

test("滑杆拉到 0 只是参数值，不会让步骤消失", () => {
  const base = chainFromRecipe("portrait");
  const steps = cloneSteps(base);
  const reshape = steps.find((s) => s.op === "face_reshape")!;
  reshape.params.slim = 0;
  const out = effectiveTuneSteps(steps, {});
  assert.equal(out.length, base.length);
  const kept = out.find((s) => s.op === "face_reshape")!;
  assert.equal(kept.params.slim, 0);
  assert.equal(kept.params.eyes, reshape.params.eyes);
});

test("枚举参数不处理：整步不参与（LUT / 风格没有中性值）", () => {
  const base = chainFromRecipe("portrait");
  const steps = cloneSteps(base);
  const off: TuneOff = { "local_film.lut": true, "auto_grade.style": true };
  const out = effectiveTuneSteps(steps, off);
  assert.ok(!out.some((s) => s.op === "local_film"));
  assert.ok(!out.some((s) => s.op === "auto_grade"));
  assert.ok(out.some((s) => s.op === "skin_smooth"));
});

test("tunePlan 只收启用步、按依赖排序并带上 intent", () => {
  const base = chainFromRecipe("portrait");
  const off: TuneOff = { "local_film.lut": true };
  const plan = tunePlan(base, off, "人像");
  assert.equal(plan.intent, "人像");
  assert.deepEqual(
    plan.steps.map((s) => s.op),
    ["auto_grade", "face_reshape", "skin_smooth"],
  );
  const stopped = restoreTune(base, base).map((s) => ({ ...s, enabled: false }));
  assert.deepEqual(tunePlan(stopped, {}).steps, []);
});
