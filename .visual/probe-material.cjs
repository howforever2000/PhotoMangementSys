const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1280, height: 800 } });
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + String(e).split("\n")[0]));
  p.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });

  await p.goto(`http://localhost:${PORT}/.visual/material-probe.html`, { waitUntil: "load" });
  await p.waitForTimeout(600);

  const read = () =>
    p.evaluate(() => {
      const out = {};
      for (const id of ["c1", "c2", "c3"]) {
        const el = document.getElementById(id);
        const cs = getComputedStyle(el);
        const a = getComputedStyle(el, "::after");
        const bf = getComputedStyle(el, "::before");
        out[id] = {
          backdropFilter: cs.backdropFilter || cs.webkitBackdropFilter,
          bgColor: cs.backgroundColor,
          bgImageCount: (cs.backgroundImage.match(/gradient/g) || []).length,
          shadow: cs.boxShadow.slice(0, 90),
          afterContent: a.content,
          afterDisplay: a.display,
          afterBg: a.backgroundImage.slice(0, 60),
          afterInset: `${a.top}/${a.left}/${a.width}x${a.height}`,
          beforeContent: bf.content,
        };
      }
      return { bodyClass: document.body.className, out };
    });

  console.log("=== LIQUID");
  console.log(JSON.stringify(await read(), null, 2));

  await p.evaluate(() => {
    document.body.classList.remove("mat-liquid");
    document.body.classList.add("mat-plain");
  });
  await p.waitForTimeout(300);
  console.log("=== PLAIN");
  console.log(JSON.stringify(await read(), null, 2));

  console.log("=== errors");
  console.log(errs.join("\n") || "(none)");

  await p.screenshot({ path: "design/preview/_material-probe.png" });
  await b.close();
})();
