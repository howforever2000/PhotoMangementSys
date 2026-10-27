const { chromium } = require('playwright');
const PORT = process.env.VITE_PORT || 1519;
(async () => {
  const b = await chromium.launch({ executablePath: '/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome' });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 } });
  const errs = [];
  p.on('pageerror', e => errs.push(String(e).split('\n')[0]));
  p.on('console', m => { if (m.type() === 'error') errs.push('console: ' + m.text().slice(0, 200)); });
  await p.goto(`http://localhost:${PORT}/home?mock=true`, { waitUntil: 'load' });
  await p.waitForTimeout(1800);
  await p.click('[title="主题 / 皮肤设置"]');
  await p.waitForTimeout(800);

  // 1) 预定义页：滚动到「我的预设」
  await p.evaluate(() => {
    const el = [...document.querySelectorAll('.pm-preset-group-title')].find(e => /我的预设/.test(e.textContent || ''));
    el?.scrollIntoView({ block: 'center' });
  });
  await p.waitForTimeout(400);
  await p.screenshot({ path: 'design/preview/theme-mine.png' });

  // 2) 自定义页 → 存为预设 → 命名
  await p.evaluate(() => [...document.querySelectorAll('[role="tab"]')].find(t => /自定义/.test(t.textContent))?.click());
  await p.waitForTimeout(500);
  await p.evaluate(() => {
    const btn = [...document.querySelectorAll('button')].find(b => /存为预设/.test(b.textContent || ''));
    btn?.click();
  });
  await p.waitForTimeout(500);
  await p.screenshot({ path: 'design/preview/theme-naming.png' });
  await p.fill('.pm-name-input', '深绿暮色');
  await p.evaluate(() => [...document.querySelectorAll('.pm-name-foot button')].find(b => /保存/.test(b.textContent))?.click());
  await p.waitForTimeout(600);
  console.log('保存后回到 tab:', await p.evaluate(() => document.querySelector('[role="tab"][aria-selected="true"]')?.textContent?.trim()));

  // 3) 色盘
  await p.evaluate(() => [...document.querySelectorAll('[role="tab"]')].find(t => /自定义/.test(t.textContent))?.click());
  await p.waitForTimeout(400);
  await p.evaluate(() => document.querySelector('.pm-swatch-btn')?.click());
  await p.waitForTimeout(700);
  await p.screenshot({ path: 'design/preview/theme-wheel.png' });
  console.log('色盘存在:', await p.evaluate(() => !!document.querySelector('.cp-ring')), '| 旧色块:', await p.evaluate(() => !!document.querySelector('.cp-grid')));
  console.log('我的预设数量:', await p.evaluate(() => JSON.parse(localStorage.getItem('pm-theme.customPresets') || '[]').length));
  console.log('错误:', errs.length ? errs.slice(0, 3) : '无');
  await b.close();
})();
