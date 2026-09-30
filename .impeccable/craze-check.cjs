/**
 * 临时脚本：直接渲染开片 SVG 贴图本身，验证裂纹网形态（不经过 CSS 叠加）。
 * 深底/浅底各铺一次，看 soft-light 下的实际观感。
 */
const { chromium } = require("playwright");

const CRAZE =
  "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='260' height='260'%3E%3Cfilter id='c'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.05' numOctaves='2' seed='8'/%3E%3CfeConvolveMatrix order='3' preserveAlpha='true' kernelMatrix='0 -1 0 -1 4 -1 0 -1 0'/%3E%3CfeColorMatrix type='matrix' values='0 0 0 0 0.85 0 0 0 0 0.9 0 0 0 0 0.86 1.2 1.2 1.2 0 -0.18'/%3E%3C/filter%3E%3Crect width='260' height='260' filter='url(%23c)'/%3E%3C/svg%3E";

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await browser.newPage({ viewport: { width: 900, height: 520 } });
  await page.setContent(`
    <body style="margin:0;display:grid;grid-template-columns:1fr 1fr 1fr;height:100vh">
      <div style="background:#16443a"></div>
      <div style="background:#a8c3ae"></div>
      <div style="background:#16443a"></div>
    </body>`);
  await page.waitForTimeout(300);
  // 直接看贴图（左：贴图原样 叠在深底；中：浅底；右：纯深底对照）——用 screenshot clip 检查
  await page.evaluate((url) => {
    const divs = document.querySelectorAll("div");
    divs[0].style.backgroundImage = `url("${url}")`;
    divs[1].style.backgroundImage = `url("${url}")`;
  }, CRAZE);
  await page.waitForTimeout(400);
  await page.screenshot({ path: ".impeccable/review/craze-raw.png" });
  console.log("done");
  await browser.close();
})();
