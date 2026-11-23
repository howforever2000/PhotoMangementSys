/* 材质统一修复探针：验证创意工坊 .ai-panel 与相册管理 .dialog
   在 plain / liquid 两种材质下的 computed 值真正随 body.mat-* 切换。
   浏览器里没有 Tauri 运行时，用 addInitScript 桩掉 __TAURI_INTERNALS__。 */
const { chromium } = require("playwright");

const BASE = "http://localhost:1420";

const TAURI_STUB = `
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    plugins: {},
    transformCallback: (cb) => cb,
    invoke: (cmd, args) => {
      if (cmd === 'get_current_user') {
        return Promise.resolve({ id: 1, username: 'probe', email: 'probe@local', created_at: 0, avatar: null });
      }
      // 其余命令：数组型返回空数组，其余返回 null（探针只看样式，不看数据）
      if (/list|albums|photos|tags|folders|stats|settings/i.test(cmd)) return Promise.resolve([]);
      return Promise.resolve(null);
    },
  };
`;

(async () => {
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const page = await browser.newPage();
  await page.addInitScript(TAURI_STUB);
  const errors = [];
  page.on("pageerror", (e) => errors.push(`PAGEERROR: ${e.message}`));

  const setMaterial = async (m) => {
    await page.evaluate((mat) => {
      const raw = localStorage.getItem("pm-theme");
      const prefs = raw ? JSON.parse(raw) : {};
      prefs.material = mat;
      localStorage.setItem("pm-theme", JSON.stringify(prefs));
    }, m);
    await page.reload({ waitUntil: "networkidle" });
    await page.waitForTimeout(1800);
  };

  const surfaceReport = (sel, withAfter) => {
    return page.evaluate(([s, wa]) => {
      const el = document.querySelector(s);
      if (!el) return "MISSING";
      const cs = getComputedStyle(el);
      const out = {
        bg: cs.backgroundColor,
        blur: cs.backdropFilter,
        shadowHasInset: cs.boxShadow.includes("inset"),
      };
      if (wa) {
        const after = getComputedStyle(el, "::after");
        out.afterContent = after.content;
        out.bgImageLayers = (cs.backgroundImage.match(/gradient/g) || []).length;
      }
      return out;
    }, [sel, withAfter]);
  };

  const report = {};

  /* ---------- 创意工坊：.ai-panel ---------- */
  await page.goto(`${BASE}/workshop`, { waitUntil: "networkidle", timeout: 30000 });
  await page.waitForTimeout(2000);
  report.wsRendered = await page.evaluate(
    () => !!document.querySelector(".ws-page") && !!document.querySelector(".ai-panel"),
  );

  await setMaterial("plain");
  report["plain bodyClass"] = await page.evaluate(() => document.body.className);
  report["plain .ai-panel"] = await surfaceReport(".ai-panel", true);

  await setMaterial("liquid");
  report["liquid bodyClass"] = await page.evaluate(() => document.body.className);
  report["liquid .ai-panel"] = await surfaceReport(".ai-panel", true);

  /* ---------- 相册管理：.dialog ---------- */
  await page.goto(`${BASE}/albums`, { waitUntil: "networkidle", timeout: 30000 });
  await page.waitForTimeout(2000);
  report.albumsRendered = await page.evaluate(() => !!document.querySelector(".album-page"));
  const newBtn = page.locator('button:has-text("新建相册")').first();
  report.foundCreateBtn = (await newBtn.count()) > 0;
  if (report.foundCreateBtn) {
    await newBtn.click();
    await page.waitForTimeout(500);
    report["liquid .dialog"] = await surfaceReport(".dialog", true);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);
  }

  await setMaterial("plain");
  if (report.foundCreateBtn) {
    await page.locator('button:has-text("新建相册")').first().click();
    await page.waitForTimeout(500);
    report["plain .dialog"] = await surfaceReport(".dialog", false);
  }

  console.log(JSON.stringify(report, null, 2));
  console.log("页面错误数:", errors.length);
  errors.slice(0, 5).forEach((e) => console.log("  ", e.slice(0, 200)));
  await browser.close();
})();
