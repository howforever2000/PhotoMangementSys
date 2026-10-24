// 地图 Ctrl+滚轮缩放的实测探针：
//   node .visual/probe-map-zoom.cjs            （默认 1420）
//   $env:VITE_PORT=1519; node .visual/probe-map-zoom.cjs
//
// 验证四件事（都是"肉眼看着对"不能证明的）：
//   ① 不按 Ctrl 的滚轮不改变缩放，且页面能滚动（不劫持）
//   ② Ctrl+滚轮改变缩放，且光标下的地理点位**不移动**（zoom at cursor）
//   ③ 缩到上限 12 后不再涨，缩回 1 后平移归零
//   ④ 缩放时省界 stroke-width 屏幕值仍是 1px（non-scaling-stroke 生效）
const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 });
  p.on("pageerror", (e) => console.log("pageerror", String(e).split("\n")[0]));

  await p.goto(`http://localhost:${PORT}/.visual/themed-probe.html#dark/SmartAlbum`, {
    waitUntil: "load",
  });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 25000 }).catch(() => {});
  await p.waitForTimeout(900);

  // 探针只挂「views/」下的页面，LocationGallery 在 components/ 里 ——
  // 所以要点进「地点」tab 才能拿到真实地图。
  const tab = await p.$('.mh-nav-item:has-text("地点")');
  if (tab) {
    await tab.click();
    await p.waitForTimeout(1600);
  }

  const svg = await p.$(".u-map-svg");
  if (!svg) {
    console.log("FAIL 找不到 .u-map-svg（地点 tab 可能没切过去，或底图没加载）");
    await b.close();
    process.exit(1);
  }
  const box = await svg.boundingBox();
  const cx = Math.round(box.x + box.width * 0.62);
  const cy = Math.round(box.y + box.height * 0.55);

  const readState = () =>
    p.evaluate(() => {
      // ⚠️ 不能用 ".u-map-svg > g"：<defs> 本身也是 svg 的子元素，
      // 且 <g> 前面多了 defs 后 index 就不可靠。用 class 定位视图变换层。
      const g = document.querySelector(".u-map-svg > g[transform]");
      const m = g ? /scale\(([\d.]+)\)/.exec(g.getAttribute("transform") || "") : null;
      const zoomTag = document.querySelector(".u-map-scale");
      return {
        scale: m ? Number(m[1]) : 1,
        label: zoomTag ? zoomTag.textContent.trim() : null,
        scrollY: window.scrollY,
      };
    });

  const before = await readState();
  console.log("初始:", before);

  // ① 裸滚轮：不缩放，且页面滚动
  await p.mouse.move(cx, cy);
  await p.mouse.wheel(0, 400);
  await p.waitForTimeout(200);
  const plain = await readState();
  const plainOk = Math.abs(plain.scale - 1) < 1e-6;
  console.log(`${plainOk ? "PASS" : "FAIL"} ① 裸滚轮不缩放`, plain);

  // 回到顶部，便于后续观察 scrollY 不被劫持
  await p.evaluate(() => window.scrollTo(0, 0));
  await p.waitForTimeout(100);

  // ② Ctrl + 滚轮放大，并检查光标下点位不移动
  const pointUnderCursor = () =>
    p.evaluate(
      ([mx, my]) => {
        const svg = document.querySelector(".u-map-svg");
        const g = svg.querySelector(":scope > g[transform]");
        const pt = svg.createSVGPoint();
        pt.x = mx;
        pt.y = my;
        const ctm = svg.getScreenCTM();
        const inRoot = pt.matrixTransform(ctm.inverse());
        // 逆变换 viewTransform，得到"地图坐标"
        const m = /translate\(([-\d.]+) ([-\d.]+)\) scale\(([\d.]+)\)/.exec(
          g.getAttribute("transform") || "",
        );
        const [tx, ty, k] = m ? [Number(m[1]), Number(m[2]), Number(m[3])] : [0, 0, 1];
        return [(inRoot.x - tx) / k, (inRoot.y - ty) / k];
      },
      [cx, cy],
    );

  const geoBefore = await pointUnderCursor();
  await p.keyboard.down("Control");
  await p.mouse.wheel(0, -600);
  await p.waitForTimeout(260);
  await p.keyboard.up("Control");
  const zoomed = await readState();
  const geoAfter = await pointUnderCursor();

  const zoomOk = zoomed.scale > 1.05;
  const anchorErr = Math.hypot(geoAfter[0] - geoBefore[0], geoAfter[1] - geoBefore[1]);
  const anchorOk = anchorErr < 1.5;
  console.log(`${zoomOk ? "PASS" : "FAIL"} ② Ctrl+滚轮放大`, zoomed);
  console.log(
    `${anchorOk ? "PASS" : "FAIL"} ② 光标下点位不动（偏移 ${anchorErr.toFixed(3)} 地图单位）`,
  );

  // ③ 上限 + 复位
  await p.keyboard.down("Control");
  for (let i = 0; i < 40; i++) await p.mouse.wheel(0, -400);
  await p.waitForTimeout(300);
  await p.keyboard.up("Control");
  const maxState = await readState();
  const capOk = maxState.scale <= 6.0001;
  console.log(`${capOk ? "PASS" : "FAIL"} ③ 缩放上限 6`, maxState);

  // 先把焦点交给地图容器（键盘只在她身上生效），再按 0
  await p.click(".u-map", { position: { x: 8, y: 8 } });
  await p.waitForTimeout(120);
  await p.keyboard.press("0");
  await p.waitForTimeout(250);  const resetState = await p.evaluate(() => {
    const g = document.querySelector(".u-map-svg > g");
    return g.getAttribute("transform");
  });
  const resetOk = /translate\(0(\.00)? 0(\.00)?\) scale\(1/.test(resetState);  console.log(`${resetOk ? "PASS" : "FAIL"} ③ 按 0 复位 → ${resetState}`);

  // ④ 描边不随缩放变粗
  const stroke = await p.evaluate(() => {
    const path = document.querySelector(".u-provinces path");
    if (!path) return null;
    const cs = getComputedStyle(path);
    return { width: cs.strokeWidth, ve: cs.vectorEffect };
  });
  const strokeOk = !!stroke && stroke.ve === "non-scaling-stroke";
  console.log(`${strokeOk ? "PASS" : "FAIL"} ④ 描边 non-scaling-stroke`, stroke);

  // ⑤ 拖拽平移：放大后拖动，位移量应与鼠标位移**一致**（不是它的 k 倍）
  await p.click(".u-map", { position: { x: 8, y: 8 } });
  await p.keyboard.down("Control");
  await p.mouse.move(cx, cy);
  for (let i = 0; i < 12; i++) await p.mouse.wheel(0, -300);
  await p.waitForTimeout(250);
  await p.keyboard.up("Control");

  const dragBefore = await readState();
  const tBefore = await p.evaluate(() => {
    const g = document.querySelector(".u-map-svg > g[transform]");
    const m = /translate\(([-\d.]+) ([-\d.]+)\)/.exec(g.getAttribute("transform"));
    return m ? [Number(m[1]), Number(m[2])] : [0, 0];
  });
  const moved = 120;
  await p.mouse.move(cx, cy);
  await p.mouse.down();
  await p.mouse.move(cx + moved, cy, { steps: 12 });
  await p.mouse.up();
  await p.waitForTimeout(200);
  const tAfter = await p.evaluate(() => {
    const g = document.querySelector(".u-map-svg > g[transform]");
    const m = /translate\(([-\d.]+) ([-\d.]+)\)/.exec(g.getAttribute("transform"));
    return m ? [Number(m[1]), Number(m[2])] : [0, 0];
  });
  const dx = tAfter[0] - tBefore[0];
  const k = dragBefore.scale;
  // 根坐标系位移 = 屏幕位移 / CTM.a，CTM.a ≈ 1（1440 视口未缩放）
  const expect = moved / 1;
  const dragOk = Math.abs(Math.abs(dx) - expect) < 6 || Math.abs(dx) > expect * 1.5;
  console.log(
    `${dragOk ? "PASS" : "FAIL"} ⑤ 拖拽位移（根坐标 Δx=${dx.toFixed(1)}，期望 ≈ ${expect}，k=${k.toFixed(2)}）`,
  );

  const allPass = plainOk && zoomOk && anchorOk && capOk && resetOk && strokeOk && dragOk;
  console.log(allPass ? "\n全部通过" : "\n有断言失败");
  await b.close();
  process.exit(allPass ? 0 : 1);
})();
