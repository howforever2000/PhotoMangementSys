/**
 * 全局概览统计缓存（FEAT-112）
 *
 * 为什么需要这一层（用户报「全局概览每次好像是实时计算，时间延迟较高」）：
 *   概览页要 5 个命令 + 一次全量时间线（2.5 万行）才能算出 11 块 widget 的数。
 *   每次切进这一页都重跑一遍 = 每次都要等；而这批数只在**相册/照片发生变动**时
 *   才会变。所以按「变动即失效」缓存，而不是按时间过期。
 *
 * 失效时机（用户指定）：导入相册 / 扫描入库 / 删除照片 / 重建分类索引。
 *   这些操作都会在成功后调 `invalidate()`；下次进概览才重新算。
 *   注意：不监听 albumStore.albums 变化来自动失效 —— fetchAlbums 在启动时也会
 *   被调用，那样等于每次启动都失效一次缓存，缓存就没意义了。必须由**变更动作**显式触发。
 */
import { defineStore } from "pinia";
import { ref, shallowRef } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type {
  CategoryIndexStats,
  CategoryOverview,
  ContentSearchHit,
  LocationGroupRow,
} from "../types/content";
import type { PersonInfo } from "../types/photo";

export interface OverviewSnapshot {
  timeline: ContentSearchHit[];
  persons: PersonInfo[];
  locations: LocationGroupRow[];
  categories: CategoryOverview[];
  catStats: CategoryIndexStats | null;
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

  /**
   * 取数据：缓存可用则**零 IPC** 直接返回；失效或强制刷新时才真正拉取。
   * 并发安全：building 期间的重复调用直接复用同一个 Promise（切 tab 手快时
   * 会连点，不能每次开一轮 IPC）。
   */
  let inflight: Promise<void> | null = null;

  async function load(force = false): Promise<void> {
    if (!force && !dirty.value && snapshot.value) return;
    if (inflight) return inflight;

    building.value = true;
    error.value = "";
    inflight = (async () => {
      try {
        const [timeline, persons, locations, categories] = await Promise.all([
          invoke<ContentSearchHit[]>("list_timeline"),
          invoke<PersonInfo[]>("list_persons"),
          invoke<LocationGroupRow[]>("list_photo_locations"),
          invoke<CategoryOverview[]>("list_categories"),
        ]);
        let catStats: CategoryIndexStats | null = null;
        try {
          catStats = await invoke<CategoryIndexStats>("category_index_stats");
        } catch {
          /* 语义索引统计不可用不影响其余 widget */
        }
        snapshot.value = { timeline, persons, locations, categories, catStats, builtAt: Date.now() };
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
