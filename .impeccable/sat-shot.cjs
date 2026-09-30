/**
 * 按指定饱和度截 home + scan 两页（FEAT-087 验证）。
 * 用法：node .impeccable\sat-shot.cjs <标签> <饱和度>
 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

const tag = process.argv[2] || "x";
const sat = Number(process.argv[3] ?? 1);

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  await page.addInitScript(INIT);
  await page.addInitScript((s) => {
    const prefs = JSON.parse(localStorage.getItem("pm-theme") || "{}");
    localStorage.setItem("pm-theme", JSON.stringify({ ...prefs, saturation: s }));
  }, sat);
  for (const [name, route] of [["home", "/home"], ["scan", "/scan"], ["smart", "/smart"], ["memories", "/memories"]]) {
    await page.goto(`http://localhost:1420${route}`, { waitUntil: "domcontentloaded", timeout: 20000 });
    await page.waitForTimeout(2000);
    await page.screenshot({ path: `.impeccable/review/sat-${tag}-${name}.png` });
  }
  console.log("captured:", tag, "sat=", sat);
  await browser.close();
})();
