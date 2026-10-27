// 全界面稿截图
const { chromium } = require("playwright");
const path = require("path");

const OUT = "D:/YUAN HAO/Documents/PersonalIdeaProject/PhotoMangementSys/design/preview";
const FILE =
  "file:///D:/YUAN%20HAO/Documents/PersonalIdeaProject/PhotoMangementSys/design/preview/all.html";
const TARGETS = [
  ["s1", "all-01-home.png"],
  ["s2", "all-02-albums.png"],
  ["s3", "all-03-album-detail.png"],
  ["s4", "all-04-smart-people.png"],
  ["s5", "all-05-timeline.png"],
  ["s6", "all-06-memories.png"],
  ["s7", "all-07-places.png"],
  ["s8", "all-08-semantic.png"],
  ["s9", "all-09-search.png"],
  ["s10", "all-10-image-search.png"],
  ["s11", "all-11-workshop.png"],
  ["s12", "all-12-scan.png"],
  ["s13", "all-13-lightbox.png"],
  ["s14", "all-14-theme.png"],
  ["s15", "all-15-albums-light.png"],
  ["s16", "all-16-smart-light.png"],
];

(async () => {
  const browser = await chromium.launch({ channel: "msedge" });
  const page = await browser.newPage({
    viewport: { width: 1480, height: 1200 },
    deviceScaleFactor: 1,
  });
  await page.goto(FILE, { waitUntil: "load", timeout: 30000 });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(800);
  for (const [id, out] of TARGETS) {
    const el = await page.$("#" + id);
    if (!el) { console.log("MISSING", id); continue; }
    await el.screenshot({ path: path.join(OUT, out) });
    console.log("shot", out);
  }
  await browser.close();
})();
