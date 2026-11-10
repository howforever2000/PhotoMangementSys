/**
 * 前端语法/模板编译自检（FEAT-152）
 *
 * 不依赖 vue-tsc/volar 的轻量检查：用 typescript transpile 抓 TS 语法错，
 * 用 @vue/compiler-sfc 的 compileScript/compileTemplate 抓 .vue 脚本与模板语法错。
 * （类型级检查仍需在开发机跑 `pnpm build` / vue-tsc。）
 */
import fs from "node:fs";
import ts from "typescript";
import { compileScript, compileTemplate, parse } from "@vue/compiler-sfc";

const FILES = [
  "src/types/ai.ts",
  "src/utils/aiPresets.ts",
  "src/stores/aiStudio.ts",
  "src/components/workshop/ToolIcon.vue",
  "src/components/workshop/ai/AiPromptPanel.vue",
  "src/components/workshop/ai/AiResultPanel.vue",
  "src/components/workshop/ai/AiSettingsDialog.vue",
  "src/components/workshop/ai/JournalTool.vue",
  "src/components/workshop/ai/BeautyTool.vue",
  "src/components/workshop/ai/StyleTransferTool.vue",
  "src/components/workshop/ai/UpscaleTool.vue",
  "src/views/Workshop.vue",
];

let errors = 0;
for (const f of FILES) {
  const src = fs.readFileSync(f, "utf8");
  if (f.endsWith(".ts")) {
    const out = ts.transpileModule(src, {
      fileName: f,
      compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
      reportDiagnostics: true,
    });
    for (const d of out.diagnostics || []) {
      if (d.category === ts.DiagnosticCategory.Error) {
        errors += 1;
        console.log("TS", f, ts.flattenDiagnosticMessageText(d.messageText, "\n"));
      }
    }
    continue;
  }
  const { descriptor, errors: perr } = parse(src, { filename: f });
  if (perr && perr.length) {
    errors += perr.length;
    perr.forEach((e) => console.log("SFC", f, e.message));
  }
  try {
    const s = compileScript(descriptor, { id: "chk" });
    const tpl = descriptor.template ? descriptor.template.content : "";
    const t = compileTemplate({ source: tpl, filename: f, id: "chk", compilerOptions: { bindingMetadata: s.bindings } });
    if (t.errors && t.errors.length) {
      errors += t.errors.length;
      t.errors.forEach((e) => console.log("TPL", f, e.message || String(e)));
    }
  } catch (e) {
    errors += 1;
    console.log("COMPILE", f, e.message);
  }
}

console.log(errors ? `FAILED ${errors}` : "FRONTEND SYNTAX OK");
process.exit(errors ? 1 : 0);
