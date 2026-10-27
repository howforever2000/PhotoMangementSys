// 当前界面实拍（mock 数据）：node .visual/shoot-current.cjs
// 例：node .visual/shoot-current.cjs Home AlbumList dark light
const { chromium } = require("playwright");

const pages = process.argv.slice(2);
const ROUTES = {
  Home: "/home",
  AlbumList: "/albums",
  SmartAlbum: "/smart",
  SmartCategory: "/smart?tab=category",
  SmartFace: "/smart?tab=face",
  SmartLocation: "/smart?tab=location",
  ScanHub: "/scan",
  Memories: "/memories",
  Timeline: "/timeline",
  SmartSearch: "/search",
  Workshop: "/workshop",
  AlbumDetail: "/album/1",
  ManualSort: "/albums/manual",
};
const PORT = process.env.VITE_PORT || "1519";

(async () => {
  const b = await chromium.launch({ executablePath: "/home/hao/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome", args: ["--no-sandbox"] });
  for (const page_ of pages.length ? pages : Object.keys(ROUTES)) {
    const p = await b.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 });
    p.on("pageerror", (e) => console.log(`[${page_}] 错误`, String(e).split("\n")[0]));
    p.on("console", (m) => {
      if (m.type() === "error") console.log(`[${page_}] console.error`, m.text().slice(0, 160));
    });
    await p.goto(`http://localhost:${PORT}${ROUTES[page_] ?? "/home"}${(ROUTES[page_] ?? "").includes("?") ? "&" : "?"}mock=true`, { waitUntil: "load" });
    await p.evaluate(() => document.fonts.ready);
    await p.waitForTimeout(1400);
    const out = `design/preview/current-${page_}.png`;
    await p.screenshot({ path: out });
    console.log("shot", out);
    await p.close();
  }
  await b.close();
})();
