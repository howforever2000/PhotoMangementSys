/**
 * `utils/workbenchPlan.ts` 单元测试
 *
 * 钉死的语义：
 *   1. 链条顺序由 catalog 的 stage 决定，插入不看点击顺序（后端 validate_plan 会拒逆序）；
 *   2. **同一算子可以出现多次**（FEAT-197），各步 uid 不同、互不影响；
 *   3. 步骤上限 10 步（与后端 `pipeline.MAX_STEPS` 同口径）；
 *   4. 链条 → 计划只带启用的步，参数原样（滑杆换算在界面层，不在计划里）；
 *   5. 滑杆 0~100 ↔ catalog 值域（slim 上限 0.8 → 滑杆 80）；
 *   6. 从配方起步要带上配方自己的参数，不是 catalog 默认值。
 */
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  chainFromRecipe,
  chainHasCloud,
  chainPlan,
  chainSummary,
  chainableOps,
  canAddOp,
  defaultStep,
  enumText,
  insertOp,
  isCloudOp,
  MAX_CHAIN_STEPS,
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
  tuneOffKey,
  tunePlan,
  type TuneOff,
  type WbChainStep,
} from "./workbenchPlan.ts";

const opsOf = (steps: WbChainStep[]) => steps.map((s) => s.op);

/* ---------------- 可串联算子清单 ---------------- */

test("链条收全部非 internal 且能出图的算子（本机 + 云端）", () => {
  const ids = chainableOps().map((o) => o.id);
  // 本机
  assert.ok(ids.includes("auto_grade"));
  assert.ok(ids.includes("local_film"));
  assert.ok(ids.includes("face_reshape"));
  assert.ok(ids.includes("skin_smooth"));
  // 云端（FEAT-197 放开）
  assert.ok(ids.includes("beauty"));
  assert.ok(ids.includes("style_transfer"));
  assert.ok(ids.includes("upscale"));
  // 文字提取产物是 txt，没有图可比对
  assert.ok(!ids.includes("ocr"));
  // internal 算子（refine）不允许被直接编排
  assert.ok(!ids.includes("refine"));
});

test("isCloudOp / chainHasCloud 按 backend 判定", () => {
  assert.equal(isCloudOp("beauty"), true);
  assert.equal(isCloudOp("auto_grade"), false);
  const s = insertOp([], "beauty");
  assert.equal(chainHasCloud(s), true);
  assert.equal(chainHasCloud(insertOp([], "auto_grade")), false);
  // 停用的云端步不算
  assert.equal(chainHasCloud(setEnabled(s, s[0].uid, false)), false);
});

/* ---------------- 插入与顺序 ---------------- */

test("插入按依赖排序：先加磨皮再加形变，形变会排到前面", () => {
  let s: WbChainStep[] = [];
  s = insertOp(s, "skin_smooth");
  s = insertOp(s, "face_reshape");
  assert.deepEqual(opsOf(s), ["face_reshape", "skin_smooth"]);
});

test("同一算子可以加多次，uid 各不相同（FEAT-197）", () => {
  let s = insertOp([], "local_film");
  s = insertOp(s, "local_film");
  assert.equal(s.length, 2);
  assert.deepEqual(opsOf(s), ["local_film", "local_film"]);
  assert.notEqual(s[0].uid, s[1].uid);
});

test("同名算子只改/删被点的那一步", () => {
  let s = insertOp(insertOp([], "local_film"), "local_film");
  const [first, second] = s;
  s = setParam(s, first.uid, "grain", 0.5);
  assert.equal(s.find((x) => x.uid === first.uid)!.params.grain, 0.5);
  assert.equal(s.find((x) => x.uid === second.uid)!.params.grain, 0);
  s = removeOp(s, first.uid);
  assert.deepEqual(opsOf(s), ["local_film"]);
  assert.equal(s[0].uid, second.uid);
});

