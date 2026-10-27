const { chromium } = require("playwright");
const PORT = process.env.VITE_PORT || "1420";
(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  const p = await b.newPage({ viewport: { width: 1280, height: 800 } });
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + String(e).split("\n").slice(0,3).join(" | ")));
  p.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 400)); });
  p.on("requestfailed", (r) => errs.push("reqfail: " + r.url().slice(0, 140) + " :: " + (r.failure()?.errorText || "")));
  // 装 Tauri 桩（真实 main.ts 会走 auth.checkSession 等）
  await p.addInitScript(() => {
    const stub = async (cmd) => {
      if (cmd === "get_current_user") return null;
      if (cmd === "get_albums" || cmd === "list_albums" || cmd === "get_albums_with_stats") return [];
      if (cmd === "list_timeline" || cmd === "list_persons") return [];
      return null;
    };
    window.__TAURI_INTERNALS__ = { invoke: stub, transformCallback: (cb) => cb, convertFileSrc: (x) => x,
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } } };
    window.__TAURI__ = { core: { invoke: stub, convertFileSrc: (x) => x }, event: { listen: async () => () => {}, emit: async () => {} }, dialog: { open: async () => null } };
  });
  await p.goto(`http://localhost:${PORT}/`, { waitUntil: "load" });
  await p.waitForTimeout(4000);
  const t = await p.evaluate(() => ({
    appChildren: document.getElementById("app")?.childElementCount,
    text: (document.body.innerText || "").slice(0, 160),
    url: location.href,
  }));
  console.log("=== errors");
  console.log(errs.slice(0, 10).join("\n") || "(none)");
  console.log("=== app children:", t.appChildren);
  console.log("=== url:", t.url);
  console.log("=== text:", JSON.stringify(t.text));
  await p.screenshot({ path: "design/preview/_dev-root.png" });
  await b.close();
})();
