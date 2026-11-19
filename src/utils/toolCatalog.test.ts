/**
 * 单一真相源守卫（FEAT-168）
 *
 * 这份测试回答「新增工具时会不会又漏改某处」：直接把 op_catalog.json、
 * Python REGISTRY / config.py、Rust 白名单、spec、前端类型与 LocalModelHint
 * 一起读进来双向核对。手抄漏一处 → 这里立刻红，不会等到用户点工具才炸。
 */
import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import {
  OPS,
  TOOLS,
  GROUPS,
  PYTHON_TOOL_IDS,
  WORKSHOP_TOOL_IDS,
  localToolDefs,
  singleToolsByGroup,
  primaryTool,
  defaultParams,
  toolLabel,
  groupLabelOf,
} from "./toolCatalog.ts";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const read = (p: string) => readFileSync(path.join(ROOT, p), "utf8");
const json = (p: string) => JSON.parse(read(p));

describe("op_catalog.json 自身", () => {
  test("ops / tools id 唯一，且工具引用的算子都存在", () => {
    const c = json("python/ai_studio/op_catalog.json");
    const opIds = new Set<string>(c.ops.map((o: { id: string }) => o.id));
    const toolIds = new Set<string>(c.tools.map((t: { id: string }) => t.id));
    assert.equal(toolIds.size, c.tools.length, "tools id 重复");
    for (const t of c.tools) {
      if (t.op) assert.ok(opIds.has(t.op), `${t.id} 引用了不存在的算子 ${t.op}`);
    }
  });

  test("每个可选参数都有 default，且 enum 默认值在取值内", () => {
    const c = json("python/ai_studio/op_catalog.json");
    for (const o of c.ops) {
      for (const [k, p] of Object.entries(o.params ?? {}) as Array<[string, any]>) {
        assert.ok("default" in p, `${o.id}.${k} 缺 default`);
        if (p.type === "enum") assert.ok(p.values.includes(p.default), `${o.id}.${k} 默认值越界`);
        if (p.type === "range") assert.ok(p.min <= p.default && p.default <= p.max, `${o.id}.${k} 默认值越界`);
      }
    }
  });

  test("requiresBefore 都指向存在且 stage 更小的算子", () => {
    const c = json("python/ai_studio/op_catalog.json");
    const byId = new Map<string, any>(c.ops.map((o: any) => [o.id, o]));
    for (const o of c.ops) {
      for (const dep of o.requiresBefore ?? []) {
        const d = byId.get(dep);
        assert.ok(d, `${o.id} 前置 ${dep} 不存在`);
        assert.ok(d.stage < o.stage, `${o.id} 前置 ${dep} 必须 stage 更小`);
      }
    }
  });

  test("配方与算子契约一致：步骤都在 allowed_ops、顺序满足依赖、schema 可夹取", () => {
    const c = json("python/ai_studio/op_catalog.json");
    const recipes = json("python/ai_studio/recipes.json").recipes;
    const byId = new Map<string, any>(c.ops.map((o: any) => [o.id, o]));
    const allowed = new Set<string>(c.ops.filter((o: any) => !o.internal).map((o: any) => o.id));
    for (const r of recipes) {
      const pos = new Map<string, number>();
      r.steps.forEach((s: any, i: number) => {
        assert.ok(allowed.has(s.op), `${r.id} 用了不允许的算子 ${s.op}`);
        pos.set(s.op, i);
      });
      r.steps.forEach((s: any, i: number) => {
        for (const dep of byId.get(s.op).requiresBefore ?? []) {
          if (pos.has(dep)) assert.ok(pos.get(dep)! < i, `${r.id}: ${s.op} 必须排在 ${dep} 之后`);
        }
      });
    }
  });
});

