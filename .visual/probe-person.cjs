/**
 * 人物卡布局探针：验证「名字不被截断」
 * node .visual/probe-person.cjs [tag]
 *
 * 判据不是“看着像没截断”，而是三个可量化的量：
 *   ① scrollHeight vs clientHeight —— 文字是否有内容被裁掉
 *   ② scrollWidth  vs clientWidth  —— 是否有横向溢出（ellipsis 场景）
 *   ③ 卡片宽度 − 内边距 − 头像 —— 名字可用宽度是否真的变宽了
 */
const { chromium } = require("playwright");
const path = require("path");
const tag = process.argv[2] || "now";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 2 });
  const errs = [];
  p.on("pageerror", (e) => errs.push(String(e).split("\n")[0]));

  await p.goto("http://localhost:1420/.visual/person-probe.html", { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 20000 }).catch(() => {});
  await p.waitForTimeout(800);

  const r = await p.evaluate(() => {
    const grid = document.querySelector(".person-grid");
    const cards = [...document.querySelectorAll(".person-card")];
    const cs = grid ? getComputedStyle(grid) : null;
    return {
      cols: cs?.gridTemplateColumns?.split(" ").length ?? 0,
      cardW: cards[0] ? Math.round(cards[0].getBoundingClientRect().width) : 0,
      cardH: cards[0] ? Math.round(cards[0].getBoundingClientRect().height) : 0,
      flexDir: cards[0] ? getComputedStyle(cards[0]).flexDirection : "",
      avatarW: Math.round(document.querySelector(".person-avatar")?.getBoundingClientRect().width ?? 0),
      items: cards.map((c) => {
        const nm = c.querySelector(".person-name");
        const info = c.querySelector(".person-info");
        return {
          text: nm?.textContent?.trim() ?? "",
          /* 文字是否被裁：scroll 大于 client 就是有内容被藏 */
          clippedY: nm ? nm.scrollHeight > nm.clientHeight + 1 : false,
          clippedX: nm ? nm.scrollWidth > nm.clientWidth + 1 : false,
          nameW: nm ? Math.round(nm.getBoundingClientRect().width) : 0,
          nameH: nm ? Math.round(nm.getBoundingClientRect().height) : 0,
          infoW: info ? Math.round(info.getBoundingClientRect().width) : 0,
          lines: nm ? Math.round(nm.getBoundingClientRect().height / parseFloat(getComputedStyle(nm).lineHeight)) : 0,
        };
      }),
    };
  });

  console.log("\n── 网格 ──");
  console.log(`  列数=${r.cols} · 卡宽=${r.cardW}px · 卡高=${r.cardH}px · 布局=${r.flexDir} · 头像=${r.avatarW}px`);
  console.log("\n── 每个名字（clipped = 有内容被裁掉）──");
  let bad = 0;
  for (const it of r.items) {
    const flag = it.clippedY || it.clippedX ? "裁切 ✗" : "完整 ✓";
    if (it.clippedY || it.clippedX) bad++;
    console.log(
      `  ${flag}  「${it.text}」 名字宽=${it.nameW}px 高=${it.nameH}px(${it.lines}行) 可用=${it.infoW}px`,
    );
  }
  console.log(`\n  被裁切：${bad}/${r.items.length}`);

  const out = path.join(__dirname, "..", "design", "preview", `person-${tag}.png`);
  await p.screenshot({ path: out, fullPage: true });
  console.log(`截图：${path.basename(out)}`);
  console.log(`页面错误：${errs.length ? errs.slice(0, 3).join(" | ") : "无"}`);
  await b.close();
})();
