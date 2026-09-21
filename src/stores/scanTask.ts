import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ScanJobSnapshot,
  ScanJobPhase,
  ScanJobProgress,
  ScanProgress,
  OrganizeSummary,
  TestPhoto,
} from "../types/photo";

/**
 * 相册扫描分组工具：后台任务状态（FEAT-064）
 *
 * 为什么放在 store 而不是页面组件里：
 *   任务状态挂在 Pinia（脱离组件生命周期），页面切走/切回都不丢 ——
 *   这是「退出后任务消失」问题的前端一半解法（另一半在后端 `ScanJobState`）。
 *
 * 与后端的职责划分：
 *   - **后端**持有权威状态（任务真在跑、进度真在涨）：`get_scan_job` 快照
 *   - **store** 持有前端视图状态：监听 `test-scan-progress` 事件做实时刷新，
 *     并在挂载/轮询时用快照对齐（防止事件丢帧导致显示落后）
 *
 * 为什么进度用「事件 + 轮询」双通道：
 *   事件负责流畅（20Hz），轮询负责兜底（事件在页面未挂载时无人接收，
 *   重进页面必须能补上；另外终态切换也靠它确定性收敛）。
 */

/**
 * 进度节流：事件可能 20Hz，但 Vue 渲染 8Hz 足够（避免高频 re-render）
 */
const RENDER_THROTTLE_MS = 120;
/** 运行中的兜底轮询间隔：事件通道正常时它只是「对齐」，异常时它保证不卡住 */
const POLL_INTERVAL_MS = 700;

/**
 * 事件监听句柄与会话内的临时量**放在闭包里而非 state**：
 * `UnlistenFn` 是函数，塞进 Pinia state 会被 reactive 包装（无意义开销，
 * 且函数被代理后调用语义容易出岔子）；它们本来也不需要被模板响应式追踪。
 */
let unlistenFn: UnlistenFn | null = null;
let pollTimer: number | null = null;
let lastRenderAt = 0;
let lastEventAt = 0;

interface ScanState {
  /** 后端权威快照（最后一次拉到/推送的状态） */
  snapshot: ScanJobSnapshot;
  /** 扫描结果照片列表（get_scan_job 不含列表，需单独拉；见 photosLoaded） */
  photos: TestPhoto[];
  /** 照片列表是否已加载（避免空态误判） */
  photosLoaded: boolean;
  /** 实时进度（事件通道，比 snapshot 更新更频繁） */
  live: ScanJobProgress | null;
  /** 事件监听是否已注册（只注册一次） */
  _ready: boolean;
}

function emptySnapshot(): ScanJobSnapshot {
  return {
    status: "idle",
    phase: null,
    dir: "",
    recursive: false,
    threads: 0,
    progress: null,
    photo_count: 0,
    place_count: 0,
    organize: null,
    error: "",
    started_at: 0,
    finished_at: 0,
  };
}

