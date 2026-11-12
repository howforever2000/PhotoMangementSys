/**
 * AI 修图工作台 store（独立视图 · FEAT-160 系列）
 * ===================================================================
 * 只服务 `/workshop/workbench` 这个**独立视图**，与原有创意工坊（`workshopUi`）分开：
 *   - 创意工坊是「选工具 → 对一组图做一次加工」的工具台，输入是共享托盘；
 *   - 工作台是「导入一批图 → 逐图套用同一修图风格 → 前后对比 → 导出并入册」的
 *     批处理空间，需要的是**每张图各自的状态**（待处理/处理中/完成/失败），
 *     这是托盘那种 `string[]` 表达不了的。
 *
 * 为什么状态放 Pinia 而不是组件局部 ref：`App.vue` 的 `<router-view/>` 没有
 * keep-alive，离开 `/workshop/workbench` 组件即销毁；把工作集、参数、输出设置
 * 提到 store，切去相册看一眼再回来，选好的照片和进度都还在（与 `workshopUi`
 * 同一考量）。
 */
import { defineStore } from "pinia";
import { baseName } from "../utils/pathParts";

export type WbItemStatus = "idle" | "queued" | "running" | "done" | "failed";

export interface WbItem {
  /** 原图绝对路径（唯一键） */
  path: string;
  /** 文件名（列表显示） */
  name: string;
  status: WbItemStatus;
  /** 该图进度 0~100 */
  progress: number;
  /** 修图产物路径（done 后由作业 outputs 回填） */
  outputPath?: string;
  /** 失败原因 */
  error?: string;
}

interface WbState {
  items: WbItem[];
  /** 已勾选的图片路径 */
  selected: string[];
  /** 本次批量作业覆盖的图片路径（作业回调据此回填状态） */
  runningBatch: string[];
  /** 修图意图（一句话） */
  prompt: string;
  /** 强度 0~100 */
  strength: number;
  /** 是否多轮反思 */
  reflect: boolean;
  /** 反思轮次 */
  rounds: number;
  /** 导出目录（产物写到这） */
  outputDir: string;
  /** 「创意工坊」相册映射目录（自动导入的落地目录） */
  albumDir: string;
  /** 相册名 */
  albumName: string;
  /** 导出后自动导入相册 */
  autoImport: boolean;
}

export const useWorkbenchStore = defineStore("workbench", {
  state: (): WbState => ({
    items: [],
    selected: [],
    runningBatch: [],
    prompt: "",
    strength: 64,
    reflect: true,
    rounds: 1,
    outputDir: "",
    albumDir: "",
    albumName: "创意工坊",
    autoImport: true,
  }),

  getters: {
    total: (s) => s.items.length,
    doneCount: (s) => s.items.filter((i) => i.status === "done").length,
    runningCount: (s) =>
      s.items.filter((i) => i.status === "running" || i.status === "queued").length,
    failedCount: (s) => s.items.filter((i) => i.status === "failed").length,
    selectedCount: (s) => s.selected.length,
    /** 正在跑（用于禁用按钮） */
    busy: (s) => s.items.some((i) => i.status === "running" || i.status === "queued"),
  },

  actions: {
    /** 追加照片（去重；新加入的默认勾选，方便直接批量套用） */
    addPaths(list: string[]): void {
      const have = new Set(this.items.map((i) => i.path));
      for (const p of list) {
        if (!p || have.has(p)) continue;
        have.add(p);
        this.items.push({ path: p, name: baseName(p), status: "idle", progress: 0 });
        if (!this.selected.includes(p)) this.selected.push(p);
      }
    },
    removePath(path: string): void {
      this.items = this.items.filter((i) => i.path !== path);
      this.selected = this.selected.filter((p) => p !== path);
      this.runningBatch = this.runningBatch.filter((p) => p !== path);
    },
    clearAll(): void {
      this.items = [];
      this.selected = [];
      this.runningBatch = [];
    },
    /** 打补丁（状态/进度/产物） */
    patch(path: string, partial: Partial<Omit<WbItem, "path" | "name">>): void {
      const it = this.items.find((i) => i.path === path);
      if (it) Object.assign(it, partial);
    },
    toggleSelect(path: string): void {
      this.selected = this.selected.includes(path)
        ? this.selected.filter((p) => p !== path)
        : [...this.selected, path];
    },
    selectAll(): void {
      this.selected = this.items.map((i) => i.path);
    },
    clearSelection(): void {
      this.selected = [];
    },
  },
});