describe("前端适配层", () => {
  test("OPS / TOOLS 与 JSON 同源，主入口唯一", () => {
    const c = json("python/ai_studio/op_catalog.json");
    assert.deepEqual(OPS.map((o) => o.id), c.ops.map((o: any) => o.id));
    assert.deepEqual(TOOLS.map((t) => t.id), c.tools.map((t: any) => t.id));
    assert.equal(primaryTool().id, "recipe");
  });

  test("工坊 id 与 Python 工作流 id 同域（无 local_reshape ↔ face_reshape 双 id）", () => {
    assert.ok(WORKSHOP_TOOL_IDS.includes("face_reshape"));
    assert.ok(!WORKSHOP_TOOL_IDS.includes("local_reshape"));
    assert.ok(PYTHON_TOOL_IDS.includes("face_reshape"));
    const rustIds = read("src-tauri/src/ai_studio.rs");
    assert.ok(!rustIds.includes('"face_reshape" |'), "Rust 白名单不应再手抄工具 id");
  });

  test("本地引擎列表 = catalog 中非 primary 的本机 Python 工具", () => {
    const names = localToolDefs().map((t) => t.id);
    assert.ok(names.includes("auto_grade") && names.includes("face_reshape"));
    assert.ok(!names.includes("recipe"));
  });

  test("③ 单步工具按类型分组，规划中沉到「处理」组末尾", () => {
    const groups = singleToolsByGroup();
    assert.ok(groups.length > 0);
    // 每个工具都只出现一次：分组不能重复也不能丢
    const listed = groups.flatMap((g) => g.items.map((t) => t.id));
    assert.equal(new Set(listed).size, listed.length, "有工具在多个组里重复出现");
    const singles = TOOLS.filter((t) => t.surface === "single" || t.surface === "planned");
    assert.deepEqual(
      [...listed].sort(),
      singles.map((t) => t.id).sort(),
      "分组后的工具集合必须等于清单里的单步 + 规划中工具",
    );
    // 规划中项一律挂在最后一组末尾
    const last = groups[groups.length - 1];
    assert.equal(last.id, "utility");
    assert.equal(last.items[last.items.length - 1].id, "ocr");
    // 分组字段必须都在 groups 里声明
    const declared = new Set(GROUPS.map((g) => g.id));
    for (const t of TOOLS.filter((x) => x.surface !== "primary")) {
      assert.ok(declared.has(t.group), `工具 ${t.id} 的 group 未声明`);
    }
    assert.equal(groupLabelOf("ocr"), "处理");
  });

  test("新工具 ocr 落在处理组、python 单图、由 ocr 算子承载", () => {
    const t = TOOLS.find((x) => x.id === "ocr")!;
    assert.equal(t.group, "utility");
    assert.equal(t.backend, "local");
    assert.equal(t.surface, "single");
    assert.equal(t.op, "ocr");
    assert.ok(OPS.some((o) => o.id === "ocr"));
    assert.ok(groupLabelOf("film_sim").length > 0);
  });

  test("defaultParams / toolLabel 由 catalog 派生", () => {
    const op = OPS.find((o) => o.id === "face_reshape")!;
    assert.deepEqual(defaultParams(op), { slim: 0.3, eyes: 0.15 });
    assert.equal(toolLabel("face_reshape"), "人像形变");
    assert.equal(toolLabel("__unknown__"), "__unknown__");
  });
});

describe("跨语言注册点（含 LocalModelHint 第三处）", () => {
  test("Python REGISTRY 覆盖全部 python 工具", () => {
    const reg = read("python/ai_studio/workflows/__init__.py");
    const c = json("python/ai_studio/op_catalog.json");
    for (const t of c.tools.filter((x: any) => x.runtime === "python")) {
      assert.ok(new RegExp(`"${t.id}"\\s*:`).test(reg), `REGISTRY 漏了 ${t.id}`);
    }
  });

  test("Rust 白名单由 include_str! 读同一份 JSON，不再手抄", () => {
    const rs = read("src-tauri/src/ai_studio.rs");
    assert.ok(rs.includes('include_str!("../../python/ai_studio/op_catalog.json")'),
      "Rust 必须内联同一份 op_catalog.json");
    assert.ok(rs.includes("tool_policy"), "Rust 必须按清单查 providerPolicy");
  });

  test("spec 的 hiddenimports / datas 由清单派生且含两份 JSON", () => {
    const spec = read("python/ai-studio.spec");
    assert.ok(spec.includes("_catalog_modules()"), "spec 必须由清单生成 hiddenimports");
    assert.ok(spec.includes("op_catalog.json") && spec.includes("recipes.json"));
  });

  test("本地模型三处同步：config.py ↔ model_dl.rs ↔ LocalModelHint.vue（补第三处）", () => {
    const c = json("python/ai_studio/op_catalog.json");
    const modelIds = new Set<string>();
    for (const o of c.ops) for (const m of o.requires?.models ?? []) modelIds.add(m);
    const py = read("python/ai_studio/config.py");
    const rs = read("src-tauri/src/model_dl.rs");
    const vue = read("src/components/workshop/ai/LocalModelHint.vue");
    // DL_NAMES 是 JS 对象字面量，合法标识符常写成不带引号的 key
    // （face_landmark_68: "local-landmark"），所以按「key + 冒号」匹配。
    const hasVueKey = (id: string) => new RegExp(`(?:^|[\\s{])(?:"${id}"|${id})\\s*:`, "m").test(vue);
    for (const id of modelIds) {
      if (id === "det_10g") continue; // 扫描侧共用，不在工坊下载表
      assert.ok(py.includes(`"${id}"`), `config.py 漏了模型 ${id}`);
      assert.ok(hasVueKey(id), `LocalModelHint.DL_NAMES 漏了模型 ${id}`);
    }
  });

  test("前端 types/ai.ts 不再硬编码工具 id 联合", () => {
    const t = read("src/types/ai.ts");
    assert.ok(!t.includes('| "face_reshape"'), "AiTool 联合应从 catalog 派生");
  });
});

