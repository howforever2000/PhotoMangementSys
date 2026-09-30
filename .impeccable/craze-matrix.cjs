/**
 * 开片参数矩阵：不同 baseFrequency × alpha 增益 并排对比，直接渲染贴图。
 */
const { chromium } = require("playwright");

function craze(bf, gain, bias, seed = 8) {
  return `data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='260'%3E%3Cfilter id='c'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='${bf}' numOctaves='2' seed='${seed}'/%3E%3CfeConvolveMatrix order='3' preserveAlpha='true' kernelMatrix='0 -1 0 -1 4 -1 0 -1 0'/%3E%3CfeColorMatrix type='matrix' values='0 0 0 0 0.85 0 0 0 0 0.9 0 0 0 0 0.86 ${gain} ${gain} ${gain} 0 ${bias}'/%3E%3C/filter%3E%3Crect width='260' height='260' filter='url(%23c)'/%3E%3C/svg%3E`;
}

const cells = [
  ["bf.08 g1.5", craze("0.08", 1.5, -0.15)],
  ["bf.14 g2", craze("0.14", 2, -0.25)],
  ["bf.2 g2.5", craze("0.2", 2.5, -0.35)],
  ["bf.14 g3", craze("0.14", 3, -0.45)],
  ["bf.2 g4", craze("0.2", 4, -0.7)],
  ["bf.28 g5", craze("0.28", 5, -1.1)],
];

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await browser.newPage({ viewport: { width: 900, height: 460 } });
  const imgs = cells
    .map(([label, uri]) => `<figure style="margin:0;text-align:center"><img src="${uri}" style="width:280px;height:280px;object-fit:cover;background:#16443a"><figcaption style="color:#fff;font:12px sans-serif">${label}</figcaption></figure>`)
    .join("");
  await page.setContent(`<body style="margin:0;background:#0e211b;display:flex;flex-wrap:wrap;gap:4px;justify-content:center">${imgs}</body>`);
  await page.waitForTimeout(600);
  await page.screenshot({ path: ".impeccable/review/craze-matrix.png", fullPage: true });
  console.log("done");
  await browser.close();
})();
