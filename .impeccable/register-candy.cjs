/**
 * FEAT-087 补：糖果彩虹色板注册（script 侧）
 *
 * 为什么单独一个脚本：register-colors.cjs 只改 <style> 块（避免误伤 JS 里的
 * canvas 用色），但 hero 横幅 / 入口卡 / 回忆故事卡的糖果渐变是**在 <script>
 * 里拼的渐变字符串**，因此需要单独一轮、且仅限这三个已确认的视图文件。
 *
 * 替换后渐变串变成 linear-gradient(135deg, var(--candy-cream) 0%, ...)，
 * 由 theme store 的饱和度管线统一下发浓淡。
 *
 * 用法：node .impeccable/register-candy.cjs [--apply]
 */
const fs = require("fs");
const path = require("path");

const ROOT = path.join(__dirname, "..");
const TARGETS = [
  "src/views/ScanHub.vue",
  "src/views/SmartAlbum.vue",
  "src/views/Memories.vue",
];

const MAP = [
  ["#FFFCBD", "var(--candy-cream)"],
  ["#F075C7", "var(--candy-pink)"],
  ["#65D5F9", "var(--candy-blue)"],
  ["#FB6D9B", "var(--candy-rose)"],
  ["#505FDD", "var(--candy-indigo)"],
  ["#3A36E4", "var(--candy-violet)"],
];

const apply = process.argv.includes("--apply");
let total = 0;

for (const rel of TARGETS) {
  const file = path.join(ROOT, rel);
  let code = fs.readFileSync(file, "utf8");
  let n = 0;
  for (const [from, to] of MAP) {
    const parts = code.split(from);
    if (parts.length > 1) {
      n += parts.length - 1;
      code = parts.join(to);
    }
  }
  if (n) {
    total += n;
    console.log(`${rel}: ${n}`);
    if (apply) fs.writeFileSync(file, code, "utf8");
  }
}
console.log(`\n${apply ? "已写入" : "dry-run"}：${total} 处糖果基色注册`);