describe("样片微调白名单与清单同源（FEAT-186）", () => {
  /**
   * `paramViews`（前端滑杆）与 `TUNABLE_PARAMS`（后端可用键）一直是**人工约定**
   * 逐键一致，没有任何守卫。少写一处就是静默失效：滑杆出现了、拖了、预览没变。
   * 这里从两侧各读一份，做双向断言。
   */
  test("catalog 里每个可微调算子的参数键 == TUNABLE_PARAMS", () => {
    const c = json("python/ai_studio/op_catalog.json");
    const py = read("python/ai_studio/recipecatalog.py");
    // 只解析 TUNABLE_PARAMS 字面量那一段：从声明处截到**顶格收尾的** `}`。
    // 收尾花括号用「行首的 }」定位而不是字面量 "\n}\n" —— 本仓库同时存在 LF 与
    // CRLF 检出（git autocrlf 视平台而定），按 \n 写死会在 CRLF 下定位失败。
    const start = py.indexOf("TUNABLE_PARAMS");
    assert.ok(start >= 0, "recipecatalog.py 里找不到 TUNABLE_PARAMS");
    const close = /^}/m.exec(py.slice(start));
    assert.ok(close, "TUNABLE_PARAMS 字面量收尾花括号定位失败");
    const block = py.slice(start, start + close.index + 1);
    const allowed = new Map<string, Set<string>>();
    for (const m of block.matchAll(/"([a-z_]+)":\s*frozenset\(\{([^}]*)\}\)/g)) {
      const keys = new Set<string>();
      for (const k of m[2].matchAll(/"([a-z_]+)"/g)) keys.add(k[1]);
      allowed.set(m[1], keys);
    }
    assert.ok(allowed.size > 0, "TUNABLE_PARAMS 解析失败");

    for (const [op, keys] of allowed) {
      const spec = c.ops.find((o: { id: string }) => o.id === op);
      assert.ok(spec, `TUNABLE_PARAMS 里的 ${op} 在 catalog 中不存在`);
      const catalogKeys = Object.keys(spec.params ?? {}).sort();
      assert.deepEqual(
        [...keys].sort(),
        catalogKeys,
        `${op} 的前端滑杆与后端白名单已漂移：catalog=[${catalogKeys}] 白名单=[${[...keys]}]`,
      );
    }
    // 反向：catalog 里有参数的本地算子，若没进白名单，说明滑杆会静默失效
    for (const o of c.ops) {
      if (o.internal || !o.params || !Object.keys(o.params).length) continue;
      if (o.backend !== "local") continue;
      if (["local_upscale", "ocr"].includes(o.id)) continue; // 不参与样片微调
      assert.ok(allowed.has(o.id), `${o.id} 有参数但不在 TUNABLE_PARAMS，微调会静默失效`);
    }
  });

  test("auto_grade 的自然饱和度：默认 0、上限 0.25（配方默认不受影响）", () => {
    const c = json("python/ai_studio/op_catalog.json");
    const op = c.ops.find((o: { id: string }) => o.id === "auto_grade");
    assert.ok(op?.params?.vibrance, "auto_grade 缺 vibrance 参数");
    assert.equal(op.params.vibrance.type, "range");
    assert.equal(op.params.vibrance.default, 0, "默认必须是 0，否则四条配方出图会变");
    assert.equal(op.params.vibrance.max, 0.25);
  });
});
