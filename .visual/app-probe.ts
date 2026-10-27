/**
 * App 真实启动探针（FEAT-106 白屏排查）
 *
 * 与 themed-probe 的区别：那个只挂**单个页面组件**；
 * 这个走完整启动链路 —— 真实 App.vue + 真实 router + 真实 store。
 * 白屏往往出在「路由守卫 / store 初始化 / 全局组件」这些页面级探针照不到的地方。
 *
 * 会给 window 装一份 Tauri 桩（命令调用返回空数据），
 * 并把所有 console.error / window.onerror / 未捕获 Promise 打到页面上，
 * 这样截图就能直接读到错误。
 */
import { createApp } from "vue";
import { createPinia } from "pinia";
import "../src/assets/tokens.css";
import "../src/assets/main.css";
import "../src/assets/pm-dialog.css";
import "../src/assets/auth.css";

const logs: string[] = [];
function log(kind: string, msg: string) {
  logs.push(`[${kind}] ${msg}`);
  const box = document.getElementById("probe-log");
  if (box) box.textContent = logs.join("\n");
  (window as any).__PROBE_LOGS__ = logs;
}

const origError = console.error;
console.error = (...a: unknown[]) => {
  log("console.error", a.map((x) => (x instanceof Error ? x.stack || x.message : String(x))).join(" "));
  origError.apply(console, a as never);
};
const origWarn = console.warn;
console.warn = (...a: unknown[]) => {
  log("console.warn", a.map(String).join(" "));
  origWarn.apply(console, a as never);
};
window.addEventListener("error", (e) => log("window.error", `${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => log("unhandledrejection", String(e.reason)));

// ---- Tauri 桩：所有命令返回空 ----
const stub = async (cmd: string) => {
  if (cmd === "get_current_user") return null;
  if (cmd === "get_albums" || cmd === "list_albums" || cmd === "get_albums_with_stats") return [];
  if (cmd === "list_timeline" || cmd === "list_persons") return [];
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
  dialog: { open: async () => null },
};

(async () => {
  const box = document.createElement("pre");
  box.id = "probe-log";
  box.style.cssText =
    "position:fixed;left:0;bottom:0;right:0;max-height:45vh;overflow:auto;margin:0;padding:8px 10px;" +
    "background:rgba(0,0,0,.86);color:#ffb4b4;font:11px/1.5 ui-monospace,Consolas,monospace;z-index:99999;white-space:pre-wrap";
  document.body.appendChild(box);

  try {
    const [{ default: App }, { default: router }] = await Promise.all([
      import("../src/App.vue"),
      import("../src/router"),
    ]);
    const app = createApp(App);
    app.use(createPinia());
    app.use(router);
    await router.isReady();
    app.mount("#app");
    log("ok", `mounted, route = ${router.currentRoute.value.fullPath}`);
    await new Promise((r) => setTimeout(r, 1200));
    log("ok", `after 1.2s, route = ${router.currentRoute.value.fullPath}, app children = ${document.getElementById("app")?.childElementCount}`);
  } catch (e) {
    log("mount-failed", e instanceof Error ? e.stack || e.message : String(e));
  }
  (window as any).__PROBE_READY__ = true;
})();
