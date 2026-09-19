// 用 Playwright 打开前端页面，收集控制台错误并截图
const { chromium } = require("playwright");

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const errors = [];
  page.on("console", (m) => {
    if (m.type() === "error" || m.type() === "warning") errors.push(m.type() + ": " + m.text());
  });
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
  await page.goto("http://localhost:1420/", { waitUntil: "networkidle", timeout: 20000 }).catch((e) => {
    errors.push("goto: " + e.message);
  });
  await page.waitForTimeout(3000);
  const title = await page.title();
  const text = (await page.evaluate(() => document.body.innerText.slice(0, 400))).replace(/\n+/g, " | ");
  await page.screenshot({ path: "D:/YUAN HAO/Documents/workbubby/Claw/.workbuddy/browser-test.png" });
  console.log("TITLE:", title);
  console.log("BODY:", text);
  console.log("CONSOLE:", errors.length ? errors.slice(0, 10).join("\n") : "(none)");
  await browser.close();
})();
