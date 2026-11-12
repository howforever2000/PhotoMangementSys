/**
 * 创意工坊 · 图像压缩后台任务 store（FEAT-150）
 *
 * 为什么放在 store 而不是 CompressTool 组件里：
 *   压缩已改为后端进程级任务（`compress_job::CompressJobState`）。任务状态挂在
 *   Pinia（脱离组件生命周期），切走页面/切回来都不丢 —— 这是「退出后任务消失」
 *   问题的前端一半解法（另一半在后端状态机）。
 *
 * 与后端职责划分：
 *   - **后端**持权威状态（真在跑、进度真在涨）：`get_compress_job` 快照
 *   - **store** 只做轮询对齐 + 视图派生（进度百分比 / 状态文案 / 部分结果）
 *
 * 与 `scanTask` 的差异：压缩是纯本地单线程 IO/CPU、进度是「张数」粒度，不需要
 * 20Hz 事件通道，**只轮询**即可；离开页面时停轮询（省资源），回来再 refresh+续轮询。
 */
import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import type { CompressJobSnapshot, CompressOptions } from "../types/tools";

/** 兜底轮询间隔 */
const POLL_INTERVAL_MS = 700;
/** 轮询句柄（非响应式，放模块级；_ 前缀避免被误塞进 state） */
let pollTimer: number | null = null;

function emptySnapshot(): CompressJobSnapshot {
  return {
    status: "idle",
    destDir: "",
    total: 0,
    done: 0,
    items: [],
    failed: [],
    totalBefore: 0,
    totalAfter: 0,
    current: "",
    error: "",
    startedAt: 0,
    finishedAt: 0,
    cancelled: false,
  };
}

interface CompressState {
  /** 后端权威快照（最后一次拉到的状态） */
  snapshot: CompressJobSnapshot;
}

export const useCompressTaskStore = defineStore("compressTask", {
  state: (): CompressState => ({
    snapshot: emptySnapshot(),
  }),

  getters: {
    /** 是否有任务在跑 */
    running: (s): boolean => s.snapshot.status === "running",
    /** 进度百分比（0~100） */
    percent: (s): number =>
      s.snapshot.total > 0
        ? Math.min(100, Math.round((s.snapshot.done / s.snapshot.total) * 100))
        : 0,
    /** 状态文案 */
    statusText(s): string {
      const st = s.snapshot.status;
      if (st === "running") return "正在压缩";
      if (st === "done") return "已完成";
      if (st === "failed") return "失败";
      if (st === "cancelled") return "已停止";
      return "空闲";
    },
    /** 是否已有可展示的结果（已完成部分 / 失败项） */
    hasResult(s): boolean {
      return s.snapshot.items.length > 0 || s.snapshot.failed.length > 0;
    },
  },

  actions: {
    /** 单次拉取后端快照（挂载 / 轮询 / 操作后对齐都用它） */
    async refresh(): Promise<CompressJobSnapshot> {
      const snap = await invoke<CompressJobSnapshot>("get_compress_job");
      this.snapshot = snap;
      if (snap.status !== "running") this.stopPolling();
      return snap;
    },

    /** 启动兜底轮询（仅在运行中需要） */
    startPolling(): void {
      if (pollTimer != null) return;
      pollTimer = window.setInterval(() => {
        void this.refresh().catch(() => {});
      }, POLL_INTERVAL_MS);
    },

    stopPolling(): void {
      if (pollTimer != null) {
        window.clearInterval(pollTimer);
        pollTimer = null;
      }
    },

    /** 启动后台压缩（启动即返回；随后轮询快照） */
    async start(
      paths: string[],
      destDir: string,
      opts: CompressOptions,
    ): Promise<void> {
      try {
        await invoke("start_compress_job", { paths, destDir, opts });
      } catch (e) {
        // 启动失败（如已有任务在跑）→ 刷新以展示真实状态
        await this.refresh().catch(() => {});
        throw e;
      }
      await this.refresh();
      this.startPolling();
    },

    /** 请求停止当前任务 */
    async cancel(): Promise<void> {
      try {
        await invoke("cancel_compress_job");
      } catch {
        /* 忽略：没有运行中的任务时后端会报「无任务」，轮询会收敛真实状态 */
      }
      await this.refresh().catch(() => {});
    },

    /** 清空任务记录（回到空闲） */
    async clear(): Promise<void> {
      await invoke("clear_compress_job");
      this.snapshot = emptySnapshot();
      this.stopPolling();
    },

    /** 组件卸载时停轮询（保留 snapshot，切回来可恢复） */
    disposeKeepState(): void {
      this.stopPolling();
    },
  },
});
