const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1400, height: 900 } });
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + String(e).split("\n")[0]));
  p.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 300)); });
  await p.goto(`http://localhost:${PORT}/.visual/app-probe.html`, { waitUntil: "load" });
  await p.waitForFunction(() => window.__PROBE_READY__ === true, { timeout: 30000 }).catch(() => {});
  await p.waitForTimeout(1500);
  const info = await p.evaluate(() => ({
    logs: window.__PROBE_LOGS__ || [],
    appChildren: document.getElementById("app")?.childElementCount,
    bodyText: (document.body.innerText || "").slice(0, 200),
  }));
  console.log("=== pageerrors/console");
  console.log(errs.slice(0, 12).join("\n") || "(none)");
  console.log("=== probe logs");
  console.log((info.logs || []).slice(0, 25).join("\n") || "(none)");
  console.log("=== app children:", info.appChildren);
  console.log("=== body text:", JSON.stringify(info.bodyText));
  await p.screenshot({ path: "design/preview/_app-probe.png" });
  await b.close();
})();
