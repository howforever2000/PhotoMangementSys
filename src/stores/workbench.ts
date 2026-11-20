/**
 * AI 修图工作台 store（独立视图 · FEAT-160 系列 / FEAT-177 重构）
 * ===================================================================
 * 只服务 `/workshop/workbench` 这个**独立视图**，与原有创意工坊（`workshopUi`）分开：
 *   - 创意工坊是「选工具 → 对一组图做一次加工」的工具台，输入是共享托盘；
 *   - 工作台是「导入一批图 → 逐图套用同一修图方案 → 前后对比 → 导出并入册」的
 *     批处理空间，需要的是**每张图各自的状态**（待处理/处理中/完成/失败），
 *     这是托盘那种 `string[]` 表达不了的。
 *
 * 为什么状态放 Pinia 而不是组件局部 ref：`App.vue` 的 `<router-view/>` 没有
 * keep-alive，离开 `/workshop/workbench` 组件即销毁；把工作集、方案、输出设置
 * 提到 store，切去相册看一眼再回来，选好的照片和进度都还在（与 `workshopUi`
 * 同一考量）。
 *
 * FEAT-177 的两处结构性调整：
 *   ① 出片方式从「配方 / AI / 本地增强（再选 7 个工具之一）」改成
 *      「配方 / 模型理解 / 创意组合（步骤链）」，本地工具不再是并列选项；
 *   ② 输出目录只留一个：`outputDir` 既是产物落盘处，也是「创意工坊」相册的
 *      指向目录。原 `albumDir` 与它并列，导致用户要选两次、且漏选时
 *      「自动导入」会静默失效。
 */
import { defineStore } from "pinia";
import { baseName } from "../utils/pathParts";
import {
  cloneSteps,
  insertOp,
  paramsOverride,
  recipeById,
  removeOp,
  restoreTune,
  setEnabled,
  setParam,
  tuneOffKey,
  tunePlan as buildTunePlan,
  chainFromRecipe,
  type TuneOff,
  type WbChainStep,
} from "../utils/workbenchPlan";

export type WbItemStatus = "idle" | "queued" | "running" | "done" | "failed";

/**
 * 出片方案（FEAT-177 / FEAT-197）：
 *   - recipe ：现成配方（懒人主路径，与创意工坊「一键出片」同一条链路）
 *   - ai     ：模型理解意图后规划步骤（语言模型 + 文生图模型，按调用计费）
 *   - chain  ：创意组合原子组件（本机 + 云端都能插，同名组件可重复，最多 10 步）
 * 三者共用同一套「导入 → 群图状态 → 前后对比 → 导出并配册」流程。
 */
export type WbEngine = "recipe" | "ai" | "chain";

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

/**
 * 样片两阶段状态机（FEAT-174，workshop-positioning.md 修订记录）：
 *   idle     —— 主路径：不点「先看样片」就没有任何面板，≤3 次点击出片不变
 *   picking  —— 自选样片：中栏点缩略图勾选，张数不限
 *   sampling —— 样片作业跑着
 *   tuning   —— 样片出来了，参数就地可调（改完即时预览）
 *   applying —— 「应用到全部」跑全量（带 paramsOverride / plan）
 * 主路径（直接点「批量套用」）完全绕过这台状态机，两者互不阻塞。
 */
export type WbSamplePhase = "idle" | "picking" | "sampling" | "tuning" | "applying";

