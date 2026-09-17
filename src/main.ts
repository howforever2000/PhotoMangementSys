import { createApp } from "vue";
import { createPinia } from "pinia";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./assets/main.css";

/**
 * 多窗口入口：Rust 侧创建的副窗口加载同一份 index.html，这里按窗口 label 分流：
 *   - "dev-logs" → 开发者视角 · 实时日志（LogWindowApp）
 *   - "dev-data" → 开发者视角 · 数据与路径（DataWindowApp）
 * 副窗口只挂各自组件 + pinia（主题 store 初始化需要），不装 router
 * ——避开登录守卫把副窗口重定向到 /login。
 */
const winLabel = (() => {
  try {
    return getCurrentWindow().label;
  } catch {
    return "";
  }
})();
const isLogWindow = winLabel === "dev-logs";
const isDataWindow = winLabel === "dev-data";
const isDevWindow = isLogWindow || isDataWindow;

/**
 * 副窗口显式报错（BUG-2026-0910-001 诊断增强）：之前任何加载/执行异常都表现为
 * "纯黑什么都不显示"，无法区分"没数据"和"崩了"。现在把错误直接画到窗口里。
 */
function showFatal(msg: string) {
  const el = document.getElementById("app");
  if (!el) return;
  const name = isLogWindow ? "日志窗口" : isDataWindow ? "数据与路径窗口" : "主窗口";
  const prefix = isDevWindow ? "[开发者视角] " : "";
  el.innerHTML = "";
  const box = document.createElement("pre");
  box.style.cssText =
    "margin:0;padding:16px;color:#ff8585;background:#0c0e14;font:12px/1.6 ui-monospace,Consolas,monospace;white-space:pre-wrap;word-break:break-all;height:100vh;box-sizing:border-box;overflow:auto";
  box.textContent = `${prefix}${name}启动失败：\n\n${msg}`;
  el.appendChild(box);
}

/** 启动期捕获到的最后一条错误（供看门狗诊断文案用） */
let bootError = "";
/** 根组件是否已挂载成功（挂载成功后再出错不再覆盖界面，避免误伤正常业务） */
let mounted = false;

if (isDevWindow) {
  window.addEventListener("error", (e) => showFatal(`${e.message}\n@ ${e.filename}:${e.lineno}`));
  window.addEventListener("unhandledrejection", (e) => showFatal(String(e.reason)));
  // 副窗口固定深色：主题 store 只在主窗口把 theme-dark 挂到 body，
  // 副窗口不会初始化，html/body 会露默认浅色底——渲染滞后时白斑由此而来。
  const dark = "#0c0e14";
  document.documentElement.style.background = dark;
  document.documentElement.style.colorScheme = "dark";
  document.body.style.background = dark;
} else {
  /**
   * 主窗口启动看门狗（BUG-2026-0917-003）：
   * 渲染引擎异常时（WebView2 渲染进程崩溃 / 前端资源加载失败 / 脚本执行出错），
   * 窗口会照常创建但页面永远空白——此前用户只能看到一个"卡死"的白窗口，
   * 得不到任何提示。这里在启动期捕获错误，并在超时未挂载时把诊断信息画到窗口里。
   */
  window.addEventListener("error", (e) => {
    if (mounted) return;
    bootError = `${e.message}\n@ ${e.filename}:${e.lineno}`;
  });
  window.addEventListener("unhandledrejection", (e) => {
    if (mounted) return;
    bootError = String(e.reason);
  });

  const BOOT_TIMEOUT_MS = 10000;
  window.setTimeout(() => {
    if (mounted) return;
    const el = document.getElementById("app");
    if (el && el.childElementCount > 0) return; // 已渲染出内容，不算白屏
    const lines = [
      `页面在 ${BOOT_TIMEOUT_MS / 1000} 秒内没有渲染出任何内容（白屏）。`,
      "",
      "可能原因：",
      "1. WebView2 运行时异常（渲染进程崩溃）—— 检查系统是否安装 / 更新 WebView2 Runtime；",
      "2. 开发模式下前端 dev server 未就绪（默认 http://localhost:1420）；",
      "3. 页面脚本执行出错或资源加载失败。",
      "",
      "可尝试：完全退出应用后重新启动；若持续出现，请查看日志目录下的 app.log。",
    ];
    if (bootError) lines.push("", "最近捕获到的错误：", bootError);
    showFatal(lines.join("\n"));
  }, BOOT_TIMEOUT_MS);
}

/**
 * 按 label 动态拉取根组件（打开慢的来源之一）：原实现虽不挂 router，但 App.vue /
 * router / 全部视图是静态 import——副窗口每次打开都要拉取并执行整套应用模块图
 * （dev 下数百个模块请求）。动态 import 后副窗口只加载自身依赖。
 */
async function bootstrap() {
  try {
    if (isLogWindow) {
      const { default: LogWindowApp } = await import("./dev/LogWindowApp.vue");
      const app = createApp(LogWindowApp);
      app.use(createPinia());
      app.mount("#app");
    } else if (isDataWindow) {
      const { default: DataWindowApp } = await import("./dev/DataWindowApp.vue");
      const app = createApp(DataWindowApp);
      app.use(createPinia());
      app.mount("#app");
    } else {
      const [{ default: App }, { default: router }] = await Promise.all([
        import("./App.vue"),
        import("./router"),
      ]);
      const app = createApp(App);
      app.use(createPinia());
      app.use(router);
      app.mount("#app");
    }
    mounted = true; // 挂载成功——此后启动看门狗不再介入
  } catch (e) {
    // 主窗口挂载失败（模块加载异常 / 渲染崩溃）也要把原因画到窗口里，
    // 而不是留下一个无提示的白窗口（BUG-2026-0917-003）。
    if (!mounted) showFatal(String(e));
    else throw e;
  }
}

void bootstrap();
