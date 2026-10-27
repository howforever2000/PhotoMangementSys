/**
 * 全局概览统计缓存（FEAT-112 起，FEAT-123 重做取数方式）
 *
 * ## 改前（FEAT-112）
 * 为了画 11 块 widget，一次拉 5 个命令，其中 `list_timeline` 是**全量 2.5 万行**
 * （实测 14,438 KB / 107 ms），再由前端遍历 6 遍算聚合；外加 `category_index_stats`
 * （实测 312~574 ms，慢在 known 的三表 UNION）。日志里一次「概览构建簇」横跨 3~44 秒。
 *
 * ## 改后（FEAT-123）
 * 聚合搬到后端：一次 `overview_stats` 返回 2~4 KB 快照。后端带**数据签名缓存**
 * （落 `overview_cache` 表），照片库没变化时连聚合都不跑 —— 也就是用户要的
 * 「只在导入照片 / 扫描入库之后才更新」。前端只再补三个本来就很小、且与照片内容
 * 无关的列表（人物 / 地点聚合 / 分类定义）。
 *
 * ## 失效时机
 * 导入相册 / 扫描入库 / 删照片 / 重建分类索引后调 `invalidate()`；
 * 即便漏调，后端签名（张数 + 最大时间戳）也会在下一次请求时自愈。
 */
import { defineStore } from "pinia";
import { ref, shallowRef } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { CategoryOverview, LocationGroupRow } from "../types/content";
import type { OverviewStats } from "../types/overview";
import type { PersonInfo } from "../types/photo";

export interface OverviewSnapshot {
  stats: OverviewStats;
  persons: PersonInfo[];
  locations: LocationGroupRow[];
  categories: CategoryOverview[];
  /** 构建时间（用于「刚更新」提示与调试） */
  builtAt: number;
}

export const useOverviewStore = defineStore("overview", () => {
  /** 最近一次构建的快照；null = 从没算过 */
  const snapshot = shallowRef<OverviewSnapshot | null>(null);
  /** 失效标记：true 时下次进入概览会重算（而不是直接读缓存） */
  const dirty = ref(true);
  const building = ref(false);
  /** 构建失败原因（保留上一次可用快照，避免一次失败把整页清空） */
  const error = ref("");

  /** 标记失效。导入相册 / 扫描入库 / 删除照片 / 重建索引后调用。 */
  function invalidate() {
    dirty.value = true;
  }

  let inflight: Promise<void> | null = null;

  async function load(force = false): Promise<void> {
    if (!force && !dirty.value && snapshot.value) return;
    if (inflight) return inflight;

    building.value = true;
    error.value = "";
    inflight = (async () => {
      try {
        const now = new Date();
        const todayMd = `${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
        const [stats, persons, locations, categories] = await Promise.all([
          invoke<OverviewStats>("overview_stats", {
            todayMd,
            todayYear: now.getFullYear(),
            force,
          }),
          invoke<PersonInfo[]>("list_persons"),
          invoke<LocationGroupRow[]>("list_photo_locations"),
          invoke<CategoryOverview[]>("list_categories"),
        ]);
        snapshot.value = { stats, persons, locations, categories, builtAt: Date.now() };
        dirty.value = false;
      } catch (e) {
        error.value = String(e);
        throw e;
      } finally {
        building.value = false;
        inflight = null;
      }
    })();
    return inflight;
  }

  return { snapshot, dirty, building, error, invalidate, load };
});
