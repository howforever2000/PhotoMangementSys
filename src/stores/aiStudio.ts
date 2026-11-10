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
import type {
  AiJobStatus,
  AiProviderInput,
  AiProviderKind,
  AiRunRequest,
  AiSettings,
  AiSettingsInput,
  AiTestResult,
} from "../types/ai";

/** 轮询句柄（非响应式，放模块级） */
const pollTimers = new Map<string, ReturnType<typeof setTimeout>>();

export const useAiStudioStore = defineStore("aiStudio", () => {
  const settings = ref<AiSettings | null>(null);
  const settingsLoading = ref(false);
  const jobs = ref<Record<string, AiJobStatus>>({});

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

  async function testProvider(
    kind: AiProviderKind,
    provider?: AiProviderInput,
  ): Promise<AiTestResult> {
    return await invoke<AiTestResult>("ai_test_provider", {
      kind,
      provider: provider ?? null,
    });
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

  async function run(req: AiRunRequest): Promise<string> {
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
    startPoll(jobId);
    return jobId;
  }

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

  return {
    settings,
    settingsLoading,
    jobs,
    loadSettings,
    saveSettings,
    testProvider,
    run,
    cancel,
    jobOf,
  };
});
