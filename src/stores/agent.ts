import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  isTerminal,
  type AgentSnapshot,
  type AgentSubmitRequest,
} from "../types/agent";

/**
 * 图像创作 Agent：任务状态 store（docs/v5.md §3、§17）。
 *
 * 与 `scanTask.ts` 同构，因为两者面对的是同一类问题：
 *   - **真相在后端**：Rust 持有 job 快照，Python 死了也能答「中断在第 3 步」；
 *   - **事件负责流畅，快照负责兜底**：事件在页面未挂载时无人接收，
 *     重开面板必须能补上（`agent_jobs_snapshot`）；
 *   - **渲染节流 120ms**：事件来自 400ms 轮询，本来不快，但一条轨迹里
 *     可能连续来多条合并结果，节流能把同一帧内的多次 patch 合成一次。
 *
 * 与 scanTask 的差别：Agent 是**单飞**的（同一时刻一个 job），
 * 因此这里额外暴露 `running`，UI 据此禁用「开始」按钮。
 */

/** 事件→渲染节流：合并同一帧内的多次快照 */
const RENDER_THROTTLE_MS = 120;
/** 兜底轮询：事件通道异常时保证不卡住（只在有运行中 job 时跑） */
const POLL_INTERVAL_MS = 700;

/** 监听句柄与定时器放闭包，不进 Pinia state（函数被 reactive 代理没有意义） */
let unlistenFn: UnlistenFn | null = null;
let pollTimer: number | null = null;
let flushTimer: number | null = null;
let pending: AgentSnapshot | null = null;
let lastFlushAt = 0;

interface AgentStoreState {
  /** 开关（默认关：关掉时工坊退化回纯工具台） */
  enabled: boolean;
  /** 全部 job 快照（按 jobId 升序，最近的在最后） */
  jobs: AgentSnapshot[];
  /** 事件监听是否已注册 */
  _ready: boolean;
  /** 侧车预热中 */
  startingSidecar: boolean;
  /** 最近一次错误（UI 内联展示用，不进 toast 的重复提示） */
  lastError: string;
}

function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

export const useAgentStore = defineStore("agent", {
  state: (): AgentStoreState => ({
    enabled: false,
    jobs: [],
    _ready: false,
    startingSidecar: false,
    lastError: "",
  }),

  getters: {
    /** 最近一个 job（面板默认展示它） */
    current(state): AgentSnapshot | null {
      return state.jobs.length ? state.jobs[state.jobs.length - 1] : null;
    },
    /** 正在跑的 job（单飞约束：最多一个） */
    running(state): AgentSnapshot | null {
      for (let i = state.jobs.length - 1; i >= 0; i--) {
        if (!isTerminal(state.jobs[i].status)) return state.jobs[i];
      }
      return null;
    },
    /** 是否有任务在跑（「开始」按钮的 disabled 依据） */
    busy(): boolean {
      return this.running !== null;
    },
  },

  actions: {
    /** 挂载时调一次：读开关 + 注册事件 + 拉一次快照 */
    async init() {
      try {
        this.enabled = await invoke<boolean>("agent_get_enabled");
      } catch (e) {
        this.enabled = false;
        this.lastError = errText(e);
      }
      if (!this._ready) {
        unlistenFn = await listen<AgentSnapshot>("agent-progress", (ev) => {
          this.pushSnapshot(ev.payload);
        });
        this._ready = true;
      }
      await this.refresh();
    },

    /** 卸载时解绑（页面通常常驻，但热重载/退出要干净） */
    dispose() {
      if (unlistenFn) {
        unlistenFn();
        unlistenFn = null;
      }
      if (pollTimer !== null) {
        window.clearInterval(pollTimer);
        pollTimer = null;
      }
      if (flushTimer !== null) {
        window.clearTimeout(flushTimer);
        flushTimer = null;
      }
      pending = null;
      this._ready = false;
    },

    async setEnabled(flag: boolean) {
      const v = await invoke<boolean>("agent_set_enabled", { enabledFlag: flag });
      this.enabled = v;
      if (v) {
        // 开启后立刻预热，用户点「开始」时才不用等进程
        void this.preheat();
      }
    },

    /**
     * 预热侧车：进入工坊页时后台调用，**不阻塞 UI、失败静默**。
     * 失败静默是有意的：开关没开、或用户机器上没打包 agent exe 时，
     * 弹一个"Agent 启动失败"的 toast 只会制造噪声 —— 等到用户真点「开始」
     * 时再给出可行动的文案（那时才知道他确实要用这个功能）。
     */
    async preheat() {
      if (!this.enabled || this.startingSidecar) return;
      this.startingSidecar = true;
      try {
        await invoke<string>("agent_ensure");
        this.lastError = "";
      } catch (e) {
        this.lastError = errText(e);
        console.debug("[agent] 预热未成功（用户还没用 Agent 功能，静默处理）：", this.lastError);
      } finally {
        this.startingSidecar = false;
      }
    },

    /** 提交任务。成功后立即把快照放进来（不等第一次事件）。 */
    async submit(req: AgentSubmitRequest): Promise<AgentSnapshot> {
      this.lastError = "";
      const snap = await invoke<AgentSnapshot>("agent_submit", { req });
      this.pushSnapshot(snap);
      this.ensurePolling();
      return snap;
    },

    /** 取消：Rust 侧双保险（本地立刻收敛 + 通知 Python）。 */
    async cancel(jobId: number) {
      await invoke("agent_cancel", { jobId });
    },

    /** 从后端拉全量快照（重开面板 / 事件丢帧时对齐） */
    async refresh() {
      try {
        const list = await invoke<AgentSnapshot[]>("agent_jobs_snapshot");
        this.jobs = Array.isArray(list) ? list : [];
        this.ensurePolling();
      } catch (e) {
        // 未登录时这里会报「请先登录」，属正常；不打扰用户
        this.lastError = errText(e);
      }
    },

    /** 把一条快照合并进列表（节流到 RENDER_THROTTLE_MS） */
    pushSnapshot(snap: AgentSnapshot) {
      pending = snap;
      const now = Date.now();
      const wait = Math.max(0, RENDER_THROTTLE_MS - (now - lastFlushAt));
      if (flushTimer !== null) return; // 已有待执行的 flush，最新的 pending 会被它消费
      flushTimer = window.setTimeout(() => {
        flushTimer = null;
        lastFlushAt = Date.now();
        const s = pending;
        pending = null;
        if (!s) return;
        this.applySnapshot(s);
      }, wait);
    },

    /** 真正写进 state（同 jobId 覆盖，否则按 jobId 插入） */
    applySnapshot(snap: AgentSnapshot) {
      const idx = this.jobs.findIndex((j) => j.jobId === snap.jobId);
      if (idx >= 0) {
        this.jobs[idx] = snap;
      } else {
        // 按 jobId 有序插入，避免乱序后"最近一个"取错
        const at = this.jobs.findIndex((j) => j.jobId > snap.jobId);
        if (at < 0) this.jobs.push(snap);
        else this.jobs.splice(at, 0, snap);
      }
      this.ensurePolling();
    },

    /**
     * 运行中才开兜底轮询；全部终态就停 ——
     * 常驻定时器会白跑一整天（性能纪律：不留无用 timer）。
     */
    ensurePolling() {
      const need = this.jobs.some((j) => !isTerminal(j.status));
      if (need && pollTimer === null) {
        pollTimer = window.setInterval(() => {
          void this.refresh();
        }, POLL_INTERVAL_MS);
      } else if (!need && pollTimer !== null) {
        window.clearInterval(pollTimer);
        pollTimer = null;
      }
    },
  },
});
