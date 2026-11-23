/**
 * AI 创作 store（FEAT-152）
 *
 * 职责三层：
 *   1) 设置缓存（多 profile：vision / edit / upscale），避免每个工具各拉一次；
 *   2) 作业提交与**轮询**（约 700ms）：后端命令返回 jobId，这里轮询 ai_job_status；
 *   3) 作业状态跨工具共享（切换左栏工具不丢进度）。
 *
 * 为什么不放组件局部 ref：作业是长任务，切走工具再回来要能看到仍在跑 —— 与
 * content store 的 combinedJobs 同一考量（BUG-2026-0821-001）。
 */
import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { trace } from "../utils/trace";
import { toolLabel } from "../utils/toolCatalog";
import type {
  AiJobStatus,
  AiRunRequest,
  AiSettings,
  AiSettingsInput,
  AiTestResult,
  LocalModelsStatus,
} from "../types/ai";

/** 轮询句柄（非响应式，放模块级） */
const pollTimers = new Map<string, ReturnType<typeof setTimeout>>();

/**
 * 从 options.plan 里收「这次要跑哪些组件（算子）」，保序去重。
 * 创意组合 / 智能修图的提交 options 带完整 plan；单步工具没有 plan 返回空。
 * 只做读取与去重，不做校验（白名单/夹取在 Python 端 validate_plan）。
 */
function planOpsOf(options: Record<string, unknown> | undefined): string[] {
  const plan = options?.plan as { steps?: { op?: unknown }[] } | undefined;
  const steps = plan?.steps;
  if (!Array.isArray(steps)) return [];
  const out: string[] = [];
  for (const s of steps) {
    const op = typeof s?.op === "string" ? s.op : "";
    if (op && !out.includes(op)) out.push(op);
  }
  return out;
}

