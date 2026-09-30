/**
 * 视觉验收取图（impeccable 一轮批次截图）
 *
 * Tauri 后端不在时，前端的所有 invoke 走 mock-init.js 注入的假数据，
 * 目的是把真实的 Vue 页面渲染出来截图，用于检查：
 *   三层递进（背景→容器→内容）、对比度、去彩虹化、去卡片化。
 * 纯开发工具，不参与应用构建。
 *
 * 用法：先起 dev server（npx vite，端口 1420），再 node .impeccable/visual-check.cjs
 */
const { chromium } = require("playwright");
const path = require("path");
const fs = require("fs");
const { INIT } = require("./mock-init.cjs");

const OUT = path.join(__dirname, "review");
fs.mkdirSync(OUT, { recursive: true });

const ROUTES = [
  ["home", "/home"],
  ["albums", "/albums"],
  ["scan", "/scan"],
  ["smart", "/smart"],
  ["memories", "/memories"],
  ["album-detail", "/album/1"],
];

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const errors = [];
  for (const [name, route] of ROUTES) {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
    const page = await ctx.newPage();
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(`[${name}] ${m.text()}`);
    });
    page.on("pageerror", (e) => errors.push(`[${name}] pageerror: ${e.message}`));
    await page.addInitScript(INIT);
    try {
      await page.goto(`http://localhost:1420${route}`, { waitUntil: "networkidle", timeout: 20000 });
    } catch (e) {
      errors.push(`[${name}] goto: ${e.message}`);
    }
    await page.waitForTimeout(1500);
    const finalUrl = page.url();
    if (!finalUrl.endsWith(route)) errors.push(`[${name}] redirected to ${finalUrl}`);
    await page.screenshot({ path: path.join(OUT, `${name}.png`), fullPage: false });
    console.log("captured:", name, "->", finalUrl);
    await ctx.close();
  }
  await browser.close();
  console.log("CONSOLE_ERRORS:", errors.length ? "\n" + [...new Set(errors)].slice(0, 15).join("\n") : "(none)");
})();

