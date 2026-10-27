// 折射是否真的生效：把顶栏压在**高频背景**上，比对「有折射 / 无折射」的像素差
//
// 判据不是"看起来像不像玻璃"（主观），而是：
//   同一位置、同一填充，唯一的差别是 backdrop-filter 里有没有 url(#liquid-lens)。
//   如果滤镜真的在工作，两种情况的**像素必然不同**。
// 做法：注入一层条纹背景，分别截图顶栏区域，算平均绝对差（MAE）。
const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1200, height: 800 }, deviceScaleFactor: 1 });
  p.on("pageerror", (e) => console.log("pageerror", String(e).split("\n")[0]));

  await p.goto(`http://localhost:${PORT}/.visual/themed-probe.html#dark/AlbumList`, { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 20000 }).catch(() => {});
  await p.waitForTimeout(1000);

  // 注入高频背景（有条纹才能看出位移）
  await p.evaluate(() => {
    const d = document.createElement("div");
    d.id = "__stripe";
    d.style.cssText =
      "position:fixed;inset:0;z-index:0;pointer-events:none;" +
      "background:repeating-linear-gradient(45deg,#fff 0 8px,#0a2 8px 16px," +
      "#f80 16px 24px,#08f 24px 32px);opacity:.95";
    document.body.insertBefore(d, document.body.firstChild);
    // 让内容层不要盖住条纹：只留顶栏
    document.querySelectorAll(".u-statband, .album-layout").forEach((el) => {
      if (el) el.style.visibility = "hidden";
    });
  });
  await p.waitForTimeout(300);

  const box = await p.evaluate(() => {
    const el = document.querySelector(".mh-inner");
    const r = el.getBoundingClientRect();
    return { x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) };
  });

  const shot = async (name) => {
    const buf = await p.screenshot({ clip: box });
    return buf;
  };

  // A：带折射（当前生产样式）
  const a = await shot("refract");

  // B：去掉 url(...)，只保留 blur
  await p.evaluate(() => {
    const el = document.querySelector(".mh-inner");
    const cs = getComputedStyle(el);
    el.dataset.saved = cs.backdropFilter;
    el.style.backdropFilter = `blur(8px) saturate(1.1)`;
    el.style.webkitBackdropFilter = `blur(8px) saturate(1.1)`;
  });
  await p.waitForTimeout(250);
  const bl = await shot("blur-only");

  // C：完全去掉 backdrop-filter（对照：证明差异确实来自滤镜而不是填充）
  await p.evaluate(() => {
    const el = document.querySelector(".mh-inner");
    el.style.backdropFilter = "none";
    el.style.webkitBackdropFilter = "none";
  });
  await p.waitForTimeout(250);
  const none = await shot("no-filter");

  const { PNG } = (() => {
    try {
      return require("pngjs");
    } catch {
      return {};
    }
  })();

  const decode = async (buf) => {
    // 不引依赖：用 Chromium 自己解 PNG -- 把 buffer 转 dataURL 再画到 canvas 取像素
    return p.evaluate(async (dataUrl) => {
      const img = new Image();
      img.src = dataUrl;
      await img.decode();
      const c = document.createElement("canvas");
      c.width = img.width;
      c.height = img.height;
      const ctx = c.getContext("2d");
      ctx.drawImage(img, 0, 0);
      return Array.from(ctx.getImageData(0, 0, c.width, c.height).data);
    }, `data:image/png;base64,${buf.toString("base64")}`);
  };

  const [pa, pb, pc] = [await decode(a), await decode(bl), await decode(none)];

  const mae = (x, y) => {
    let s = 0;
    for (let i = 0; i < x.length; i += 4) {
      s += Math.abs(x[i] - y[i]) + Math.abs(x[i + 1] - y[i + 1]) + Math.abs(x[i + 2] - y[i + 2]);
    }
    return +(s / (x.length / 4) / 3).toFixed(2);
  };

  const d1 = mae(pa, pb); // 折射 vs 纯模糊
  const d2 = mae(pb, pc); // 纯模糊 vs 无滤镜
  console.log(JSON.stringify({
    顶栏区域: box,
    "平均绝对差 · 折射 vs 纯模糊": d1,
    "平均绝对差 · 纯模糊 vs 无滤镜": d2,
  }, null, 1));

  const ok = d1 > 0.5 && d2 > 0.5;
  console.log(
    ok
      ? "\nPASS 折射与纯模糊的像素**明显不同** → url(#liquid-lens) 确实在参与渲染"
      : "\nFAIL 两者几乎一致 → 滤镜没有被解析（检查 <GlassFilters /> 是否挂在 App 根）",
  );
  await b.close();
  process.exit(ok ? 0 : 1);
})();
