// FEAT-112 回归探针：node .visual/probe-feat112.cjs
// 验证：① 地图放大后点击城市能 emit pick（拖拽阈值修复）
//       ② 概览页点人物 widget 能切到人物分区（query watch 修复）
//       ③ 概览二次进入零 IPC（缓存生效）
const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 } });
  const errors = [];
  p.on("pageerror", (e) => errors.push(String(e).split("\n")[0]));
  p.on("framenavigated", () => {});

  await p.goto(`http://localhost:${PORT}/.visual/overview-probe.html#dusk`, { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
  await p.waitForTimeout(600);

  // ── ② 概览页点「人物」widget → 应切到人物分区 ──
  const beforeTab = await p.evaluate(() => location.hash);
  await p.locator(".ui-widget", { hasText: "按出现次数排序" }).first().click();
  await p.waitForTimeout(400);
  const after = await p.evaluate(() => ({
    hash: location.hash,
    hasPersonGallery: !!document.querySelector(".pg-wrap, .person-grid"),
  }));
  console.log("② 人物跳转:", after.hasPersonGallery ? "PASS" : "FAIL", JSON.stringify(after), "before=" + beforeTab);

  // ── ① 地图：放大后点城市 ──
  // 先切到地点分区
  await p.evaluate(() => {
    document.querySelectorAll(".mh-nav-item").forEach((el) => {
      if (el.textContent.trim().startsWith("地点")) el.click();
    });
  });
  await p.waitForTimeout(1200);
  const mapOk = await p.evaluate(() => !!document.querySelector(".u-map-svg"));
  if (mapOk) {
    // 用 Ctrl+滚轮放大到 2x
    const box = await p.locator(".u-map-svg").boundingBox();
    await p.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await p.keyboard.down("Control");
    await p.mouse.wheel(0, -200);
    await p.keyboard.up("Control");
    await p.waitForTimeout(400);
    const zoomed = await p.evaluate(() => {
      const el = document.querySelector(".u-map-svg");
      return el.classList.contains("zoomed") ? el.getAttribute("class") : null;
    });
    // 点一个城市光点
    const pts = p.locator(".u-point .dot");
    const n = await pts.count();
    let clicked = false;
    if (n > 0) {
      const pb = await pts.first().boundingBox();
      if (pb) {
        await p.mouse.click(pb.x + pb.width / 2, pb.y + pb.height / 2);
        await p.waitForTimeout(900);
        clicked = await p.evaluate(() => !!document.querySelector(".lg-detail-head, .lg-detail-title"));
      }
    }
    console.log("① 地图放大后点击:", clicked ? "PASS" : "FAIL", `zoomed=${zoomed} points=${n}`);
  } else {
    console.log("① 地图: SKIP（探针无地点数据）");
  }

  // ── ③ 缓存：切回概览应零 list_timeline 调用 ──
  let invokeCount = 0;
  await p.exposeFunction("__countInvoke", () => { invokeCount += 1; });
  await p.evaluate(() => {
    const orig = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === "list_timeline") window.__countInvoke();
      return orig(cmd, args);
    };
  });
  await p.evaluate(() => {
    document.querySelectorAll(".mh-nav-item").forEach((el) => {
      if (el.textContent.trim().startsWith("全局概览")) el.click();
    });
  });
  await p.waitForTimeout(300);
  await p.evaluate(() => {
    document.querySelectorAll(".mh-nav-item").forEach((el) => {
      if (el.textContent.trim().startsWith("地点")) el.click();
    });
  });
  await p.waitForTimeout(300);
  await p.evaluate(() => {
    document.querySelectorAll(".mh-nav-item").forEach((el) => {
      if (el.textContent.trim().startsWith("全局概览")) el.click();
    });
  });
  await p.waitForTimeout(600);
  console.log("③ 二次进入概览 list_timeline 次数:", invokeCount, invokeCount === 0 ? "PASS（缓存生效）" : "FAIL（仍在实时拉取）");

  console.log("页面错误:", errors.length ? errors.join(" | ") : "无");
  await b.close();
})();
