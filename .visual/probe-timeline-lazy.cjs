const { chromium } = require('playwright');
const PORT = process.env.VITE_PORT || 1519;
(async () => {
  const b = await chromium.launch({ executablePath: '/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome' });
  const p = await b.newPage({ viewport: { width: 1440, height: 900 } });
  await p.goto(`http://localhost:${PORT}/timeline?mock=true`, { waitUntil: 'load' });
  await p.waitForTimeout(2500);
  const stat = () => p.evaluate(() => ({
    months: document.querySelectorAll('.tl-month').length,
    cards: document.querySelectorAll('.tl-card').length,
    more: !!document.querySelector('.tl-more'),
    docH: Math.round(document.documentElement.scrollHeight),
  }));
  console.log('首屏      ', await stat());
  for (let i = 0; i < 3; i++) {
    await p.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
    await p.waitForTimeout(700);
    console.log(`滚到底 #${i + 1}`, await stat());
  }
  await b.close();
})();
