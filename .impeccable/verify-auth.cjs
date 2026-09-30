/**
 * 验收：FEAT-093 三张鉴权页（/login /register /forgot-password）
 *
 * 与 audit-contrast.cjs 的分工：那份按「祖先链 + 兄弟遮罩 + 渐变插值」**推算**底色，
 * 遇到「背景是 photo（background-image: url）」「材质高光是伪元素」这两类就量不准
 * （它会退回兜底深色 → 假通过）。本脚本改为**量真实渲染像素**：
 *   1. 截图 → 把 PNG 作为 data URL 送回页面 → canvas 解码 → 读任意点像素；
 *   2. 每个文字元素取「贴着字框外侧」的 8 个点采样（保证落在文字墨迹之外），
 *      浅字取最亮样本、深字取最暗样本算最坏对比度 —— 伪元素高光/噪点全被量进去；
 *   3. 另取卡片左侧留白处的像素，与「遮罩后壁纸 × 卡片色 × 材质高光」的解析模型对账，
 *      证明 loginTheme.ts 单测里那套推导与真实渲染一致（不是自说自话）。
 *
 * 依赖：本地 vite dev (1420)。IPC mock 覆写为**未登录**（否则 /login 会被守卫踢到 /home）。
 */
const { chromium } = require("playwright");

/** 未登录 mock：只保证守卫判断得出来，业务请求一律走空值 */
const INIT_UNAUTH = `
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {};
window.__TAURI_INTERNALS__ = {
  convertFileSrc: (p) => "asset://localhost/" + encodeURIComponent(p),
  transformCallback(cb, once) {
    const id = Math.floor(Math.random() * 1e9);
    window["__cb_" + id] = (res) => { try { cb(res); } catch (e) {} if (once) delete window["__cb_" + id]; };
    return id;
  },
  async invoke(cmd) {
    if (cmd === "get_current_user") return null;
    if (/^(list|search|query|load|get_.*s$)/.test(cmd)) return [];
    return null;
  },
};
`;

/* ------------------------------------------------------------------ *
 * 页面内审计：解码截图 + 采样 + 对比度 + 模型对账
 * ------------------------------------------------------------------ */
