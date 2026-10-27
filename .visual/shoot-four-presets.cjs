/* FEAT-119 验收：点**真实预设卡**（不是写 localStorage）→ 截 Home，证明代码改动生效 */
const fs = require('fs'), path = require('path');
const { chromium } = require('playwright');
const PORT = process.env.VITE_PORT || 1519;
const CHROME = process.env.PW_CHROME || '/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome';
const WANT = ['藏青夜', '松夜绿', '晨雾青瓷', '陶砂'];
const OUT = 'design/preview/preset-shots';
const SHEET = 'design/preview/four-presets.png';

(async () => {
  fs.mkdirSync(OUT, { recursive: true });
  const b = await chromium.launch({ executablePath: CHROME, args: ['--no-sandbox'] });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
  const errs = [];
  p.on('pageerror', e => errs.push(String(e).split('\n')[0]));
  await p.goto(`http://localhost:${PORT}/home?mock=true`, { waitUntil: 'load' });
  await p.waitForTimeout(1500);
  await p.click('[title="主题 / 皮肤设置"]');
  await p.waitForTimeout(700);
  const cards = await p.evaluate(() => [...document.querySelectorAll('.pm-preset-name > span:first-child')].map(e => e.textContent.trim()));
  console.log('弹窗里的预设卡:', JSON.stringify(cards));

  const shots = [];
  for (const name of WANT) {
    // 每次重新开弹窗（点预设后弹窗保持开着，这里统一先关再开，避免状态串）
    const opened = await p.evaluate(() => !!document.querySelector('.pm-dialog'));
    if (!opened) { await p.click('[title="主题 / 皮肤设置"]'); await p.waitForTimeout(500); }
    const ok = await p.evaluate((n) => {
      const btn = [...document.querySelectorAll('.pm-preset')].find(e => {
        const t = e.querySelector('.pm-preset-name > span:first-child');
        return t && t.textContent.trim() === n;
      });
      if (!btn) return false;
      btn.click(); return true;
    }, name);
    if (!ok) { console.log('找不到卡片:', name); continue; }
    await p.waitForTimeout(500);
    await p.keyboard.press('Escape');
    await p.waitForTimeout(900);
    const file = path.join(OUT, `four-${name}.png`);
    await p.screenshot({ path: file });
    const info = await p.evaluate(() => {
      const raw = JSON.parse(localStorage.getItem('pm-theme') || '{}');
      return { mode: raw.mode, comp: raw.compColor, from: raw.gradFrom, mid: raw.gradMid, to: raw.gradTo };
    });
    shots.push({ name, file, ...info });
    console.log('shot', name, JSON.stringify(info));
    await p.click('[title="主题 / 皮肤设置"]'); await p.waitForTimeout(500);
  }
  await b.close();

  const img64 = f => fs.readFileSync(f).toString('base64');
  const cells = shots.map(s => `<figure>
    <figcaption>
      <b>${s.name}</b>
      <span class="meta">${s.mode === 'dark' ? '深色' : '浅色'} · 组件 ${s.comp}</span>
      <span class="ramp">${[s.from, s.mid, s.to].map(c => `<i style="background:${c}"></i>`).join('')}</span>
    </figcaption>
    <img src="data:image/png;base64,${img64(s.file)}" alt="${s.name}">
  </figure>`).join('');

  const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><style>
  *{box-sizing:border-box}
  body{margin:0;padding:26px 28px 30px;background:#0c0c10;color:#eceaf0;
       font:14px/1.5 -apple-system,"Segoe UI","PingFang SC","Microsoft YaHei",sans-serif}
  h1{margin:0 0 3px;font-size:19px;letter-spacing:-.2px}
  .sub{margin:0 0 20px;font-size:12.5px;color:#9a97a6}
  .grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:18px}
  figure{margin:0}
  figcaption{display:flex;align-items:center;gap:9px;margin-bottom:6px}
  figcaption b{font-size:13px;font-weight:650}
  .meta{font-size:11px;color:#8d8a99}
  .ramp{display:flex;gap:3px;margin-left:auto}
  .ramp i{width:16px;height:10px;border-radius:2px;display:block;box-shadow:inset 0 0 0 1px rgba(255,255,255,.14)}
  img{display:block;width:100%;border-radius:9px;box-shadow:0 0 0 1px rgba(255,255,255,.09),0 8px 22px -16px #000}
</style></head><body>
  <h1>最终四套 · 真实点击预设卡后的 Home 页</h1>
  <p class="sub">藏青夜 / 松夜绿 / 晨雾青瓷 / 陶砂 · 色条 = 渐变三色标（上→下）</p>
  <div class="grid">${cells}</div>
  ${errs.length ? `<p style="color:#ff8080">控制台错误：${errs.slice(0,3).join(' | ')}</p>` : ''}
</body></html>`;
  const tmp = path.join(OUT, '_four.html');
  fs.writeFileSync(tmp, html);
  const b2 = await chromium.launch({ executablePath: CHROME, args: ['--no-sandbox'] });
  const sheet = await b2.newPage({ viewport: { width: 1560, height: 900 }, deviceScaleFactor: 1 });
  await sheet.goto('file://' + path.resolve(tmp), { waitUntil: 'load' });
  await sheet.waitForTimeout(400);
  await sheet.screenshot({ path: SHEET, fullPage: true });
  await b2.close();
  fs.unlinkSync(tmp);
  console.log('→', SHEET);
})();
