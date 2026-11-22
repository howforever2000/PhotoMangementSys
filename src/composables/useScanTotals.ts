/**
 * 全库入库读数（页面「已入库」的唯一口径）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么不能继续拿相册列表里的数相加
 * ──────────────────────────────────────────────────────────────────────────
 * 后端每册的 `scanned_photo_count` 是「这册目录子树下有多少条扫描记录」，
 * 对单册徽标是对的（父子相册各显示完整覆盖）。但把它 Σ 起来当页头读数就会出错：
 *
 * - 父子相册 / 同一文件夹重复导入 → 同一张照片被每个包含它的相册各算一次；
 * - 扫描记录的主键是「路径 + 大小 + mtime」的哈希，路径写法漂移或 mtime 变化
 *   会为同一张照片再写一行，旧行不自动消失 → 行数只增不减；
 * - 文件被移走 / 在系统里删掉后，行同样不回收。
 *
 * 结果就是用户看到的「照片 25384 / 已入库 60585」。这里改读后端
 * `scan_library_totals`：按路径去重（COUNT(DISTINCT path)），并在返回值里给出
 * 历史冗余行数，前端可提示清理。
 *
 * 刷新时机：挂载一次 + 相册列表变化（扫描入库 / 删除照片 / 导入相册都会
 * 重新拉列表，正好是读数会变的时刻）。
 */
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/** 与后端 `scan_stats::ScanLibraryTotals`（db/content.rs 定义）逐字段对应 */
export interface ScanLibraryTotals {
  /** photo_content_scan 行数（含历史冗余行） */
  rows: number;
  /** 去重后的照片路径数：**页面该显示这个** */
  paths: number;
  /** 同一路径的多余行数（rows - paths） */
  duplicate_rows: number;
}

export function useScanTotals() {
  const totals = ref<ScanLibraryTotals | null>(null);
  const loading = ref(false);

  /** 拉一次读数（失败保留上一次结果：读数不该因为一次网络/IPC 抖动变 0） */
  async function refresh() {
    if (loading.value) return;
    loading.value = true;
    try {
      totals.value = await invoke<ScanLibraryTotals>("scan_library_totals");
    } catch (e) {
      console.warn("[scan-totals] 读取入库总量失败", e);
    } finally {
      loading.value = false;
    }
  }

  return { totals, loading, refresh };
}
