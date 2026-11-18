/**
 * 主题手账画面风格护栏（FEAT-170）
 *
 * 手账风格是「后端定文案、前端定标签」的双份声明：`python/ai_studio/presets.py`
 * 的 `JOURNAL_STYLES` 是真值，`src/utils/aiPresets.ts` 只镜像 key + 标签 + 分组。
 * 两份一旦漂移，症状是**静默的** —— 前端能点、后端 `JOURNAL_STYLES.get(style, sketch)`
 * 悄悄回落成素描，用户以为选了卡通却出了张铅笔稿。这份测试盯三件事：
 *   1. key 与 group 两份声明完全一致；
 *   2. 分组表覆盖所有被用到的 group（漏登记 = 前端渲染出一个空组被 filter 掉，选项消失）；
 *   3. 卡通提示词不点名第三方 IP、且仍带 no text（文字由版式层渲染）。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { JOURNAL_STYLES, JOURNAL_STYLE_GROUPS } from "./aiPresets.ts";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const read = (p: string) => readFileSync(path.join(ROOT, p), "utf8");

/** 从 `presets.py` 的 JOURNAL_STYLES 块里抽出 [{ key, label, group }]（块结束于下一个顶层注释）。 */
function pythonJournalStyles(): { key: string; label: string; group: string }[] {
  // 统一换行：仓库里 Python 文件是 CRLF，正则若按 \n 收尾会永远匹配不到块尾
  const py = read("python/ai_studio/presets.py").replace(/\r\n/g, "\n");
  const start = py.indexOf("JOURNAL_STYLES: dict[str, dict] = {");
  assert.ok(start >= 0, "presets.py 里找不到 JOURNAL_STYLES");
  const end = py.indexOf("\n}\n", start);
  assert.ok(end > start, "JOURNAL_STYLES 块未正常闭合");
  const block = py.slice(start, end);

  const out: { key: string; label: string; group: string }[] = [];
  const entry = /^ {4}"([a-z0-9_]+)": \{\n([\s\S]*?)^ {4}\},$/gm;
  for (const m of block.matchAll(entry)) {
    const body = m[2];
    const label = body.match(/"label": "([^"]+)"/)?.[1];
    const group = body.match(/"group": "([^"]+)"/)?.[1];
    assert.ok(label, `${m[1]} 缺 label`);
    assert.ok(group, `${m[1]} 缺 group（FEAT-170 起每条风格必须声明分组）`);
    out.push({ key: m[1], label: label!, group: group! });
  }
  return out;
}

describe("手账画面风格：两份声明不许漂移", () => {
  test("key 集合与顺序与后端一致", () => {
    const py = pythonJournalStyles();
    assert.ok(py.length >= 6, `后端只解析出 ${py.length} 条风格，解析规则可能失效`);
    assert.deepEqual(
      JOURNAL_STYLES.map((s) => s.value),
      py.map((s) => s.key),
      "前端 JOURNAL_STYLES 的 key/顺序与 presets.py 不一致（漂移会让用户选到回落成素描的风格）",
    );
  });

  test("每条风格的 label 与 group 与后端一致", () => {
    const py = pythonJournalStyles();
    for (const p of py) {
      const fe = JOURNAL_STYLES.find((s) => s.value === p.key)!;
      assert.equal(fe.label, p.label, `${p.key} 的 label 前后端不一致`);
      assert.equal(fe.group, p.group, `${p.key} 的 group 前后端不一致`);
    }
  });

  test("分组表覆盖全部被用到的 group（漏登记 = 该组选项在前端消失）", () => {
    const declared = new Set(JOURNAL_STYLE_GROUPS.map((g) => g.value));
    for (const s of JOURNAL_STYLES) {
      assert.ok(declared.has(s.group), `${s.value} 的 group「${s.group}」未登记进 JOURNAL_STYLE_GROUPS`);
    }
    // 反向：登记的组必须真的有风格，否则前端出现空标题
    for (const g of JOURNAL_STYLE_GROUPS) {
      assert.ok(
        JOURNAL_STYLES.some((s) => s.group === g.value),
        `分组「${g.label}」下没有任何风格`,
      );
    }
  });

  test("卡通形象组齐全：2D 动画夏日 + 国漫 3D", () => {
    const cartoon = JOURNAL_STYLES.filter((s) => s.group === "cartoon").map((s) => s.value);
    for (const want of ["anime_summer", "guoman_3d"]) {
      assert.ok(cartoon.includes(want), `卡通形象组缺少 ${want}`);
    }
  });
});

