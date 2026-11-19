/**
 * 模型风格预设护栏（FEAT-198）
 *
 * 风格是「后端定义、前端镜像」的双份声明：`python/ai_studio/model_styles.json`
 * 是真值（`modelstyle.py` 载入 + 拼提示词），`src/utils/modelStylePresets.ts`
 * 只镜像 id/label/zh。漂移的症状是**静默的**：
 *   - 前端镜像里有、JSON 里没有 → 后端 `en_block()` 返回空串，用户点了「王家卫式」
 *     却什么都没加，只在 warnings 里留一句；
 *   - 顺序不一致 → chips 顺序与规划提示词里的清单对不上，排查时对不上号。
 *
 * 这份测试钉四件事：
 *   1. id / label / 顺序 与 JSON 完全一致；
 *   2. 中文原文在 JSON 里，且前端追加进输入框的就是**同一段**（不是手抄的副本）；
 *   3. 追加语义：不覆盖用户意图、幂等、切风格时摘旧接新；
 *   4. JSON 里没有品牌黑名单冲突（胶片型号名保留，但动画工作室类 IP 不该出现）。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import {
  MODEL_STYLES,
  appendStyleText,
  modelStyleLabel,
  modelStyleZh,
  stripStyleText,
  switchStyleText,
  toggleStyle,
} from "./modelStylePresets.ts";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const read = (p: string) => readFileSync(path.join(ROOT, p), "utf8");

const json = (): {
  styles: Array<{ id: string; label: string; zh: string; en: string }>;
} => JSON.parse(read("python/ai_studio/model_styles.json"));

describe("前后端声明不许漂移", () => {
  test("id 与顺序与 model_styles.json 一致", () => {
    const py = json().styles;
    assert.equal(py.length, 6, `应恰好 6 条风格，实际 ${py.length}`);
    assert.deepEqual(
      MODEL_STYLES.map((s) => s.id),
      py.map((s) => s.id),
      "前端 MODEL_STYLES 的 id/顺序与 model_styles.json 不一致",
    );
  });

  test("label 与中文原文逐条一致", () => {
    for (const s of json().styles) {
      assert.equal(modelStyleLabel(s.id), s.label, `${s.id} 的 label 前后端不一致`);
      assert.equal(modelStyleZh(s.id), s.zh, `${s.id} 的中文原文前后端不一致`);
    }
  });

  test("六个用户指定款齐全", () => {
    const want = ["hk90", "kodak_gold_200", "wong_kar_wai", "cinestill_800t", "portra_400", "fuji_pro_160ns"];
    assert.deepEqual(MODEL_STYLES.map((s) => s.id), want);
    for (const w of want) assert.ok(MODEL_STYLES.some((s) => s.id === w), `缺少 ${w}`);
  });

  test("每条都有中文原文与英文要点（缺失会让某一路通道静默失效）", () => {
    for (const s of json().styles) {
      assert.ok(s.zh.trim().length >= 100, `${s.id} 中文原文过短（${s.zh.length}）`);
      assert.ok(s.en.trim().length >= 80, `${s.id} 英文要点过短（${s.en.length}）`);
    }
  });
});

describe("后端注入通道（源码级）", () => {
  const py = read("python/ai_studio/modelstyle.py");
  const smart = read("python/ai_studio/workflows/smart_edit.py");

  test("风格段不走 params.extra（norm_params 对 string 截断 500 字）", () => {
    // 真正的注入点是 RunContext.style_prompt → ops._run_edit；
    // 若有人改成塞进 params.extra，长描述会被静默截断。
    assert.ok(
      !/params\[["']extra["']\]\s*\+?=\s*.*style/i.test(smart),
      "smart_edit 不应把风格段写进 params.extra",
    );
    assert.ok(/RunContext\(render_steps,\s*style_block\)/.test(smart), "run() 应把风格段交给 RunContext");
  });

  test("中文与英文分两条通道（避免同一段描述进提示词两次）", () => {
    assert.ok(/def compose_zh\(/.test(py), "modelstyle 需要 compose_zh");
    assert.ok(/def compose_en\(/.test(py), "modelstyle 需要 compose_en");
    assert.ok(/def en_block\(/.test(py), "modelstyle 需要 en_block");
  });

  test("超分步不叠风格段（否则两遍 2× 各带一次风格重绘，颜色逐遍漂移）", () => {
    const ops = read("python/ai_studio/ops.py");
    const upIdx = ops.indexOf("def op_upscale(");
    assert.ok(upIdx > 0, "找不到 op_upscale");
    const body = ops.slice(upIdx, ops.indexOf("\n\n\n", upIdx));
    assert.ok(!body.includes("apply_style"), "op_upscale 不应调用 apply_style");
  });
});

describe("追加语义（Q4(a)：可见可编辑、不覆盖用户输入）", () => {
  test("空输入 → 只放风格原文", () => {
    const out = appendStyleText("", "hk90");
    assert.equal(out, modelStyleZh("hk90"));
  });

  test("已有输入 → 接在原文后面，不覆盖", () => {
    const out = appendStyleText("把这张夜市照片调一下", "cinestill_800t");
    assert.ok(out.startsWith("把这张夜市照片调一下"), "用户意图必须保留在最前面");
    assert.ok(out.includes(modelStyleZh("cinestill_800t")), "风格原文应接在后面");
  });

  test("幂等：连点两次不会出现两份原文", () => {
    const once = appendStyleText("夜景降噪", "hk90");
    const twice = appendStyleText(once, "hk90");
    assert.equal(twice, once);
  });

  test("未知 id → 原样返回", () => {
    assert.equal(appendStyleText("abc", "nope"), "abc");
  });

  test("stripStyleText 能摘掉原文（换风格时用）", () => {
    const withStyle = appendStyleText("意图", "hk90");
    assert.equal(stripStyleText(withStyle, "hk90"), "意图");
  });

  test("用户改过原文时 strip 不动它（宁可留文字也不删用户输入）", () => {
    const edited = "意图\n" + modelStyleZh("hk90") + "（我加了点要求）";
    // 原文仍在 → 会摘掉原文，但「（我加了点要求）」保留
    const out = stripStyleText(edited, "hk90");
    assert.ok(out.includes("我加了点要求"), "用户追加的编辑不能被吞掉");
  });

  test("switchStyleText：摘旧接新", () => {
    const a = appendStyleText("夜景", "hk90");
    const b = switchStyleText(a, "hk90", "portra_400");
    assert.ok(!b.includes(modelStyleZh("hk90")), "旧风格应被摘掉");
    assert.ok(b.includes(modelStyleZh("portra_400")), "新风格应被接上");
    assert.ok(b.startsWith("夜景"));
  });

  test("switchStyleText 到空 id = 只摘不加", () => {
    const a = appendStyleText("夜景", "hk90");
    assert.equal(switchStyleText(a, "hk90", ""), "夜景");
  });

  test("toggleStyle：同款再点即取消", () => {
    assert.equal(toggleStyle("", "hk90"), "hk90");
    assert.equal(toggleStyle("hk90", "hk90"), "");
    assert.equal(toggleStyle("hk90", "fuji_pro_160ns"), "fuji_pro_160ns");
  });
});

describe("内容约束", () => {
  const raw = read("python/ai_studio/model_styles.json").toLowerCase();

  test("不点名动画工作室 / 导演（网关审核会随机拒答，与 FEAT-170 一致）", () => {
    for (const bad of ["ghibli", "miyazaki", "disney", "pixar", "shinkai"]) {
      assert.ok(!raw.includes(bad), `风格数据不应出现 IP 名「${bad}」`);
    }
    // 注意：`wong_kar_wai`（王家卫）是用户指定的风格名，属胶片影调语境，
    // 与「不点名动画工作室」不冲突；这里只挡动画/工作室类。
    assert.ok(raw.includes("wong_kar_wai"), "用户指定的王家卫式风格应保留");
  });

  test("胶片型号名保留（用户裁定 Q3）", () => {
    for (const brand of ["portra", "kodak", "cinestill", "fuji"]) {
      assert.ok(raw.includes(brand), `应保留胶片型号名「${brand}」`);
    }
  });

  test("每条中文原文都提到影调要素（不是空泛的一句「好看」）", () => {
    for (const s of json().styles) {
      const hasTone = /高光|暗部|中间调|影调|肤色|颗粒|反差|饱和/.test(s.zh);
      assert.ok(hasTone, `${s.id} 的中文原文缺少影调描述`);
    }
  });
});
