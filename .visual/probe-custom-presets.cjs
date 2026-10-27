/* 自定义预设端到端：存 6 个 → 第 7 个入口必须禁用且按钮文案说明原因 → 删一个后恢复 */
const { chromium } = require('playwright');
const PORT = process.env.VITE_PORT || 1519;
(async () => {
  const b = await chromium.launch({ executablePath: '/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome' });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 } });
  const errs = [];
  p.on('pageerror', e => errs.push(String(e).split('\n')[0]));
  await p.goto(`http://localhost:${PORT}/home?mock=true`, { waitUntil: 'load' });
  await p.waitForTimeout(1500);
  await p.evaluate(() => localStorage.removeItem('pm-theme.customPresets'));
  await p.reload({ waitUntil: 'load' });
  await p.waitForTimeout(1500);
  await p.click('[title="主题 / 皮肤设置"]');
  await p.waitForTimeout(600);

  const saveBtn = () => p.evaluate(() => {
    const b = [...document.querySelectorAll('.pm-dialog button')].find(x => /存为预设|预设已满/.test(x.textContent || ''));
    return b ? { text: b.textContent.trim(), disabled: b.disabled } : null;
  });
  const store = () => p.evaluate(() => JSON.parse(localStorage.getItem('pm-theme.customPresets') || '[]'));

  for (let i = 1; i <= 6; i++) {
    await p.evaluate(() => [...document.querySelectorAll('.pm-dialog button')].find(x => /存为预设/.test(x.textContent||''))?.click());
    await p.waitForTimeout(280);
    await p.fill('.pm-name-input', `预设${i}`);
    await p.evaluate(() => [...document.querySelectorAll('.pm-name-foot button')].find(b => /保存/.test(b.textContent))?.click());
    await p.waitForTimeout(320);
  }
  console.log('存 6 个后:', JSON.stringify(await saveBtn()), '| 库存:', (await store()).length);
  console.log('卡片数:', await p.evaluate(() => document.querySelectorAll('.pm-preset-custom').length));
  console.log('分组标题:', await p.evaluate(() => {
    const t = [...document.querySelectorAll('.pm-preset-group-title')].find(e => /我的预设/.test(e.textContent||''));
    return t?.parentElement?.querySelector('.pm-preset-group-note')?.textContent?.trim();
  }));

  // 满了之后再点（按钮已 disabled，直接强行触发 saveCustom 的路径也要挡住）
  await p.evaluate(() => [...document.querySelectorAll('.pm-dialog button')].find(x => /预设已满/.test(x.textContent||''))?.click());
  await p.waitForTimeout(300);
  console.log('满后点入口 → 浮层:', await p.evaluate(() => !!document.querySelector('.pm-name-mask')), '| 库存仍:', (await store()).length);

  // 删除一个
  await p.evaluate(() => document.querySelector('.pm-preset-del')?.click());
  await p.waitForTimeout(400);
  console.log('删一个后:', JSON.stringify(await saveBtn()), '| 库存:', (await store()).length);
  console.log('错误:', errs.length ? errs.slice(0,2) : '无');
  await p.evaluate(() => localStorage.removeItem('pm-theme.customPresets'));
  await b.close();
})();
