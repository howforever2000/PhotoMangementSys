import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [vue()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      // 4. 编辑器保存时会先写「同目录临时文件 → 原子替换」，临时目录（`.xxx.tmpdir/`）
      //    在 Windows 上被 chokidar 追到时偶发 EBUSY 并让 dev server 整个退出。
      //    这类目录与构建无关，忽略即可（.visual 下的大截图同样不值得追）。
      ignored: ["**/src-tauri/**", "**/*.tmpdir/**", "**/.visual/**", "**/dist/**"],
    },
  },
}));
