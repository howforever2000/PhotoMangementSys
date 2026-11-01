import { ref } from "vue";
import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import type { HealthReport } from "../types/health";
import { trace } from "../utils/trace";

/**
 * 入库体检状态（FEAT-152）
 *
 * 为什么放 Pinia 而不是留在组件里：
 *   · 体检是「手动触发 + 结果要看一会儿」的只读快照，用户在几个分区之间来回切时
 *     不该每次都重跑一遍（每次都要聚合三张表 + 抽样 stat）；
 *   · 与 `useDedupeStore` 同一分工：store 管数据与生命周期，组件只管呈现。
 *
 * 不做缓存过期：这是**手动刷新**的诊断快照，任何自动过期都会让用户分不清
 * "这屏结论是刚跑的还是旧的"。时间戳如实展示（`ranAt`），要新的就点「重新体检」。
 */
export const useAlbumHealthStore = defineStore("albumHealth", () => {
  const report = ref<HealthReport | null>(null);
  const loading = ref(false);
  const error = ref("");
  /** 上次跑完的时刻（毫秒时间戳；null = 还没跑过） */
  const ranAt = ref<number | null>(null);

  /** 跑一次体检（只读命令；`force=false` 且已有结果时直接复用） */
  const load = trace("albumHealth.load", async (force = false) => {
    if (loading.value) return;
    if (!force && report.value) return;
    loading.value = true;
    error.value = "";
    try {
      const r = await invoke<HealthReport>("album_health_report");
      report.value = r;
      ranAt.value = Date.now();
      console.info(
        `[health] 体检返回：${r.album_count} 册 · 已入库 ${r.total_scanned}/${r.total_photos} · 根失效 ${r.missing_roots} · ${r.elapsed_ms}ms`,
      );
    } catch (e) {
      error.value = String(e);
    } finally {
      loading.value = false;
    }
  });

  return { report, loading, error, ranAt, load };
});
