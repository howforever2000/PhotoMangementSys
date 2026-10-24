import { createApp, h, nextTick } from "vue";
import { createRouter, createWebHashHistory } from "vue-router";
import { createPinia } from "pinia";
import "../src/assets/tokens.css";
import "../src/assets/main.css";

/**
 * FEAT-105 结构验收探针：直接挂载**真实页面组件**，用桩 invoke 提供空数据。
 *
 * 要验证的不是"好不好看"，而是用户反馈的那个结构问题：
 *   页面里还有没有彩虹渐变 hero / 渐变入口卡 / 全局导航？
 *   顺序是不是 页头 → 二级 tab → 数据带 → 内容？
 *
 * ⚠️ 每个页面必须是**独立的 app + 独立的 router**：共用一个 router 时，
 * 后一次 push 会把前一个页面的 route 改掉，导致两个 tab 同时高亮
 * （第一版就是这么错的 —— 那是探针缺陷，不是产品 bug）。
 */

const stub = async (cmd: string) => {
  if (cmd === "list_timeline" || cmd === "list_persons") return [];
  if (cmd === "get_albums" || cmd === "list_albums") return [];
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
  "/",
  "/home",
  "/albums",
  "/smart",
  "/scan",
  "/scan/test",
  "/timeline",
  "/memories",
  "/search",
  "/workshop",
  "/album/:id",
].map((path) => ({ path, component: { render: () => null } }));

(async () => {
  const [{ default: SmartAlbum }, { default: ScanHub }, { default: Memories }] = await Promise.all([
    import("../src/views/SmartAlbum.vue"),
    import("../src/views/ScanHub.vue"),
    import("../src/views/Memories.vue"),
  ]);

  const cases = [
    { id: "smart", comp: SmartAlbum, path: "/smart" },
    { id: "scan", comp: ScanHub, path: "/scan" },
    { id: "memories", comp: Memories, path: "/memories" },
  ];

  for (const c of cases) {
    const host = document.createElement("div");
    host.id = `probe-${c.id}`;
    document.body.appendChild(host);

    const router = createRouter({
      history: createWebHashHistory(),
      routes: [...ROUTES, { path: "/:pathMatch(.*)*", component: { render: () => null } }],
    });
    const app = createApp({ render: () => h(c.comp as any) });
    app.use(createPinia());
    app.use(router);
    await router.isReady();
    await router.push(c.path).catch(() => {});
    app.mount(host);
    await nextTick();
    await new Promise((r) => setTimeout(r, 400));
  }

  (window as any).__PROBE_READY__ = true;
})();
