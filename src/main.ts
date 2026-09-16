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
  const name = isLogWindow ? "日志窗口" : "数据与路径窗口";
  el.innerHTML = "";
  const box = document.createElement("pre");
  box.style.cssText =
    "margin:0;padding:16px;color:#ff8585;background:#0c0e14;font:12px/1.6 ui-monospace,Consolas,monospace;white-space:pre-wrap;word-break:break-all;height:100vh;box-sizing:border-box;overflow:auto";
  box.textContent = `[开发者视角] ${name}启动失败：\n\n${msg}`;
  el.appendChild(box);
}

if (isDevWindow) {
  window.addEventListener("error", (e) => showFatal(`${e.message}\n@ ${e.filename}:${e.lineno}`));
  window.addEventListener("unhandledrejection", (e) => showFatal(String(e.reason)));
  // 副窗口固定深色：主题 store 只在主窗口把 theme-dark 挂到 body，
  // 副窗口不会初始化，html/body 会露默认浅色底——渲染滞后时白斑由此而来。
  const dark = "#0c0e14";
  document.documentElement.style.background = dark;
  document.documentElement.style.colorScheme = "dark";
  document.body.style.background = dark;
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
  } catch (e) {
    if (isDevWindow) showFatal(String(e));
    else throw e;
  }
}

void bootstrap();
