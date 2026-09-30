const { chromium } = require("playwright");
const path = require("path");
const fs = require("fs");
const INIT = fs.readFileSync(path.join(__dirname, "visual-check.cjs"), "utf8").split("const ROUTES")[0].match(/const INIT = `([\s\S]*?)`;/)[1]
  .replace("${JSON.stringify(ALBUMS)}", "[]");

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text()); });
  await page.addInitScript(INIT);
  await page.goto("http://localhost:1420/albums", { waitUntil: "networkidle", timeout: 20000 }).catch((e) => errs.push("goto " + e.message));
  await page.waitForTimeout(1500);
  console.log("URL:", page.url());
  console.log("BODY:", (await page.evaluate(() => document.body.innerText)).slice(0, 300).replace(/\n+/g, " | "));
  console.log("ERRORS:", errs.slice(0, 8).join("\n"));
  await browser.close();
})();
