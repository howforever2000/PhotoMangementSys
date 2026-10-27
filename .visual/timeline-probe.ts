/**
 * 时间线探针（FEAT-113 ③-3）
 *
 * 挂**真实的 Timeline.vue**，只桩掉 Tauri 命令与缩略图。
 * 为什么不自己写一份 HTML 模拟：这次改的是「左栏 sticky + 行式密排 + 滚动跟随」，
 * 全是组件内部行为（ResizeObserver 量宽、scrollIntoView、railActive 跟随），
 * 复刻一份只能验证到 CSS，验证不到逻辑。
 */
import { createApp, h } from "vue";
import { createPinia } from "pinia";
import { createRouter, createWebHashHistory } from "vue-router";
import "../src/assets/tokens.css";
import "../src/assets/main.css";
import Timeline from "../src/views/Timeline.vue";

/* ── 造数据：3 年、每月张数不一，制造「一个月跨几屏」的真实情况 ── */
const LOCS = ["四川省·成都", "四川省·达州", "贵州省·贵阳", "云南省·昆明", "重庆市"];
const LANDSCAPES = ["毕业旅行", "山林", "海边", "街头", "夜拍", "美食", "家人"];

function mkRows() {
  const rows: any[] = [];
  let id = 1;
  const spec: Array<[string, number]> = [
    ["2026", 10], ["2026", 9], ["2026", 8], ["2026", 3],
    ["2025", 12], ["2025", 11], ["2025", 7], ["2025", 5], ["2025", 2],
    ["2024", 10], ["2024", 6],
  ];
  for (const [year, month] of spec) {
    const n = 3 + ((year.charCodeAt(3) + month * 7) % 11); /* 3~13 张/月，避免整齐 */
    for (let i = 0; i < n; i++) {
      rows.push({
        id: id++,
        path: `/photos/mock-${year}-${month}-${i}.jpg`,
        album_id: 1,
        album_name: LANDSCAPES[(year.charCodeAt(3) + i) % LANDSCAPES.length],
        label: i % 3 === 0 ? LANDSCAPES[i % LANDSCAPES.length] : null,
        location: i % 4 === 0 ? LOCS[i % LOCS.length] : null,
        shoot_time: `${year}-${String(month).padStart(2, "0")}-${String(1 + (i % 27)).padStart(2, "0")} 12:00:00`,
        width: 4000,
        height: 3000,
      });
    }
  }
  return rows;
}
const ROWS = mkRows();

/* 缩略图：用 CSS 渐变代替真图，避免依赖外部文件 */
const HUES = [268, 210, 32, 150, 340, 190, 60, 290, 120, 240, 8, 175];
function ph(path: string) {
  const h1 = HUES[path.length % HUES.length];
  const h2 = (h1 + 42) % 360;
  return `linear-gradient(135deg, hsl(${h1} 38% 46%), hsl(${h2} 44% 30%))`;
}

const stub = async (cmd: string) => {
  if (cmd === "list_timeline") return ROWS;
  if (cmd === "get_photo_thumbs") return {};
  if (cmd === "get_current_user") return null;
  if (cmd === "get_albums" || cmd === "list_albums") return [];
  if (cmd === "list_persons") return [];
  return null;
};
(window as any).__TAURI_INTERNALS__ = {
  invoke: stub,
  transformCallback: (cb: unknown) => cb,
  convertFileSrc: (p: string) => p,
  metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
};
(window as any).__TAURI__ = {
  core: { invoke: stub, convertFileSrc: (p: string) => p },
  event: { listen: async () => () => {}, emit: async () => {} },
};

/* 让 .tl-thumb 在假图下也可见：缩略图走 thumbMap，桩里返回空 → 组件渲染占位。
   这里补一层背景，让不同卡片能被区分（否则整片是同一个 🖼️ 占位）。 */
const css = document.createElement("style");
css.textContent = `
.tl-thumb-ph { background: #2a2436 !important; }
.tl-card:nth-child(3n+1) .tl-thumb-ph { background: hsl(268 30% 34%) !important; }
.tl-card:nth-child(3n+2) .tl-thumb-ph { background: hsl(198 32% 32%) !important; }
.tl-card:nth-child(3n)   .tl-thumb-ph { background: hsl(32 34% 34%) !important; }
`;
document.head.appendChild(css);

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", component: Timeline },
    { path: "/smart", component: { render: () => h("div") } },
    { path: "/album/:id", component: { render: () => h("div") } },
  ],
});

const app = createApp({ render: () => h(Timeline) });
app.use(createPinia());
app.use(router);
router.isReady().then(() => {
  app.mount("#app");
  setTimeout(() => {
    (window as any).__PROBE_READY__ = true;
  }, 700);
});
