/**
 * 临时脚本：对比浅色预设（粉绿晨光，磨砂）与默认深色下的玻璃边缘质感。
 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  for (const [name, patch] of [
    ["light-preset", { bgStyle: "gradient", gradFrom: "#ffd7d7", gradMid: "#7e9963", gradTo: "#195e53", compColor: "#f2f9f4", compAlpha: 0.55, material: "frosted" }],
    ["dark-default", { bgStyle: "color", compColor: "#16443a", compAlpha: 0.42, material: "frosted" }],
  ]) {
    const page = await (await browser.newContext({ viewport: { width: 1760, height: 920 } })).newPage();
    await page.addInitScript(INIT);
    await page.addInitScript((p) => {
      const prefs = JSON.parse(localStorage.getItem("pm-theme") || "{}");
      localStorage.setItem("pm-theme", JSON.stringify({ ...prefs, ...p }));
    }, patch);
    await page.goto("http://localhost:1420/home", { waitUntil: "domcontentloaded", timeout: 15000 });
    await page.waitForTimeout(2200);
    await page.screenshot({ path: `.impeccable/review/edge-${name}.png` });
    console.log("captured:", name);
    await page.context().close();
  }
  await browser.close();
})();
