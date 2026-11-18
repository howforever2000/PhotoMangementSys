import { onScopeDispose, ref } from "vue";
import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import type { ResourceUsageSnapshot } from "../types/resourceUsage";
import { trace } from "../utils/trace";

/**
 * 资源使用情况状态（FEAT-178，FEAT-179：+ 服务启停）
 *
 * store 管数据 / 轮询 / 启停生命周期，组件只管呈现。
 * CPU 占用是「两次采样间隔」的增量，必须持续轮询才有意义，故每 2s 采样，
 * onScopeDispose 清理定时器，不留悬挂轮询。
 */
export const useResourceUsageStore = defineStore("resourceUsage", () => {
  const snapshot = ref<ResourceUsageSnapshot | null>(null);
  const loading = ref(false);
  const error = ref("");
  const polling = ref(false);
  /** 正在启停的组件 key（null = 无操作进行中） */
  const controlling = ref<string | null>(null);
  /** 最近一次启停的结果提示 */
  const notice = ref("");

  let timer: number | null = null;
  const POLL_MS = 2000;

  /** 采一次样（命令只读、不拉起/不杀死任何进程） */
  const sample = trace("resourceUsage.sample", async () => {
    if (loading.value) return;
    loading.value = true;
    error.value = "";
    try {
      const r = await invoke<ResourceUsageSnapshot>("resource_usage_snapshot");
      snapshot.value = r;
      console.info(
        `[resource] 采样返回：${r.components.length} 组件 · 系统CPU ${r.system_cpu_percent.toFixed(1)}% · 间隔 ${r.sample_interval_ms}ms`,
      );
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  });

  /** 启动 / 停止一个服务（key ∈ vcr / ai_studio，action ∈ start / stop） */
  const control = trace("resourceUsage.control", async (key: string, action: "start" | "stop") => {
    if (controlling.value) return;
    controlling.value = key;
    notice.value = "";
    error.value = "";
    try {
      const msg = await invoke<string>("service_control", { key, action });
      notice.value = msg;
      console.info(`[resource] ${key} ${action}: ${msg}`);
      // 启停后立刻补采一次，让界面尽快反映新状态
      await sample();
    } catch (e) {
      error.value = `${key} ${action} 失败：${e}`;
    } finally {
      controlling.value = null;
    }
  });

  /** 开始轮询：立即采一次，之后每 POLL_MS 一次（重复调用幂等） */
  function startPolling() {
    if (polling.value) return;
    polling.value = true;
    void sample();
    timer = window.setInterval(() => void sample(), POLL_MS);
  }

  /** 停止轮询（组件卸载 / 窗口关闭时调用） */
  function stopPolling() {
    polling.value = false;
    if (timer !== null) {
      window.clearInterval(timer);
      timer = null;
    }
  }

  onScopeDispose(stopPolling);

  return {
    snapshot,
    loading,
    error,
    polling,
    controlling,
    notice,
    sample,
    control,
    startPolling,
    stopPolling,
  };
});
