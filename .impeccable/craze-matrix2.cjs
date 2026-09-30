/**
 * 开片形态第二弹：连续细线管线对比。
 * E) turbulence 脊线阈值化  F) fractalNoise 等值线(discrete)  G) 等值线窄带(matrix)
 */
const { chromium } = require("playwright");

const E = `data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='260'%3E%3Cfilter id='e'%3E%3CfeTurbulence type='turbulence' baseFrequency='0.035' numOctaves='2' seed='8'/%3E%3CfeColorMatrix type='matrix' values='0 0 0 0 0.85 0 0 0 0 0.9 0 0 0 0 0.86 1.6 1.6 1.6 0 -2.1'/%3E%3C/filter%3E%3Crect width='260' height='260' filter='url(%23e)'/%3E%3C/svg%3E`;

const F = `data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='260'%3E%3Cfilter id='f'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.013' numOctaves='1' seed='8'/%3E%3CfeColorMatrix type='matrix' values='0 0 0 0 0.85 0 0 0 0 0.9 0 0 0 0 0.86 1 0 0 0 -0.485'/%3E%3C/filter%3E%3Crect width='260' height='260' filter='url(%23f)'/%3E%3C/svg%3E`;

const G = `data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='260'%3E%3Cfilter id='g'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.011' numOctaves='1' seed='8'/%3E%3CfeColorMatrix type='matrix' values='0 0 0 0 1 0 0 0 0 1 0 0 0 0 1 1 0 0 0 0'/%3E%3CfeComponentTransfer%3E%3CfeFuncA type='discrete' tableValues='0 0 0 1 0 0 0'/%3E%3C/feComponentTransfer%3E%3CfeColorMatrix type='matrix' values='1 0 0 0 0 0 1 0 0 0 0 0 1 0 0 0 0 0 0.9 0'/%3E%3C/filter%3E%3Crect width='260' height='260' filter='url(%23g)'/%3E%3C/svg%3E`;

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await browser.newPage({ viewport: { width: 900, height: 340 } });
  const imgs = [
    ["E turbulence脊线", E],
    ["F 等值线窄带", F],
    ["G discrete等值线", G],
  ]
    .map(([label, uri]) => `<figure style="margin:0;text-align:center"><img src="${uri}" style="width:290px;height:290px;object-fit:cover;background:#16443a"><figcaption style="color:#fff;font:12px sans-serif">${label}</figcaption></figure>`)
    .join("");
  await page.setContent(`<body style="margin:0;background:#0e211b;display:flex;gap:4px;justify-content:center">${imgs}</body>`);
  await page.waitForTimeout(600);
  await page.screenshot({ path: ".impeccable/review/craze-matrix2.png" });
  console.log("done");
  await browser.close();
})();
