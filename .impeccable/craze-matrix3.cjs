/**
 * 开片形态第三弹：table 窄峰等值线（21/31 点峰），频率与峰位微调。
 */
const { chromium } = require("playwright");

function contour(bf, points, peakIdx, seed = 8) {
  const tv = Array.from({ length: points }, (_, i) => (i === peakIdx ? 1 : 0)).join(" ");
  return `data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='260'%3E%3Cfilter id='c'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='${bf}' numOctaves='1' seed='${seed}'/%3E%3CfeColorMatrix type='matrix' values='0 0 0 0 0.92 0 0 0 0 0.95 0 0 0 0 0.9 1 0 0 0 0'/%3E%3CfeComponentTransfer%3E%3CfeFuncA type='table' tableValues='${tv}'/%3E%3C/feComponentTransfer%3E%3C/filter%3E%3Crect width='260' height='260' filter='url(%23c)'/%3E%3C/svg%3E`;
}

const cells = [
  ["bf.011 21pt", contour("0.011", 21, 10)],
  ["bf.018 31pt", contour("0.018", 31, 15)],
  ["bf.024 41pt", contour("0.024", 41, 20)],
  ["bf.018 31pt s3", contour("0.018", 31, 15, 3)],
];

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await browser.newPage({ viewport: { width: 900, height: 660 } });
  const imgs = cells
    .map(([label, uri]) => `<figure style="margin:0;text-align:center"><img src="${uri}" style="width:280px;height:280px;object-fit:cover;background:#16443a"><figcaption style="color:#fff;font:12px sans-serif">${label}</figcaption></figure>`)
    .join("");
  await page.setContent(`<body style="margin:0;background:#0e211b;display:flex;flex-wrap:wrap;gap:4px;justify-content:center">${imgs}</body>`);
  await page.waitForTimeout(600);
  await page.screenshot({ path: ".impeccable/review/craze-matrix3.png", fullPage: true });
  console.log("done");
  await browser.close();
})();