export const useScanTaskStore = defineStore("scanTask", {
  state: (): ScanState => ({
    snapshot: emptySnapshot(),
    photos: [],
    photosLoaded: false,
    live: null,
    _ready: false,
  }),

  getters: {
    /** 是否有任务在跑（后端权威） */
    running: (s): boolean => s.snapshot.status === "running",
    /** 当前阶段（运行中才有效） */
    phase: (s): ScanJobPhase | null =>
      s.snapshot.status === "running" ? s.snapshot.phase : null,
    /** 进度（优先实时事件，回落到快照） */
    progress: (s): ScanJobProgress | null => s.live ?? s.snapshot.progress,
    /** 进度百分比（0~100） */
    percent(): number {
      const p = this.progress;
      if (!p || p.total <= 0) return 0;
      return Math.min(100, Math.round((p.current / p.total) * 100));
    },
    /** 运行状态文案（头部状态条） */
    statusText(s): string {
      const st = s.snapshot.status;
      if (st === "running") {
        const map: Record<string, string> = {
          scan: "正在扫描",
          resolve: "正在解析地名",
          organize: "正在组织移动",
        };
        return map[s.snapshot.phase ?? ""] ?? "处理中";
      }
      if (st === "done") return "已完成";
      if (st === "failed") return "失败";
      if (st === "cancelled") return "已停止";
      return "空闲";
    },
    /** 组织移动报告（后端回填，供报告卡片渲染） */
    organizeReport: (s): OrganizeSummary | null => s.snapshot.organize,
    /** 是否已有可展示的结果（扫描过或正在扫） */
    hasResult(s): boolean {
      return s.photosLoaded && s.photos.length > 0;
    },
    /** 任务使用了多少线程（运行中展示，帮助用户确认设置生效） */
    threadsUsed: (s) => s.snapshot.threads,
  },

  actions: {
    /**
     * 注册进度事件监听 + 启动兜底轮询（幂等，重复调用无副作用）
     *
     * 事件回调只更新 `live`（轻量），由节流控制渲染；轮询负责把 `snapshot`
     * 与后端对齐（含 status 变化 —— 任务结束不一定有事件，必须有轮询兜底）。
     */
    async ensureListener() {
      if (this._ready) return;
      this._ready = true;
      unlistenFn = await listen<ScanProgress>("test-scan-progress", (e) => {
        const now = Date.now();
        // 节流：事件可达 20Hz，但 Vue 渲染没必要那么快
        if (now - lastRenderAt < RENDER_THROTTLE_MS) return;
        lastRenderAt = now;
        // 事件不带 rate/eta（后端只在快照里给）→ 这里按到达时间本地估算，
        // 让进度条旁边的「x 张/秒 · 剩余 y 秒」在事件通道也能实时跳动；
        // 精确值以轮询拉到的快照为准（会覆盖它）。
        const p = e.payload;
        const prev = this.live;
        let rate = 0;
        if (prev && p.current > prev.current && lastEventAt > 0) {
          const dt = (now - lastEventAt) / 1000;
          if (dt > 0) rate = (p.current - prev.current) / dt;
        }
        lastEventAt = now;
        this.live = {
          phase: p.phase,
          current: p.current,
          total: p.total,
          file_name: p.file_name,
          message: p.message,
          rate,
          eta_sec:
            rate > 0 && p.total > p.current
              ? (p.total - p.current) / rate
              : p.current >= p.total
                ? 0
                : null,
        };
      });
    },

    /** 单次拉取后端快照（页面挂载 / 轮询 / 操作后立即对齐都用它） */
    async refresh() {
      const snap = await invoke<ScanJobSnapshot>("get_scan_job");
      this.snapshot = snap;
      // 任务结束（非运行中）→ 停止轮询，并把 live 对齐快照
      if (snap.status !== "running") {
        this.stopPolling();
        this.live = snap.progress;
      }
      return snap;
    },

    /** 启动兜底轮询（仅在运行中需要） */
    startPolling() {
      if (pollTimer != null) return;
      pollTimer = window.setInterval(() => {
        void this.refresh().catch(() => {});
      }, POLL_INTERVAL_MS);
    },

    stopPolling() {
      if (pollTimer != null) {
        window.clearInterval(pollTimer);
        pollTimer = null;
      }
    },

    /** 开始一个阶段任务（scan / resolve / organize），返回是否成功启动 */
    async start(phase: ScanJobPhase, dir: string, recursive: boolean): Promise<boolean> {
      await this.ensureListener();
      const cmd =
        phase === "scan"
          ? "start_scan_job"
          : phase === "resolve"
            ? "start_resolve_job"
            : "start_organize_job";
      try {
        const snap = await invoke<ScanJobSnapshot>(cmd, { path: dir, recurse: recursive });
        this.snapshot = snap;
        this.live = snap.progress;
        // scan 阶段开始即清空旧结果，避免与新结果混显
        if (phase === "scan") {
          this.photos = [];
          this.photosLoaded = false;
        }
        this.startPolling();
        return true;
      } catch (e) {
        // 启动失败（如已有任务在跑）→ 刷新以展示真实状态
        await this.refresh().catch(() => {});
        throw e;
      }
    },

    /** 请求停止当前任务 */
    async cancel() {
      try {
        await invoke<boolean>("cancel_scan_job");
      } catch {
        /* 忽略：轮询会收敛真实状态 */
      }
      // 立即对齐一次，让按钮状态快些反映「停止中」
      await this.refresh().catch(() => {});
    },

    /** 清空任务记录（回到空闲） */
    async clear() {
      await invoke("clear_scan_job");
      this.snapshot = emptySnapshot();
      this.live = null;
      this.photos = [];
      this.photosLoaded = false;
      this.stopPolling();
    },

    /**
     * 拉取扫描结果照片列表
     *
     * 后端快照刻意**不塞整表**（万张相册序列化会卡），所以列表由本动作单独取：
     * 扫描/解析地名完成后调用一次即可。取用 `get_scan_job` 的 dir + recursive
     * 重跑一次后端扫描函数（不落库、只读，幂等）。
     */
    async loadPhotos(dir: string, recursive: boolean) {
      const list = await invoke<TestPhoto[]>("scan_test_photos", {
        path: dir,
        recurse: recursive,
      });
      this.photos = list;
      this.photosLoaded = true;
      return list;
    },

    /** 组件卸载时释放（保留 snapshot 以便切回来恢复；仅停轮询与事件） */
    dispose() {
      this.stopPolling();
      unlistenFn?.();
      unlistenFn = null;
      this._ready = false;
      lastRenderAt = 0;
      lastEventAt = 0;
    },
  },
});
