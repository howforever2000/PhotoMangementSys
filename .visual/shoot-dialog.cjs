// 主题弹窗（预设卡）实拍：node .visual/shoot-dialog.cjs sunset-dusk paper-light
const { chromium } = require("playwright");
const fs = require("fs");
const PORT = process.env.VITE_PORT || "1519";
const OUT = ".visual/out/dialog";
fs.mkdirSync(OUT, { recursive: true });
const ids = process.argv.slice(2).length ? process.argv.slice(2) : ["sunset-dusk"];

const INIT = () => {
  const stub = async (cmd) => {
    if (cmd === "get_current_user") return { id: 1, username: "hao", email: "t@example.com", created_at: 1, avatar: null };
    if (cmd === "get_albums") return [];
    if (cmd === "list_timeline") return [];
    if (cmd === "list_persons") return [];
    return null;
  };
  window.__TAURI_INTERNALS__ = { invoke: stub, transformCallback: (cb) => cb, convertFileSrc: (p) => p };
  window.__TAURI__ = { core: { invoke: stub, convertFileSrc: (p) => p }, event: { listen: async () => () => {}, emit: async () => {} }, dialog: { open: async () => null } };
};

(async () => {
  const b = await chromium.launch({ channel: "msedge" });
  for (const id of ids) {
    const p = await b.newPage({ viewport: { width: 1440, height: 1300 }, deviceScaleFactor: 1 });
    p.on("pageerror", (e) => console.log(`[${id}] 错误`, String(e).split("\n")[0]));
    await p.addInitScript(INIT);
    await p.goto(`http://localhost:${PORT}/home`, { waitUntil: "load" });
    await p.evaluate(async (pid) => {
      const m = await import("/src/utils/presets.ts");
      const pr = [...m.DARK_PRESETS, ...m.LIGHT_PRESETS].find((x) => x.id === pid);
      localStorage.setItem("pm-theme", JSON.stringify(m.presetPrefs(pr)));
    }, id);
    await p.reload({ waitUntil: "load" });
    await p.waitForTimeout(1200);
    await p.click('button[aria-label="主题 / 皮肤设置"]');
    await p.waitForTimeout(700);
    const box = await p.locator("#panel-preset").boundingBox();
    if (!box) { console.log(`[${id}] 弹窗未打开`); await p.close(); continue; }
    await p.screenshot({ path: `${OUT}/${id}.png`, clip: { x: box.x - 8, y: Math.max(0, box.y - 70), width: box.width + 16, height: Math.min(1240, box.height + 90) } });
    console.log("shot", `${OUT}/${id}.png`);
    await p.close();
  }
  await b.close();
})();