const AUDIT = async (shotDataUrl) => {
  const q = (s) => document.querySelector(s);
  const cs = (el, pseudo) => getComputedStyle(el, pseudo || null);

  /* ---------- 颜色工具 ---------- */
  const parse = (c) => {
    if (!c) return null;
    if (c === "transparent") return { r: 0, g: 0, b: 0, a: 0 };
    const m = c.match(/rgba?\(([^)]+)\)/);
    if (!m) {
      if (/^#[0-9a-f]{3,8}$/i.test(c)) {
        let h = c.slice(1);
        if (h.length === 3) h = h.split("").map((s) => s + s).join("");
        const n = parseInt(h.slice(0, 6), 16);
        return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255, a: 1 };
      }
      return null;
    }
    const p = m[1].split(",").map((s) => parseFloat(s));
    return { r: p[0], g: p[1], b: p[2], a: p.length > 3 ? p[3] : 1 };
  };
  const lin = (v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  const lum = (c) => 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
  const over = (fg, bg) => ({
    r: fg.r * fg.a + bg.r * (1 - fg.a),
    g: fg.g * fg.a + bg.g * (1 - fg.a),
    b: fg.b * fg.a + bg.b * (1 - fg.a),
    a: fg.a + bg.a * (1 - fg.a),
  });
  const ratio = (a, b) => {
    const la = lum(a), lb = lum(b);
    return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
  };
  const mix = (a, b, t) => ({
    r: a.r + (b.r - a.r) * t,
    g: a.g + (b.g - a.g) * t,
    b: a.b + (b.b - a.b) * t,
    a: a.a + (b.a - a.a) * t,
  });
  const splitTop = (s) => {
    const out = [];
    let d = 0, cur = "";
    for (const ch of s) {
      if (ch === "(") d++;
      if (ch === ")") d--;
      if (ch === "," && d === 0) { out.push(cur); cur = ""; continue; }
      cur += ch;
    }
    if (cur.trim()) out.push(cur);
    return out.map((x) => x.trim()).filter(Boolean);
  };
  /** 竖向 linear-gradient 在「元素内 y 比例 t」处的颜色（材质高光就是这种） */
  const gradAtT = (layer, t) => {
    const inner = layer.replace(/^\s*linear-gradient\(/i, "").replace(/\)\s*$/, "");
    const parts = splitTop(inner);
    let angle = 180, i0 = 0;
    const head = (parts[0] || "").toLowerCase();
    if (/^[0-9.]+deg$/.test(head)) { angle = parseFloat(head); i0 = 1; }
    const stops = [];
    for (let i = i0; i < parts.length; i++) {
      const raw = parts[i].trim();
      const pm = raw.match(/\s+([0-9.]+)%\s*$/);
      let colorStr = raw, pos = null;
      if (pm) { pos = parseFloat(pm[1]) / 100; colorStr = raw.slice(0, pm.index).trim(); }
      const col = parse(colorStr);
      if (col) stops.push({ col, pos });
    }
    if (!stops.length) return null;
    stops.forEach((s, i) => { s.pos = s.pos ?? (i === 0 ? 0 : i === stops.length - 1 ? 1 : i / (stops.length - 1)); });
    if (angle === 0) t = 1 - t; // to top / 0deg：t 从底部算起
    t = Math.max(0, Math.min(1, t));
    let s0 = stops[0], s1 = stops[stops.length - 1];
    let matched = false;
    for (let i = 0; i < stops.length - 1; i++) {
      if (t >= stops[i].pos && t <= stops[i + 1].pos) { s0 = stops[i]; s1 = stops[i + 1]; matched = true; break; }
    }
    // 超出末段必须夹住而不是外推：外推会把 alpha 算成负数（对账曾出现 rgb(-23,-40,3)）
    if (!matched) return t <= stops[0].pos ? stops[0].col : stops[stops.length - 1].col;
    const span = s1.pos - s0.pos || 1;
    return mix(s0.col, s1.col, (t - s0.pos) / span);
  };

  /* ---------- 截图解码 → 像素读取 ---------- */
  const img = new Image();
  img.src = shotDataUrl;
  await img.decode();
  const cv = document.createElement("canvas");
  cv.width = img.naturalWidth;
  cv.height = img.naturalHeight;
  const ctx = cv.getContext("2d", { willReadFrequently: true });
  ctx.drawImage(img, 0, 0);
  const data = ctx.getImageData(0, 0, cv.width, cv.height).data;
  const scaleX = cv.width / window.innerWidth;
  const scaleY = cv.height / window.innerHeight;
  const px = (x, y) => {
    const sx = Math.round(x * scaleX), sy = Math.round(y * scaleY);
    if (sx < 0 || sy < 0 || sx >= cv.width || sy >= cv.height) return null;
    const i = (sy * cv.width + sx) * 4;
    return { r: data[i], g: data[i + 1], b: data[i + 2], a: 1 };
  };

  /* ---------- 结构探针 ---------- */
  const page_ = q(".auth-page");
  const card = q(".auth-card");
  const cover = q(".auth-cover");
  const overlay = q(".auth-overlay");
  const structure = {
    page: !!page_,
    card: !!card,
    cardClass: card ? card.className : "",
    cover: cover ? cs(cover).backgroundImage.slice(0, 90) : "",
    overlay: overlay ? cs(overlay).backgroundImage.slice(0, 60) : "",
    cardBg: card ? cs(card).backgroundColor : "",
    cardRadius: card ? cs(card).borderRadius : "",
    cardFilter: card ? cs(card).backdropFilter : "",
    cardImage: card ? cs(card).backgroundImage.slice(0, 60) : "",
    cardBefore: card ? cs(card, "::before").backgroundImage.slice(0, 70) : "",
    vars: {},
    favicon: (q("link[rel='icon']") || {}).href || "(none)",
    title: (q(".auth-title") || {}).textContent || "",
  };
  for (const v of [
    "--auth-card-bg", "--auth-card-border", "--auth-title", "--auth-link",
    "--auth-field-bg", "--auth-accent", "--auth-danger-text", "--auth-scrim",
  ]) {
    structure.vars[v] = (page_ ? cs(page_).getPropertyValue(v) : "").trim().slice(0, 70);
  }

  /* ---------- 材质跟随（默认效果）探针 ---------- */
  const materials = {};
  const keep = document.body.className;
  for (const m of ["mat-frosted", "mat-liquid", "mat-glazed"]) {
    document.body.className = keep.replace(/mat-\w+/g, "").trim() + " " + m;
    materials[m] = {
      filter: cs(card).backdropFilter,
      image: cs(card).backgroundImage.slice(0, 46),
      before: cs(card, "::before").backgroundImage.slice(0, 34),
    };
  }
  document.body.className = keep;

  /* ---------- 文字元素 + 采样点 ---------- */
  const cardRect = card.getBoundingClientRect();
  const targets = [];
  for (const el of card.querySelectorAll("*")) {
    const own = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (!own) continue;
    const s = cs(el);
    if (s.visibility === "hidden" || s.display === "none" || parseFloat(s.opacity) < 0.5) continue;
    if (el.matches(":disabled") || el.closest(":disabled")) continue;
    const r = el.getBoundingClientRect();
    if (r.width < 4 || r.height < 4) continue;
    const fg = parse(s.color);
    if (!fg) continue;
    targets.push({
      text: (el.textContent || "").trim().slice(0, 18),
      cls: (el.className || "").toString().slice(0, 24),
      color: s.color,
      bgColor: s.backgroundColor,
      fg,
      size: parseFloat(s.fontSize),
      bold: parseInt(s.fontWeight, 10) >= 700,
      rect: { l: r.left, t: r.top, r: r.right, b: r.bottom, cy: r.top + r.height / 2, cx: r.left + r.width / 2 },
    });
  }
  // 输入框里的占位符（::placeholder 不是元素）单独补测：取框内左侧空白 + 中部
  const phTargets = [];
  for (const inp of card.querySelectorAll(".field-input")) {
    const s = cs(inp);
    const phColor = (() => {
      const c = s.getPropertyValue("--auth-placeholder");
      return c || "#d6d0e2";
    })();
    const r = inp.getBoundingClientRect();
    phTargets.push({
      text: "占位符",
      cls: "::placeholder",
      color: parse(phColor) ? phColor : "#d6d0e2",
      bgColor: s.backgroundColor,
      fg: parse(phColor) || { r: 214, g: 208, b: 226, a: 1 },
      size: parseFloat(s.fontSize),
      bold: false,
      rect: { l: r.left, t: r.top, r: r.right, b: r.bottom, cy: r.top + r.height / 2, cx: r.left + r.width / 2 },
    });
  }

  /**
   * 取文字底下真实的底色样本 —— 按元素**自身是否带底色**二选一，不能混：
   *   ① 元素自带底色（主按钮 / 提示条 / 输入框）：文字压在**自己的填充**上，
   *      只取框**内**贴边、避开墨迹的点（按钮文字居中、输入框有 12px 内边距）；
   *      若混入框外样本，深色按钮字会被拿去和框外的紫玻璃比 → 1.4:1 的**假失败**。
   *   ② 元素无底色（标题 / 标签 / 链接）：文字压在卡片玻璃上，只取框**外**贴边的点；
   *      框内取样会点到墨迹本身（中文首字从 x=0 开始，l+4 就在笔画上）→ 假失败。
   * 取样点一律夹在卡片范围内，避免量到卡片外的壁纸。
   */
  const sampleAround = (r, pad, insideOnly) => {
    const inner = [
      [r.l + 4, r.cy], [r.r - 4, r.cy], [r.cx, r.t + 3], [r.cx, r.b - 3],
      [r.l + 4, r.t + 3], [r.r - 4, r.b - 3],
    ];
    const outer = [
      [r.l - pad, r.cy], [r.r + pad, r.cy], [r.cx, r.t - pad], [r.cx, r.b + pad],
      [r.l - pad, r.t - pad], [r.r + pad, r.t - pad], [r.l - pad, r.b + pad], [r.r + pad, r.b + pad],
    ];
    const cand = insideOnly ? inner : outer;
    const out = [];
    for (const [x, y] of cand) {
      if (x < cardRect.left + 2 || x > cardRect.right - 2) continue;
      if (y < cardRect.top + 2 || y > cardRect.bottom - 2) continue;
      if (x < 0 || y < 0 || x >= window.innerWidth || y >= window.innerHeight) continue;
      const c = px(x, y);
      if (c) out.push(c);
    }
    return out;
  };

  const contrast = [];
  for (const t of [...targets, ...phTargets]) {
    const own = parse(t.bgColor);
    const insideOnly = !!own && own.a > 0.02;
    const samples = sampleAround(t.rect, 7, insideOnly);
    if (!samples.length) continue;
    const darkText = lum({ r: t.fg.r, g: t.fg.g, b: t.fg.b }) < 0.4;
    const worst = samples.reduce((a, b) => (darkText ? (lum(b) < lum(a) ? b : a) : lum(b) > lum(a) ? b : a));
    const need = t.size >= 24 || (t.bold && t.size >= 18.66) ? 3 : 4.5;
    contrast.push({
      text: t.text || "(空)",
      cls: t.cls,
      color: t.color,
      ownBg: t.bgColor,
      worstBg: `rgb(${worst.r},${worst.g},${worst.b})`,
      ratio: Math.round(ratio(t.fg, worst) * 100) / 100,
      need,
      pass: ratio(t.fg, worst) >= need,
    });
  }

  /* ---------- 解析模型 vs 渲染像素对账 ---------- */
  // 卡片左侧留白（padding 32px）：x = left+8 处的竖排采样
  const cardColor = parse(cs(card).backgroundColor);
  const hl = cs(card).backgroundImage.includes("gradient") ? cs(card).backgroundImage : "";
  const model = [];
  for (let i = 1; i < 10; i++) {
    const y = cardRect.top + (cardRect.height * i) / 10;
    const outside = px(cardRect.left - 14, y); // 遮罩后的壁纸（卡片外）
    const inside = px(cardRect.left + 8, y); // 渲染出的玻璃
    if (!outside || !inside) continue;
    let pred = over(cardColor, outside);
    if (hl) {
      const t = (y - cardRect.top) / cardRect.height;
      const g = gradAtT(hl, t);
      if (g) pred = over(g, pred);
    }
    model.push({
      y: Math.round(y),
      rendered: `rgb(${Math.round(inside.r)},${Math.round(inside.g)},${Math.round(inside.b)})`,
      predicted: `rgb(${Math.round(pred.r)},${Math.round(pred.g)},${Math.round(pred.b)})`,
      dLum: Math.round(Math.abs(lum(inside) - lum(pred)) * 1000) / 1000,
    });
  }

  return { route: location.pathname, structure, materials, contrast, model };
};

