/**
 * 饱和度档位下的对比度机测：复用 audit-contrast.cjs 的全部判定逻辑，
 * 只在加载前注入 saturation 偏好。
 * 用法：node .impeccable\audit-sat.cjs 0.4
 */
const fs = require("fs");
const path = require("path");

const sat = Number(process.argv[2] ?? 1);
const src = fs.readFileSync(path.join(__dirname, "audit-contrast.cjs"), "utf8");

const anchor = "    await page.addInitScript(INIT);";
if (!src.includes(anchor)) {
  console.error("anchor not found in audit-contrast.cjs");
  process.exit(2);
}
const patched = src.replace(
  anchor,
  `${anchor}\n    await page.addInitScript((s) => {\n      const p = JSON.parse(localStorage.getItem("pm-theme") || "{}");\n      localStorage.setItem("pm-theme", JSON.stringify({ ...p, saturation: s }));\n    }, ${sat});`,
);

const tmp = path.join(__dirname, `.audit-sat-${String(sat).replace(".", "_")}.cjs`);
fs.writeFileSync(tmp, patched, "utf8");
require(tmp);