test("步数上限 10：到顶后不再插入", () => {
  let s: WbChainStep[] = [];
  for (let i = 0; i < MAX_CHAIN_STEPS + 3; i++) s = insertOp(s, "auto_grade");
  assert.equal(s.length, MAX_CHAIN_STEPS);
  assert.equal(canAddOp(s), false);
  assert.equal(canAddOp(s.slice(0, MAX_CHAIN_STEPS - 1)), true);
});

test("删除与停用", () => {
  let s = insertOp(insertOp([], "auto_grade"), "local_film");
  const grade = s.find((x) => x.op === "auto_grade")!;
  s = removeOp(s, grade.uid);
  assert.deepEqual(opsOf(s), ["local_film"]);
  s = setEnabled(s, s[0].uid, false);
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

test("同名算子重复也满足依赖（按首次出现位置判定）", () => {
  let s = insertOp(insertOp([], "auto_grade"), "local_film");
  s = insertOp(s, "auto_grade");
  assert.deepEqual(orderIssues(s), []);
});

/* ---------------- 链条 → 计划 ---------------- */

test("计划只带启用的步，参数原样透传", () => {
  let s = insertOp([], "auto_grade");
  const gradeUid = s[0].uid;
  s = setParam(s, gradeUid, "strength", 0.8);
  s = insertOp(s, "local_film");
  const film = s.find((x) => x.op === "local_film")!;
  s = setEnabled(s, film.uid, false);

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

test("同名算子两次都进计划（「跑两遍」不会被去重）", () => {
  let s = insertOp(insertOp([], "local_film"), "local_film");
  s = setParam(s, s[1].uid, "strength", 0.4);
  const plan = chainPlan(s);
  assert.deepEqual(
    plan.steps.map((x) => x.op),
    ["local_film", "local_film"],
  );
  assert.equal(plan.steps[1].params.strength, 0.4);
});

test("没有启用的步时不可提交", () => {
  let s = insertOp([], "auto_grade");
  assert.equal(planRunnable(s), true);
  s = setEnabled(s, s[0].uid, false);
  assert.equal(planRunnable(s), false);
  assert.equal(chainPlan(s).steps.length, 0);
});

/* ---------------- 参数视图与滑杆换算 ---------------- */

test("滑杆与值域互转：slim 上限 0.8 → 滑杆 80", () => {
  let s = insertOp([], "face_reshape");
  s = setParam(s, s[0].uid, "slim", 0.8);
  assert.equal(sliderOf(s[0], "slim"), 100);
  const params = applySlider(s[0], "slim", 50);
  assert.equal(params.slim, 0.4);
});

// FEAT-206：双向值域（自然饱和度）——中性 0 落在滑条中间 50，两侧对称换算
test("双向值域：自然饱和度默认滑杆在中间 50，左右对称映射", () => {
  const s = defaultStep("local_vibrance");
  assert.equal(sliderOf(s, "vibrance"), 50, "默认 0 必须落在滑条中间，不是最左");
  assert.equal(applySlider(s, "vibrance", 50).vibrance, 0, "滑条 50 = 不做");
  assert.equal(applySlider(s, "vibrance", 100).vibrance, 0.25, "最右 = +0.25 提升");
  assert.equal(applySlider(s, "vibrance", 0).vibrance, -0.25, "最左 = -0.25 收敛");
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

test("云端算子的枚举也有中文标签（美颜 / 滤镜迁移 / 云端超分）", () => {
  const beauty = defaultStep("beauty");
  assert.equal(enumText(beauty, "style"), "自然");
  assert.equal(enumText(beauty, "level"), "中");
  const transfer = defaultStep("style_transfer");
  assert.equal(enumText(transfer, "mode"), "一句话调色");
  const up = defaultStep("upscale");
  assert.equal(enumText(up, "scale"), "2×");
});

test("步骤摘要先枚举后滑杆，且不超过两项滑杆", () => {
  const film = defaultStep("local_film");
  const s = stepSummary(film);
  assert.ok(s.startsWith("标准彩负"));
  assert.ok(s.split("·").length <= 3);
});

test("整条链摘要按执行顺序串起来，重复算子各列一次", () => {
  const s = insertOp(insertOp([], "local_film"), "auto_grade");
  assert.equal(chainSummary(s), "智能调色 → 胶片模拟");
  assert.equal(chainSummary([]), "自定义组合");
  let dup = insertOp(insertOp([], "local_film"), "local_film");
  dup = setParam(dup, dup[0].uid, "strength", 0.5);
  assert.equal(chainSummary(dup), "胶片模拟 → 胶片模拟");
});

/* ---------------- 从配方起步 ---------------- */

test("从配方起步保留配方参数，不是 catalog 默认值", () => {
  const s = chainFromRecipe("portrait");
  // FEAT-206：配方末尾新增 local_vibrance（默认 0 = 不改出图）
  // FEAT-208：磨皮后新增 skin_tone（肤色保真，复用磨皮的人脸分析）
  assert.deepEqual(opsOf(s), [
    "auto_grade",
    "local_film",
    "face_reshape",
    "skin_smooth",
    "skin_tone",
    "local_vibrance",
  ]);
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
  const baseMap = new Map(base.map((s) => [s.uid, s.params]));
  const steps = cloneSteps(base);
  for (const s of steps) s.params.strength = 0.01;
  steps[0].enabled = false;

  const smoothUid = steps.find((s) => s.op === "skin_smooth")!.uid;
  const one = restoreTune(steps, base, smoothUid);
  assert.equal(one.length, steps.length);
  assert.equal(one.find((s) => s.op === "skin_smooth")!.params.strength, baseMap.get(smoothUid)!.strength);
  // 其他步仍是被改过的值/停用态
  assert.equal(one.find((s) => s.op === "auto_grade")!.params.strength, 0.01);
  assert.equal(one[0].enabled, false);
  assert.equal(one.find((s) => s.op === "skin_smooth")!.enabled, true);

  const all = restoreTune(steps, base);
  for (const s of all) {
    for (const [k, v] of Object.entries(s.params)) assert.equal(v, baseMap.get(s.uid)![k]);
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
  const film = steps.find((s) => s.op === "local_film")!;
  const grade = steps.find((s) => s.op === "auto_grade")!;
  const off: TuneOff = {
    [tuneOffKey(film.uid, "lut")]: true,
    [tuneOffKey(grade.uid, "style")]: true,
  };
  const out = effectiveTuneSteps(steps, off);
  assert.ok(!out.some((s) => s.op === "local_film"));
  assert.ok(!out.some((s) => s.op === "auto_grade"));
  assert.ok(out.some((s) => s.op === "skin_smooth"));
});

test("同名算子的「不处理」互不影响（按 uid 定位）", () => {
  let s = insertOp(insertOp([], "local_film"), "local_film");
  const [a, b] = s;
  s = setParam(s, b.uid, "lut", "velvia");
  const out = effectiveTuneSteps(s, { [tuneOffKey(a.uid, "lut")]: true });
  assert.equal(out.length, 1);
  assert.equal(out[0].uid, b.uid);
  assert.equal(out[0].params.lut, "velvia");
});

test("tunePlan 只收启用步、按依赖排序并带上 intent", () => {
  const base = chainFromRecipe("portrait");
  const film = base.find((s) => s.op === "local_film")!;
  const off: TuneOff = { [tuneOffKey(film.uid, "lut")]: true };
  const plan = tunePlan(base, off, "人像");
  assert.equal(plan.intent, "人像");
  assert.deepEqual(
    plan.steps.map((s) => s.op),
    ["auto_grade", "face_reshape", "skin_smooth", "skin_tone", "local_vibrance"],
  );
  const stopped = restoreTune(base, base).map((s) => ({ ...s, enabled: false }));
  assert.deepEqual(tunePlan(stopped, {}).steps, []);
});