interface WbState {
  items: WbItem[];
  /** 已勾选的图片路径 */
  selected: string[];
  /** 本次批量作业覆盖的图片路径（作业回调据此回填状态） */
  runningBatch: string[];
  /* ---------------- 方案 ---------------- */
  engine: WbEngine;
  /** 配方引擎：配方 id（与 recipes.json 同源） */
  recipeId: string;
  /** 创意组合：步骤链（顺序由依赖决定，见 utils/workbenchPlan） */
  chain: WbChainStep[];
  /* ---------------- AI 引擎 ---------------- */
  /** 修图意图（一句话） */
  prompt: string;
  /** 强度 0~100（进入规划提示词的补充要求） */
  strength: number;
  /** 是否做一轮反思自检（免费本地 critic + 云端复核，不改出图质量） */
  reflect: boolean;
  /** 反思轮次（界面固定 1；保留字段供旧作业 / 接口兼容） */
  rounds: number;
  /** 文生图采样步数（FEAT-197：界面「步数」；默认 1，不写进请求体） */
  renderSteps: number;
  /** 影调风格预设 id（FEAT-198；空串 = 不追加风格） */
  styleId: string;
  /** 上采样：默认不勾（每张多一次云端调用） */
  upsample: boolean;
  /** 上采样倍率：2 或 4（4 由「2× 跑两遍」实现） */
  upsampleScale: number;
  /* ---------------- 输出 ---------------- */
  /** 输出目录：产物写到这里；「创意工坊」相册也指向这里 */
  outputDir: string;
  /** 导出后自动导入「创意工坊」相册 */
  autoImport: boolean;
  /* ---------------- 样片两阶段（FEAT-174 / FEAT-177） ---------------- */
  samplePhase: WbSamplePhase;
  /** 样片路径（自选不限张数；与 items 同源，仅用于作业提交） */
  samplePaths: string[];
  /** 样片是自动等距抽的还是用户自选的（决定提示文案） */
  sampleAuto: boolean;
  /* ---------------- 单步微调（FEAT-180） ---------------- */
  /** 配方步骤 + 当前微调参数（catalog 值域；与 op_catalog.json 同源） */
  tuneSteps: WbChainStep[];
  /** 进入微调时冻结的配方默认值：判「改了没有」与复位都靠它 */
  tuneBase: WbChainStep[];
  /** 下拉参数的「不处理」：`op.key` → true（枚举没有中性值，整步不参与；滑杆不用标记） */
  tuneOff: TuneOff;
  /** 当前微调绑定的配方 id：只有换配方才重建基线，切页面回来不冲掉用户改动 */
  tuneRecipeId: string;
  /* ---------------- 工作区视图（FEAT-180） ---------------- */
  /** 中栏视图：单张三段对比 / 选片网格 */
  viewMode: "loupe" | "grid";
  /** 左右栏折叠（LR 式收纳，只留一条竖排标签） */
  leftRailCollapsed: boolean;
  rightRailCollapsed: boolean;
}