export const useAiStudioStore = defineStore("aiStudio", () => {
  const settings = ref<AiSettings | null>(null);
  const settingsLoading = ref(false);
  const jobs = ref<Record<string, AiJobStatus>>({});
  /**
   * 每个工具「当前挂着的作业」（工具名 → jobId）。
   *
   * 为什么不放组件局部 ref：作业是长任务，切走工具/切走页面再回来要能看到仍在跑、
   * 或在跑完后看到结果。把「该工具当前作业」也提进 store（脱离组件生命周期），
   * 工具组件重挂载时按工具名取回 jobId 重连即可（配合后端进程级作业）。
   */
  const activeByTool = ref<Record<string, string>>({});

  /* ---------------- 设置 ---------------- */

  async function loadSettings(force = false): Promise<AiSettings> {
    if (settings.value && !force) return settings.value;
    settingsLoading.value = true;
    try {
      settings.value = await invoke<AiSettings>("ai_get_settings");
      return settings.value;
    } finally {
      settingsLoading.value = false;
    }
  }

  async function saveSettings(input: AiSettingsInput): Promise<AiSettings> {
    settings.value = await invoke<AiSettings>("ai_set_settings", { input });
    return settings.value;
  }

  /**
   * 连通性测试（FEAT-196：按「供应商 + 模型」测，不再按能力位测）。
   *
   * 三个入参都是**界面上的当前值**，允许还没保存：
   * - `providerId` 用于取该供应商已存的密钥（新建供应商传空串）
   * - `baseUrl` / `model` 用界面值，空则回落已存值
   * - `apiKey` 空或掩码 = 沿用已存密钥
   *
   * 为什么不再传 kind：改多供应商后，「这条供应商能不能通」与「它被哪个能力位
   * 引用」是两件事，用户常在保存前先测一下新建的供应商。
   */
  async function testProvider(req: {
    providerId: string;
    baseUrl: string;
    model: string;
    apiKey: string;
  }): Promise<AiTestResult> {
    return await invoke<AiTestResult>("ai_test_provider", {
      providerId: req.providerId || null,
      baseUrl: req.baseUrl,
      model: req.model,
      apiKey: req.apiKey,
    });
  }

  /** 本地增强资源状态（模型是否就绪 + 下载地址、内置 LUT 清单）。 */
  async function localModels(): Promise<LocalModelsStatus> {
    return await invoke<LocalModelsStatus>("local_models_status");
  }

  /* ---------------- 作业 ---------------- */

  function isTerminal(status: string): boolean {
    return status === "done" || status === "failed" || status === "cancelled";
  }

  function stopPoll(jobId: string): void {
    const t = pollTimers.get(jobId);
    if (t) {
      clearTimeout(t);
      pollTimers.delete(jobId);
    }
  }

  function startPoll(jobId: string): void {
    stopPoll(jobId);
    let failures = 0;
    const tick = async () => {
      try {
        const v = await invoke<AiJobStatus>("ai_job_status", { jobId });
        // 高频轮询：只在状态/阶段变化时打一行，避免日志洪水（bug-notes 高频成功路径条）。
        const prev = jobs.value[jobId];
        if (!prev || prev.status !== v.status || prev.stage !== v.stage) {
          console.info(
            `[工坊][作业] ${jobId} ${v.status} · ${v.stage} · ${v.progress}%`,
          );
        }
        jobs.value[jobId] = v;
        failures = 0;
        if (isTerminal(v.status)) {
          stopPoll(jobId);
          return;
        }
      } catch {
        failures += 1;
        // 轮询连续失败 3 次以上（服务异常）：标记失败，避免永远转圈
        if (failures >= 3) {
          const cur = jobs.value[jobId];
          if (cur) {
            jobs.value[jobId] = { ...cur, status: "failed", stage: "状态查询失败", error: "无法获取作业状态，请重试" };
          }
          stopPoll(jobId);
          return;
        }
      }
      pollTimers.set(jobId, setTimeout(tick, 700));
    };
    pollTimers.set(jobId, setTimeout(tick, 400));
  }

  /** 提交作业：收敛入口，AOP 记录工具名 + 中文名 + 输入规模 + 耗时。 */
  const run = trace(
    "工坊.提交作业",
    async function submitJob(req: AiRunRequest): Promise<string> {
      const label = toolLabel(req.tool);
      // 组件清单：创意组合 / 智能修图提交时 plan 里带着算子序列，
      // 日志里点出「这次用了哪些组件」，排障时不用再翻 options 原文。
      const ops = planOpsOf(req.options);
      const opsTag = ops.length ? ` ops=[${ops.join(",")}]` : "";
      console.info(
        `[工坊][提交] tool=${req.tool}（${label}） n=${req.paths.length} out=${req.outputDir}${opsTag}`,
      );
      const jobId = await invoke<string>("ai_run_job", { req });
      jobs.value[jobId] = {
        jobId,
        status: "queued",
        progress: 0,
        stage: "排队中",
        message: "",
        outputs: [],
        warnings: [],
        error: null,
      };
      activeByTool.value[req.tool] = jobId;
      startPoll(jobId);
      return jobId;
    },
  );

  async function cancel(jobId: string): Promise<void> {
    try {
      await invoke("ai_cancel_job", { jobId });
    } finally {
      // 让下一次轮询尽快反映取消
    }
  }

  function jobOf(jobId: string | null): AiJobStatus | null {
    return jobId ? jobs.value[jobId] ?? null : null;
  }

  /** 该工具当前挂着的 jobId（无则 null） */
  function activeJobId(tool: string): string | null {
    return activeByTool.value[tool] ?? null;
  }

  /** 该工具当前作业的状态（用于工具重挂载后重连展示） */
  function jobOfTool(tool: string): AiJobStatus | null {
    const id = activeByTool.value[tool];
    return id ? jobs.value[id] ?? null : null;
  }

  return {
    settings,
    settingsLoading,
    jobs,
    activeByTool,
    loadSettings,
    saveSettings,
    testProvider,
    localModels,
    run,
    cancel,
    jobOf,
    activeJobId,
    jobOfTool,
  };
});
