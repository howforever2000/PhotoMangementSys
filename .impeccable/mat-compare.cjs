/**
 * 临时脚本：验证磨砂粗颗粒与釉瓷开片在深/浅背景下的观感。
 */
const { chromium } = require("playwright");
const { INIT } = require("./mock-init.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  for (const [name, patch] of [
    ["frosted-dark", { bgStyle: "color", compColor: "#16443a", compAlpha: 0.42, material: "frosted" }],
    ["frosted-light", { bgStyle: "gradient", gradFrom: "#ffd7d7", gradMid: "#7e9963", gradTo: "#195e53", compColor: "#f2f9f4", compAlpha: 0.55, material: "frosted" }],
    ["glazed-dark", { bgStyle: "color", compColor: "#16443a", compAlpha: 0.42, material: "glazed" }],
    ["glazed-light", { bgStyle: "gradient", gradFrom: "#ffd7d7", gradMid: "#7e9963", gradTo: "#195e53", compColor: "#f2f9f4", compAlpha: 0.55, material: "glazed" }],
  ]) {
    const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
    await page.addInitScript(INIT);
    await page.addInitScript((p) => {
      const prefs = JSON.parse(localStorage.getItem("pm-theme") || "{}");
      localStorage.setItem("pm-theme", JSON.stringify({ ...prefs, ...p }));
    }, patch);
    await page.goto("http://localhost:1420/home", { waitUntil: "domcontentloaded", timeout: 20000 });
    await page.waitForTimeout(2000);
    await page.screenshot({ path: `.impeccable/review/mat-${name}.png` });
    console.log("captured:", name);
    await page.context().close();
  }
  await browser.close();
})();
