/**
 * 釉瓷在不同组件透明度下的观感：0.9（最不透明，用户反馈场景）、0.42 默认、0.55 浅色预设。
 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  for (const [name, patch] of [
    ["a09-dark", { bgStyle: "color", compColor: "#16443a", compAlpha: 0.9, material: "glazed" }],
    ["a042-dark", { bgStyle: "color", compColor: "#16443a", compAlpha: 0.42, material: "glazed" }],
    ["a055-light", { bgStyle: "gradient", gradFrom: "#ffd7d7", gradMid: "#7e9963", gradTo: "#195e53", compColor: "#f2f9f4", compAlpha: 0.55, material: "glazed" }],
  ]) {
    const page = await (await browser.newContext({ viewport: { width: 1760, height: 920 } })).newPage();
    await page.addInitScript(INIT);
    await page.addInitScript((p) => {
      const prefs = JSON.parse(localStorage.getItem("pm-theme") || "{}");
      localStorage.setItem("pm-theme", JSON.stringify({ ...prefs, ...p }));
    }, patch);
    await page.goto("http://localhost:1420/home", { waitUntil: "domcontentloaded", timeout: 20000 });
    await page.waitForTimeout(2200);
    await page.screenshot({ path: `.impeccable/review/glv-${name}.png` });
    console.log("captured:", name);
    await page.context().close();
  }
  await browser.close();
})();