export const useWorkbenchStore = defineStore("workbench", {
  state: (): WbState => ({
    items: [],
    selected: [],
    runningBatch: [],
    engine: "recipe",
    recipeId: "natural",
    chain: chainFromRecipe("film"),
    prompt: "",
    strength: 64,
    reflect: true,
    rounds: 1,
    renderSteps: 1,
    styleId: "",
    upsample: false,
    upsampleScale: 2,
    outputDir: "",
    autoImport: true,
    samplePhase: "idle",
    samplePaths: [],
    sampleAuto: true,
    tuneSteps: [],
    tuneBase: [],
    tuneOff: {},
    tuneRecipeId: "",
    viewMode: "loupe",
    leftRailCollapsed: false,
    rightRailCollapsed: false,
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
    /** 本次要处理的目标：有勾选就是勾选的，否则全量 */
    targets: (s) => (s.selected.length ? s.items.filter((i) => s.selected.includes(i.path)) : s.items),
    /** 单步微调的差量覆盖（只含改过的键；空对象 = 完全走配方原值） */
    tuneOverrides: (s) => paramsOverride(s.tuneSteps, s.tuneBase),
    /** 改过的参数个数（不含被标为「不处理」的键，避免与「跳过」重复计数） */
    tuneChangedCount: (s) => {
      const base = new Map(s.tuneBase.map((x) => [x.uid, x.params]));
      let n = 0;
      for (const st of s.tuneSteps) {
        const before = base.get(st.uid);
        for (const [k, v] of Object.entries(st.params)) {
          if (s.tuneOff[tuneOffKey(st.uid, k)]) continue;
          if (!before || before[k] !== v) n++;
        }
      }
      return n;
    },
    /** 标为「不处理」的参数个数 */
    tuneSkippedCount: (s) => Object.values(s.tuneOff).filter(Boolean).length,
    /** 有任一参数不处理 → 提交改成显式 plan（recipeId+paramsOverride 无法跳过步骤） */
    tuneUsesPlan: (s) => Object.values(s.tuneOff).some(Boolean),
    /** 微调后的执行计划（intent 用配方名；空 steps = 全都不处理） */
    tunePlan: (s) => buildTunePlan(s.tuneSteps, s.tuneOff, recipeById(s.recipeId)?.label ?? ""),
    /** 至少还有一步参与出片（后端 validate_plan 拒绝空计划） */
    tuneRunnable: (s) =>
      buildTunePlan(s.tuneSteps, s.tuneOff, recipeById(s.recipeId)?.label ?? "").steps.length > 0,
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
    /** 单张移出工作集（不动原图） */
    removePath(path: string): void {
      this.items = this.items.filter((i) => i.path !== path);
      this.selected = this.selected.filter((p) => p !== path);
      this.runningBatch = this.runningBatch.filter((p) => p !== path);
      this.samplePaths = this.samplePaths.filter((p) => p !== path);
    },
    /** 批量移出工作集（「删除所选」） */
    removePaths(list: string[]): void {
      const kill = new Set(list);
      this.items = this.items.filter((i) => !kill.has(i.path));
      this.selected = this.selected.filter((p) => !kill.has(p));
      this.runningBatch = this.runningBatch.filter((p) => !kill.has(p));
      this.samplePaths = this.samplePaths.filter((p) => !kill.has(p));
    },
    clearAll(): void {
      this.items = [];
      this.selected = [];
      this.runningBatch = [];
      this.samplePaths = [];
      this.samplePhase = "idle";
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
    invertSelection(): void {
      this.selected = this.items.filter((i) => !this.selected.includes(i.path)).map((i) => i.path);
    },
    clearSelection(): void {
      this.selected = [];
    },
    /* ---------------- 步骤链（创意组合） ---------------- */
    /** 加一步（允许同一组件多次插入；到 10 步上限时原样返回） */
    addChainOp(op: string): void {
      this.chain = insertOp(this.chain, op);
    },
    /**
     * 链条重置为**只含一个算子**（单步工具「在创意组合中打开」的落地动作）。
     * 不是追加：跳转语义是「以这个工具为起点组合」，残留旧链反而困惑。
     */
    resetChainToOp(op: string): void {
      this.chain = insertOp([], op);
    },
    removeChainOp(uid: string): void {
      this.chain = removeOp(this.chain, uid);
    },
    toggleChainOp(uid: string): void {
      const cur = this.chain.find((s) => s.uid === uid);
      if (cur) this.chain = setEnabled(this.chain, uid, !cur.enabled);
    },
    setChainParam(uid: string, key: string, value: number | string | boolean): void {
      this.chain = setParam(this.chain, uid, key, value);
    },
    loadChainFromRecipe(recipeId: string): void {
      this.chain = chainFromRecipe(recipeId);
    },
    /* ---------------- 样片两阶段（FEAT-174） ---------------- */
    /** 进入自选样片 */
    startPicking(): void {
      this.samplePhase = "picking";
      this.samplePaths = [];
      this.sampleAuto = false;
    },
    /** 选/取消一张样片（自选不限张数） */
    toggleSample(path: string): void {
      this.samplePaths = this.samplePaths.includes(path)
        ? this.samplePaths.filter((p) => p !== path)
        : [...this.samplePaths, path];
    },
    /** 开始跑样片（自动抽样由组件给 paths） */
    startSample(paths: string[], auto: boolean): void {
      this.samplePaths = paths;
      this.sampleAuto = auto;
      this.samplePhase = "sampling";
    },
    /** 样片出来了：进入微调（微调面板由页面自动展开一次） */
    beginTuning(): void {
      this.samplePhase = "tuning";
    },
    /**
     * 准备单步微调的编辑副本与基线（页面在配方变化时调用）。
     * 同配方重复调用直接返回：切去相册再回来，用户调过的参数不能被重置。
     */
    ensureTune(recipeId: string, steps: WbChainStep[]): void {
      if (this.tuneRecipeId === recipeId && this.tuneBase.length) return;
      this.tuneRecipeId = recipeId;
      this.tuneBase = cloneSteps(steps);
      this.tuneSteps = cloneSteps(steps);
      this.tuneOff = {};
    },
    /** 改一个算子参数（值域与 catalog 一致，滑杆换算在组件层） */
    setTuneParam(uid: string, key: string, value: number | string | boolean): void {
      this.tuneSteps = setParam(this.tuneSteps, uid, key, value);
    },
    /** 下拉「不处理」：只对枚举参数生效（数值参数直接拉到 0，不占标记） */
    setTuneOff(uid: string, key: string, off: boolean): void {
      const k = tuneOffKey(uid, key);
      const next: TuneOff = { ...this.tuneOff };
      if (off) next[k] = true;
      else delete next[k];
      this.tuneOff = next;
    },
    /** 复位单步：参数与「不处理」标记都回基线 */
    resetTuneStep(uid: string): void {
      this.tuneSteps = restoreTune(this.tuneSteps, this.tuneBase, uid);
      this.tuneOff = Object.fromEntries(
        Object.entries(this.tuneOff).filter(([k]) => !k.startsWith(`${uid}.`)),
      );
    },
    /** 复位全部：参数与「不处理」标记都回基线 */
    resetTune(): void {
      this.tuneSteps = restoreTune(this.tuneSteps, this.tuneBase);
      this.tuneOff = {};
    },
    /** 中栏在「单张对比」与「网格选片」之间切换 */
    setViewMode(mode: "loupe" | "grid"): void {
      this.viewMode = mode;
    },
    toggleRail(side: "left" | "right"): void {
      if (side === "left") this.leftRailCollapsed = !this.leftRailCollapsed;
      else this.rightRailCollapsed = !this.rightRailCollapsed;
    },
    /** 结束样片流程（应用完成或用户放弃），回到主路径；微调参数保留给下一次 */
    endSample(): void {
      this.samplePhase = "idle";
      this.samplePaths = [];
    },
  },
});
