/**
 * 排查开片滤镜：A) 完整版（turbulence→convolve→colorMatrix）
 * B) 只到 convolve（看 Laplacian 输出形态）
 * C) 只 turbulence（对照）
 */
const { chromium } = require("playwright");

const A =
  "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='260'%3E%3Cfilter id='c'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.05' numOctaves='2' seed='8'/%3E%3CfeConvolveMatrix order='3' preserveAlpha='true' kernelMatrix='0 -1 0 -1 4 -1 0 -1 0'/%3E%3CfeColorMatrix type='matrix' values='0 0 0 0 0.85 0 0 0 0 0.9 0 0 0 0 0.86 1.2 1.2 1.2 0 -0.18'/%3E%3C/filter%3E%3Crect width='260' height='260' filter='url(%23c)'/%3E%3C/svg%3E";
const B =
  "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='260'%3E%3Cfilter id='b'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.05' numOctaves='2' seed='8'/%3E%3CfeConvolveMatrix order='3' preserveAlpha='true' kernelMatrix='0 -1 0 -1 4 -1 0 -1 0'/%3E%3C/filter%3E%3Crect width='260' height='260' filter='url(%23b)'/%3E%3C/svg%3E";
const C =
  "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='260'%3E%3Cfilter id='t'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.05' numOctaves='2' seed='8'/%3E%3C/filter%3E%3Crect width='260' height='260' filter='url(%23t)'/%3E%3C/svg%3E";

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await browser.newPage({ viewport: { width: 840, height: 300 } });
  await page.setContent(`
    <body style="margin:0;display:flex;height:100vh;background:#222">
      <img src="${A}" style="width:280px;height:300px;object-fit:cover">
      <img src="${B}" style="width:280px;height:300px;object-fit:cover">
      <img src="${C}" style="width:280px;height:300px;object-fit:cover">
    </body>`);
  await page.waitForTimeout(500);
  await page.screenshot({ path: ".impeccable/review/craze-debug.png" });
  console.log("done");
  await browser.close();
})();
