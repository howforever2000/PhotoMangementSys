// 大图预览器（Lightbox）重排方案 · 效果图截图
// 用法：node design/preview/proposal/shoot-lightbox.cjs
const { chromium } = require("playwright");
const path = require("path");

const DIR = __dirname;
const TARGETS = [
  ["lightbox-a.html", "lightbox-a.png"],
  ["lightbox-b.html", "lightbox-b.png"],
  ["lightbox-c.html", "lightbox-c.png"],
];

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1 });
  for (const [src, out] of TARGETS) {
    const file = "file://" + path.join(DIR, src);
    try {
      await page.goto(file, { waitUntil: "load", timeout: 20000 });
    } catch (e) {
      console.log("skip", src, e.message);
      continue;
    }
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(250);
    await page.screenshot({ path: path.join(DIR, out) });
    console.log("shot", out);
  }
  await browser.close();
})();