/* ------------------------------------------------------------------ */
(async () => {
  const BASE = process.env.PM_BASE || "http://localhost:1420";
  const browser = await chromium.launch({ headless: true, channel: "msedge" });
  const routes = ["/login", "/register", "/forgot-password"];
  let totalFails = 0;

  /** 跑一轮：截图 → 送回页面解码 → 对比度 + 模型对账 */
  async function runPass(page, label, mode) {
    await page.waitForTimeout(400);
    const shot = (await page.screenshot({ fullPage: false })).toString("base64");
    const res = await page.evaluate(AUDIT, "data:image/png;base64," + shot);
    const fails = res.contrast.filter((c) => !c.pass);
    totalFails += fails.length;
    const maxDLum = res.model.reduce((a, m) => Math.max(a, m.dLum), 0);
    const minRatio = res.contrast.reduce((a, c) => Math.min(a, c.ratio), Infinity);
    if (mode === "full") {
      console.log(`结构: card.class="${res.structure.cardClass}" radius=${res.structure.cardRadius} bg=${res.structure.cardBg}`);
      console.log(`      cover=${res.structure.cover ? "有" : "无"} overlay=${res.structure.overlay ? "有" : "无"} filter=${res.structure.cardFilter}`);
      console.log(`      cardBgImage=${res.structure.cardImage}`);
      console.log(`      令牌: ${JSON.stringify(res.structure.vars).slice(0, 190)}`);
      console.log(`      favicon=${res.structure.favicon}`);
      console.log(`材质跟随: ${["mat-frosted", "mat-liquid", "mat-glazed"]
        .map((m) => `${m.replace("mat-", "")}[blur=${(res.materials[m].filter.match(/blur\([^)]+\)/) || ["-"])[0]},img=${res.materials[m].image.slice(0, 22)}]`)
        .join(" ")}`);
    }
    if (mode === "compact") {
      console.log(
        `   ${fails.length ? "✘" : "✔"} ${label.padEnd(12)} 最低对比 ${minRatio}:1  不达标 ${fails.length} 项  对账 Δlum ${maxDLum}`,
      );
      if (fails.length) for (const f of fails) console.log(`      ✘ ${f.text} ${f.color} on ${f.worstBg} = ${f.ratio}:1`);
      return res;
    }
    console.log(`对比度（${label}，真实渲染像素最坏采样点）:`);
    for (const c of res.contrast) {
      console.log(`  ${c.pass ? "✔" : "✘"} ${c.text.padEnd(16)} ${String(c.color).padEnd(20)} on ${c.worstBg.padEnd(17)} ${String(c.ratio).padStart(5)}:1 (需 ${c.need})`);
    }
    console.log(`  模型对账最大 |Δ相对亮度| = ${maxDLum}（模型不含 ::before 材质层（磨砂颗粒/釉面开片，soft-light），液态/釉瓷下 0.06~0.09 属预期）`);
    console.log(`  本轮不达标 ${fails.length} 项`);
    return res;
  }

  for (const route of routes) {
    const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
    const page = await ctx.newPage();
    const errors = [];
    page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
    page.on("console", (m) => { if (m.type() === "error") errors.push("console: " + m.text()); });
    await page.addInitScript(INIT_UNAUTH);
    await page.goto(BASE + route, { waitUntil: "networkidle", timeout: 20000 }).catch(() => {});
    await page.waitForSelector(".auth-card", { timeout: 10000 });
    await page.waitForTimeout(700);

    console.log(`\n===== ${route} =====`);
    const res = await runPass(page, "默认态", "full");
    res.errors = errors;
    const assets = await page.evaluate(async () => {
      const load = (url) =>
        new Promise((resolve) => {
          const i = new Image();
          i.onload = () => resolve({ url, ok: true, w: i.naturalWidth, h: i.naturalHeight });
          i.onerror = () => resolve({ url, ok: false });
          i.src = url;
        });
      const coverUrl = (getComputedStyle(document.querySelector(".auth-cover")).backgroundImage.match(/url\("([^"]+)"\)/) || [])[1];
      const iconUrl = (document.querySelector("link[rel='icon']") || {}).href;
      return Promise.all([load(coverUrl), load(iconUrl)]);
    });
    console.log(`资源可加载: ${assets.map((a) => `${a.url.split("/").pop()}=${a.ok ? a.w + "x" + a.h : "FAIL"}`).join("  ")}`);

    // 错误态：空表单提交触发字段级报错 → 提示条自身底色上的文字也要达标
    await page.click(".btn-primary");
    await page.waitForSelector(".error-msg", { timeout: 5000 }).catch(() => {});
    const errShown = (await page.$(".error-msg")) !== null;
    if (errShown) await runPass(page, "错误提示态", "full");
    else console.log("（未弹出错误提示，跳过错误态复测）");
    await page.evaluate(() => {
      const e = document.querySelector(".error-msg");
      if (e) e.closest("form").reset && e.closest("form").reset();
    });

    // 三种「默认效果」材质各跑一轮：釉瓷的镜面天光带最强，是顶部标题/副标题的风险点
    console.log("各材质复测（证明登录卡真的跟随「默认效果」的材质）:");
    for (const m of ["mat-frosted", "mat-liquid", "mat-glazed"]) {
      await page.evaluate((cls) => {
        document.body.className = document.body.className.replace(/mat-\w+/g, "").trim() + " " + cls;
      }, m);
      const r = await runPass(page, m.replace("mat-", ""), "compact");
      console.log(`      实测 blur=${(r.structure.cardFilter.match(/blur\([^)]+\)/) || ["?"])[0]} 高光=${r.structure.cardImage.slice(0, 34)}`);
    }
    await page.evaluate(() => {
      document.body.className = document.body.className.replace(/mat-\w+/g, "").trim() + " mat-frosted";
    });

    await page.screenshot({ path: `.impeccable/review/auth-${route.replace(/\//g, "")}.png` });
    console.log(`控制台错误 ${errors.length} 项${errors.length ? ": " + errors.slice(0, 4).join(" | ") : ""}`);
    await ctx.close();
  }

  console.log(`\n===== 合计：不达标 ${totalFails} 项 =====`);
  await browser.close();
})();
