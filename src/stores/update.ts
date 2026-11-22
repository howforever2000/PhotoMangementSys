import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UpdateDownloadStatus, UpdateInfo, UpdateState } from "../types/update";
import { notify } from "../composables/useNotify";

/**
 * 应用内更新 store（FEAT-212）
 *
 * 为什么状态收在 store 而不是弹窗里：
 *  - 「检查更新 / 下载 / 安装」三条链路都要活过弹窗关闭 —— 用户点了下载就把窗口关掉
 *    是常态，进度必须继续走；
 *  - 后端会在启动 8s 后用 `update-available` 事件主动推一次发现新版本，
 *    监听必须在应用启动时就位（`App.vue` 调 `init()`），不能等到弹窗第一次打开；
 *  - 设置菜单的「发现新版本」小圆点也要读同一份状态。
 *
 * 状态来源只有两处：命令返回（`update_state` / `check_update`）与两个事件；
 * 组件不自己拼状态，只渲染这里。
 */

/** 下载状态初值（与后端 `UpdateDownloadStatus::default()` 逐字段对齐） */
function emptyDownload(): UpdateDownloadStatus {
  return {
    running: false,
    stage: "idle",
    received: 0,
    total: 0,
    bytes_per_sec: 0,
    error: null,
    path: null,
  };
}

export const useUpdateStore = defineStore("update", {
  state: () => ({
    /** 当前运行版本（后端 `package_info().version`） */
    current: "",
    /** 最近一次检查结果；null = 本次启动还没检查过 */
    info: null as UpdateInfo | null,
    /** 下载进度（由 `update-download-progress` 事件持续刷新） */
    download: emptyDownload(),
    /** 检查请求进行中（按钮禁用 + 「正在检查…」） */
    checking: false,
    /** 检查失败原因（空串 = 无错误） */
    checkError: "",
    /** 更新弹窗是否打开 */
    dialogOpen: false,
    /** 事件监听是否已注册（幂等标记） */
    wired: false,
    /** 已经弹过 toast 的版本 tag：同一版本只打扰用户一次 */
    notifiedTag: "",
  }),

  getters: {
    /** 远端有更新版本 */
    hasUpdate: (s): boolean => !!s.info?.has_update,
    /**
     * 安装包已下载且校验通过（可以点「立即安装」）
     *
     * 必须同时要求 `path`：`stage === "done"` 但没有路径时（理论上不会，
     * 但后端若在别处改了终态写法就是这种形状）点安装只会得到一句报错。
     */
    readyToInstall: (s): boolean => s.download.stage === "done" && !!s.download.path,
  },

  actions: {
    /**
     * 注册事件监听并同步一次状态（幂等，重复调用直接返回）
     *
     * 必须在应用启动时就调：后端自动检查完会 `emit("update-available")`，
     * 那时用户可能还没打开过任何更新相关界面，晚一步就丢事件。
     */
    async init() {
      if (this.wired) return;
      this.wired = true;

      listen<UpdateDownloadStatus>("update-download-progress", (e) => {
        this.download = e.payload;
      }).catch(() => {
        /* 监听失败不阻塞：还能通过 refresh() 拉快照，只是没有实时速度 */
      });

      listen<UpdateInfo>("update-available", (e) => {
        const info = e.payload;
        this.info = info;
        // 同一版本只提示一次：这个事件每次启动都可能重发，反复弹窗等于骚扰。
        // `notifiedTag` 只在内存里 —— 重启后再提示一次是合理的（用户可能上次点了稍后）。
        if (info.latest === this.notifiedTag) return;
        this.notifiedTag = info.latest;
        notify.action(
          "info",
          "发现新版本 v" + info.latest,
          [
            { label: "去更新", style: "primary", onClick: () => this.openDialog() },
            { label: "稍后", style: "secondary", onClick: () => {} },
          ],
          "当前 v" + (this.current || info.current) + " · 安装包 177MB 左右",
          0,
        );
      }).catch(() => {
        /* 同上：事件通道不可用不影响手动检查 */
      });

      await this.refresh();
    },

    /**
     * 拉一次后端快照（不联网）
     *
     * 打开弹窗、快捷键回到前台等场景都靠它对齐：事件可能因为窗口重建/监听晚注册
     * 而漏掉一两条，快照永远是权威的那一份。
     */
    async refresh() {
      try {
        const st = await invoke<UpdateState>("update_state");
        this.current = st.current;
        this.info = st.info;
        this.download = st.download;
      } catch (e) {
        // 快照失败不改任何字段：宁可用旧状态，也不要把界面刷成「什么都没有」
        console.warn("[update] 读取更新状态失败:", e);
      }
    },

    /**
     * 检查更新
     *
     * `force = false` 时后端在 6 小时节流窗口内直接回缓存（打开弹窗用这个，
     * 避免「点一次菜单就打一次 GitHub API」）；用户主动点「检查更新」时用 true。
     */
    async check(force = true) {
      if (this.checking) return;
      this.checking = true;
      this.checkError = "";
      try {
        const info = await invoke<UpdateInfo>("check_update", { force });
        this.info = info;
        this.checkError = "";
      } catch (e) {
        this.checkError = String(e);
      } finally {
        this.checking = false;
      }
    },

    /** 开始下载安装包（后端立即返回，进度靠事件推） */
    async startDownload() {
      try {
        await invoke("start_update_download");
        // 命令内部已把 stage 置为 probing 并 emit，但那条事件可能早于本次
        // 调用返回就到达（甚至到达时本组件还没挂载）—— 补拉一次快照最稳。
        await this.refresh();
      } catch (e) {
        notify.error("下载更新失败", String(e));
      }
    },

    /** 取消下载（后端保留分片，再点下载会续传） */
    async cancelDownload() {
      try {
        await invoke("cancel_update_download");
        // 终态由后台任务落定时 emit；这里不乐观改 stage，
        // 否则「点取消 → 立刻又点继续」会出现两个判断同时成立的中间态。
      } catch (e) {
        notify.error("取消下载失败", String(e));
      }
    },

    /** 拉起安装程序（成功即意味着本进程 600ms 后退出） */
    async install() {
      try {
        await invoke("install_update");
        // 安装包已下完且校验过，退出是预期行为：这句不是「出错了」，
        // 而是解释「窗口为什么消失」，避免用户以为应用崩了。
        notify.info("正在退出并启动安装程序", "安装完成后请从桌面/开始菜单重新打开");
      } catch (e) {
        notify.error("启动安装程序失败", String(e));
      }
    },

    /** 打开安装包下载目录（先问后端要路径，再复用已有的 open_folder 命令） */
    async openFolder() {
      try {
        const dir = await invoke<string>("update_download_dir");
        await invoke("open_folder", { path: dir });
      } catch (e) {
        notify.error("打开下载文件夹失败", String(e));
      }
    },

    /**
     * 打开更新弹窗
     *
     * 先从后端拉一次快照（可能在其他窗口/上次会话里已经检查过），
     * 完全没有结果时才自动查一次（`force = false`，遵守后端 6 小时节流）。
     */
    async openDialog() {
      this.dialogOpen = true;
      await this.refresh();
      if (!this.info) void this.check(false);
    },

    closeDialog() {
      this.dialogOpen = false;
    },
  },
});