describe("卡通提示词的硬要求", () => {
  const py = read("python/ai_studio/presets.py").replace(/\r\n/g, "\n");
  const journalBlock = py.slice(
    py.indexOf("JOURNAL_STYLES: dict[str, dict] = {"),
    py.indexOf("def journal_style_prompt("),
  );

  test("不点名第三方工作室/IP（网关审核会随机拒答，产品文案也不该写别人 IP）", () => {
    for (const bad of ["ghibli", "miyazaki", "studio ghibli", "hayao", "disney", "pixar", "shinkai"]) {
      assert.ok(!journalBlock.toLowerCase().includes(bad), `手账风格提示词不应出现 IP 名「${bad}」`);
    }
  });

  test("卡通提示词同样带 no text（文字由版式层确定渲染）", () => {
    for (const key of ["anime_summer", "guoman_3d"]) {
      const i = journalBlock.indexOf(`"${key}": {`);
      assert.ok(i > 0, `找不到 ${key}`);
      const body = journalBlock.slice(i, journalBlock.indexOf('    },', i));
      assert.ok(/no text/i.test(body), `${key} 提示词必须带 no text`);
    }
  });

  test("手账仍不拼结构保全硬段（卡通化改形状是目的本身）", () => {
    const body = py.slice(py.indexOf("def journal_style_prompt("));
    assert.ok(!body.slice(0, 600).includes("lock("), "journal 不应拼结构保全硬段");
  });

  /** 取某款风格 `prompt` 字段的字符串字面量拼接结果（跳过行内中文注释，只看真正发给网关的文本）。 */
  function promptLiteral(key: string): string {
    const i = journalBlock.indexOf(`"${key}": {`);
    assert.ok(i > 0, `找不到 ${key}`);
    const body = journalBlock.slice(i, journalBlock.indexOf("    },", i));
    const start = body.indexOf('"prompt": (');
    assert.ok(start > 0, `${key} 缺 prompt`);
    const end = body.indexOf(")", start);
    const chunk = body.slice(start, end);
    return [...chunk.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1]).join("");
  }

  test("卡通提示词是纯 ASCII（非 ASCII 标点在网关请求体里会被二次转义）", () => {
    // 实测踩过：句子里写了 em dash「—」，json.dumps 后变成 \u2014，
    // 部分网关把它当非法字符回 400。提示词一律用 ASCII，中文只出现在 label 与注释。
    for (const key of ["anime_summer", "guoman_3d"]) {
      const bad = [...promptLiteral(key)].filter((c) => c.charCodeAt(0) > 126);
      assert.equal(bad.length, 0, `${key} 提示词含非 ASCII 字符：${bad.join("")}`);
    }
  });

  test("卡通提示词排除另一条通路（2D 不许漂成 3D，3D 不许漂成手绘）", () => {
    // 两条卡通风格最典型的翻车是互相污染：2D 出成塑料 3D 感、3D 出成厚涂插画。
    // 网关没有 negative prompt 通道，禁令只能写成正面约束句，因此这里断言它们存在。
    const flat2d = promptLiteral("anime_summer");
    assert.ok(/not 3D/i.test(flat2d), "anime_summer 必须显式排除 3D");
    assert.ok(/not CGI/i.test(flat2d), "anime_summer 必须显式排除 CGI");
    assert.ok(/cel shading/i.test(flat2d), "anime_summer 必须点名 cel shading");
    assert.ok(/watercolor/i.test(flat2d), "anime_summer 背景必须点名 watercolor");

    const cg3d = promptLiteral("guoman_3d");
    assert.ok(/not a photograph/i.test(cg3d), "guoman_3d 必须显式排除照片");
    assert.ok(/not a hand-drawn illustration/i.test(cg3d), "guoman_3d 必须显式排除手绘");
    assert.ok(/subsurface scattering/i.test(cg3d), "guoman_3d 必须点名 SSS 皮肤");
    assert.ok(/rim light/i.test(cg3d), "guoman_3d 必须点名 rim light");
  });
});
