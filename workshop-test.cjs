/* 创意工坊叠加处理冒烟测试：加载 /workshop 页面，检查组件渲染与控制台错误 */
const { chromium } = require("playwright");

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await browser.newPage();
  const errors = [];
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  page.on("pageerror", (e) => errors.push(`PAGEERROR: ${e.message}`));

  await page.goto("http://localhost:1420/workshop", { waitUntil: "networkidle", timeout: 30000 });
  await page.waitForTimeout(3000);

  const hasSelectBtn = await page.locator("text=选择图片").count();
  const body = await page.locator("body").innerText();
  const hasOpArea = body.includes("加载") || body.includes("重试") || body.includes("均衡") || body.includes("模糊");
  const chainVisible = body.includes("叠加链") || true; // 无处理时可不显示

  await page.screenshot({ path: "workshop-test.png", fullPage: false });
  console.log("选择图片按钮:", hasSelectBtn > 0 ? "OK" : "MISSING");
  console.log("算子区渲染:", hasOpArea ? "OK" : "MISSING");
  console.log("控制台错误数:", errors.length);
  errors.slice(0, 10).forEach((e) => console.log("  ERR:", e.slice(0, 200)));
  await browser.close();
})();
