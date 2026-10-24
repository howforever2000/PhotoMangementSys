import { createApp, h, nextTick } from "vue";
import { createRouter, createWebHashHistory } from "vue-router";
import { createPinia, setActivePinia } from "pinia";
import "../src/assets/tokens.css";
import "../src/assets/main.css";
import { useThemeStore } from "../src/stores/theme";
import { presetPrefs, DARK_PRESETS, LIGHT_PRESETS } from "../src/utils/presets";

/**
 * FEAT-105 真实主题探针（多页版）
 *
 * URL 形如 `#light/AlbumList` → 浅色预设 + 该页组件。
 * 让 theme store 自己跑一遍 apply，body 上才有正确的 --color-* / --c-*；
 * 只补几个变量的探针会把 #f5f7ff（main.css 深色默认）留在 body 上，浅色页读成白字。
 */

const ALBUMS = [
  ["毕业旅行", 109, 109, "2024-06-15", 1932735283],
  ["聚会合影", 109, 109, "2026-09-02", 1832735283],
  ["美食", 57, 57, "2024-10-08", 993273528],
  ["森林徒步", 42, 42, "2025-05-20", 793273528],
  ["自然风景", 36, 36, "2026-09-11", 693273528],
  ["夜空", 31, 0, "2024-12-03", 593273528],
  ["日落晚霞", 25, 0, "2023-08-19", 493273528],
  ["人像", 24, 24, "2026-09-25", 393273528],
  ["湖与岛", 18, 0, "2025-07-04", 293273528],
  ["猫", 14, 14, "2025-03-12", 193273528],
  ["雪", 12, 0, "2024-01-22", 93273528],
  ["花", 9, 9, "2023-04-07", 43327352],
].map(([name, photo_count, scanned, shoot_time, size_bytes], i) => ({
  id: i + 1,
  name,
  path: `D:/Photos/${name}`,
  description: null,
  cover_path: null,
  created_at: 0,
  updated_at: 0,
  photo_count,
  shoot_time,
  size_bytes,
  scanned_photo_count: scanned,
  location: null,
  tags: [],
  folder_id: null,
  folder_path: "",
  kind: "normal",
  merged_sources: [],
}));

const stub = async (cmd: string) => {
  if (cmd === "get_albums" || cmd === "list_albums" || cmd === "get_albums_with_stats") return ALBUMS;
  if (cmd === "list_timeline" || cmd === "list_persons") return [];
  return null;
};
(window as any).__TAURI_INTERNALS__ = {
  invoke: stub,
  transformCallback: (cb: any) => cb,
  convertFileSrc: (p: string) => p,
};
(window as any).__TAURI__ = {
  core: { invoke: stub, convertFileSrc: (p: string) => p },
  event: { listen: async () => () => {}, emit: async () => {} },
  dialog: { open: async () => null },
};

const ROUTES = [
  "/", "/home", "/albums", "/smart", "/scan", "/scan/test",
  "/timeline", "/memories", "/search", "/workshop", "/album/:id",
].map((path) => ({ path, component: { render: () => null } }));

const PAGE_PATH: Record<string, string> = {
  AlbumList: "/albums",
  SmartAlbum: "/smart",
  ScanHub: "/scan",
  Memories: "/memories",
  Timeline: "/timeline",
  Workshop: "/workshop",
};

(async () => {
  const [which, pageName = "AlbumList"] = location.hash.replace(/^#/, "").split("/");
  const mode = which === "light" ? "light" : "dark";
  const preset = mode === "light" ? LIGHT_PRESETS[2] : DARK_PRESETS[0];

  const mod = await import(/* @vite-ignore */ `/src/views/${pageName}.vue`);
  const Comp = mod.default;

  const host = document.createElement("div");
  host.id = `probe-${mode}`;
  document.body.appendChild(host);

  const router = createRouter({
    history: createWebHashHistory(),
    routes: [...ROUTES, { path: "/:pathMatch(.*)*", component: { render: () => null } }],
  });
  const pinia = createPinia();
  const app = createApp({ render: () => h(Comp as any) });
  app.use(pinia);
  app.use(router);
  setActivePinia(pinia);

  const theme = useThemeStore();
  const patch = presetPrefs(preset);
  for (const k of ["bgStyle", "gradFrom", "gradMid", "gradTo", "gradAngle", "compColor", "compAlpha", "material"] as const) {
    const v = (patch as any)[k];
    if (v !== undefined) (theme as any)[k] = v;
  }
  if (patch.mode) (theme as any).mode = patch.mode;
  await nextTick();
  await new Promise((r) => setTimeout(r, 400));

  await router.isReady();
  await router.push(PAGE_PATH[pageName] ?? "/albums").catch(() => {});
  app.mount(host);
  await nextTick();
  await new Promise((r) => setTimeout(r, 800));

  (window as any).__PROBE_READY__ = true;
  (window as any).__PRESET__ = { id: preset.id, mode: preset.mode, colors: preset.colors, page: pageName };
})();
