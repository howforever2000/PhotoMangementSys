import { createApp } from "vue";
import { createPinia } from "pinia";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./assets/main.css";

/**
 * 双窗口入口：Rust 侧 open_dev_log_window 创建的日志副窗口 label 为 "dev-logs"，
 * 加载同一份 index.html。这里按窗口 label 分流：日志窗口只挂终端组件 + pinia
 * （主题 store 初始化需要），不装 router——避开登录守卫把副窗口重定向到 /login。
 */
const isLogWindow = (() => {
  try {
    return getCurrentWindow().label === "dev-logs";
  } catch {
    return false;
  }
})();

/**
 * 副窗口显式报错（BUG-2026-0910-001 诊断增强）：之前任何加载/执行异常都表现为
 * "纯黑什么都不显示"，无法区分"没日志"和"崩了"。现在把错误直接画到窗口里。
 */
function showFatal(msg: string) {
  const el = document.getElementById("app");
  if (!el) return;
  el.innerHTML = "";
  const box = document.createElement("pre");
  box.style.cssText =
    "margin:0;padding:16px;color:#ff8585;background:#0c0e14;font:12px/1.6 ui-monospace,Consolas,monospace;white-space:pre-wrap;word-break:break-all;height:100vh;box-sizing:border-box;overflow:auto";
  box.textContent = `[开发者视角] 日志窗口启动失败：\n\n${msg}`;
  el.appendChild(box);
}

if (isLogWindow) {
  window.addEventListener("error", (e) => showFatal(`${e.message}\n@ ${e.filename}:${e.lineno}`));
  window.addEventListener("unhandledrejection", (e) => showFatal(String(e.reason)));
  // 日志窗口固定纯黑终端风：主题 store 只在主窗口把 theme-dark 挂到 body，
  // 副窗口不会初始化，html/body 会露默认浅色底——渲染滞后时白斑由此而来。
  const dark = "#0c0e14";
  document.documentElement.style.background = dark;
  document.documentElement.style.colorScheme = "dark";
  document.body.style.background = dark;
}

/**
 * 按 label 动态拉取根组件（打开慢的来源之一）：原实现虽不挂 router，但 App.vue /
 * router / 全部视图是静态 import——日志副窗口每次打开都要拉取并执行整套应用模块图
 * （dev 下数百个模块请求）。动态 import 后日志窗只加载终端组件自身依赖。
 */
async function bootstrap() {
  try {
    if (isLogWindow) {
      const { default: LogWindowApp } = await import("./dev/LogWindowApp.vue");
      const app = createApp(LogWindowApp);
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
    if (isLogWindow) showFatal(String(e));
    else throw e;
  }
}

void bootstrap();
