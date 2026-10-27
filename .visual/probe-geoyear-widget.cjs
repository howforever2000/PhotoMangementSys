/* FEAT-120 回归探针：概览「地点 / 年份」两栏块
   跑法：npx vite --port 1519 & 然后 node .visual/probe-geoyear-widget.cjs
   产出：.visual/shots/geoyear/sheet.png（深色 / 浅色 / 壁纸 三套并排）+ full-wall.png（整墙）
   数据来自 .visual/overview-probe.ts 的真实形状 mock（2017–2026 逐年增长）——
   不用 ?mock=true，那份 mock 的年份单一、地点聚合也比总数还大，看不出柱阵形状。 */
const fs = require('fs'), path = require('path');
const { chromium } = require('playwright');
const CHROME = process.env.PW_CHROME || '/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome';
const PORT = process.env.VITE_PORT || 1519;
const BASE = `http://localhost:${PORT}/.visual/overview-probe.html`;
/* 输出落到 .visual/shots/（已被 .gitignore 排除），不往仓库里塞截图 */
const OUT = '.visual/shots/geoyear';
fs.mkdirSync(OUT, { recursive: true });

(async () => {
  const b = await chromium.launch({ executablePath: CHROME, args: ['--no-sandbox'] });
  const shots = [];
  for (const [key, theme] of [['dark', 'dusk'], ['light', 'light'], ['wall', 'wall']]) {
    const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 2 });
    await p.goto(`${BASE}#${theme}/SmartAlbum`, { waitUntil: 'load' });
    await p.waitForTimeout(3000);
    await p.evaluate(() => document.querySelectorAll('button').forEach((x) => x.textContent.trim() === '全局概览' && x.click()));
    await p.waitForTimeout(2500);
    const handle = await p.evaluateHandle(() => [...document.querySelectorAll('.ui-widget')].find((e) => e.querySelector('.ov-split')) ?? null);
    const box = await handle.boundingBox();
    if (box) {
      const f = path.join(OUT, `${key}.png`);
      await p.screenshot({ path: f, clip: { x: box.x - 10, y: box.y - 10, width: box.width + 20, height: box.height + 20 } });
      shots.push([key, f]);
    }
    await p.close();
  }
  const p2 = await b.newPage({ viewport: { width: 1440, height: 1100 } });
  await p2.goto(`${BASE}#dusk/SmartAlbum`, { waitUntil: 'load' });
  await p2.waitForTimeout(3000);
  await p2.evaluate(() => document.querySelectorAll('button').forEach((x) => x.textContent.trim() === '全局概览' && x.click()));
  await p2.waitForTimeout(2500);
  await p2.screenshot({ path: path.join(OUT, 'full-wall.png') });
  await b.close();

  const html = `<!doctype html><html><head><meta charset="utf-8"><style>
  body{margin:0;padding:22px 24px;background:#0c0c10;color:#eceaf0;font:14px/1.5 -apple-system,"PingFang SC",sans-serif}
  h1{margin:0 0 16px;font-size:17px}
  .grid{display:grid;grid-template-columns:repeat(2,1fr);gap:16px}
  figure{margin:0}figcaption{font-size:12px;color:#9a97a6;margin-bottom:6px}
  img{width:100%;display:block;border-radius:9px;box-shadow:0 0 0 1px rgba(255,255,255,.09)}
  </style></head><body><h1>地点 / 年份 两栏 · 深色 / 浅色 / 壁纸</h1><div class="grid">
  ${shots.map(([k, f]) => `<figure><figcaption>${k}</figcaption><img src="data:image/png;base64,${fs.readFileSync(f).toString('base64')}"></figure>`).join('')}
  </div></body></html>`;
  const tmp = '.visual/_geo3.html';
  fs.writeFileSync(tmp, html);
  const b2 = await chromium.launch({ executablePath: CHROME, args: ['--no-sandbox'] });
  const sp = await b2.newPage({ viewport: { width: 1500, height: 800 }, deviceScaleFactor: 1 });
  await sp.goto('file://' + path.resolve(tmp), { waitUntil: 'load' });
  await sp.waitForTimeout(400);
  await sp.screenshot({ path: path.join(OUT, 'sheet.png'), fullPage: true });
  await b2.close();
  fs.unlinkSync(tmp);
  console.log('→', OUT, '（sheet.png / full-wall.png / dark|light|wall.png）');
})();
