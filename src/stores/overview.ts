/**
 * 全局概览统计缓存（FEAT-112 起，FEAT-123 重做取数方式，FEAT-153 加前端持久化）
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
 * ## FEAT-153：先渲染后校验（SWR）
 * 痛点：Pinia store 每次应用启动都是空的，即使照片库一字未变，启动进概览仍要
 * 等一轮完整 IPC（overview_stats + list_persons + list_photo_locations +
 * list_categories）才出内容 —— 用户反馈「每次都是重新渲染的，前端要等待一会」。
 * 现在快照同时落 `localStorage`：
 *  · 启动时先恢复上次快照 → 概览**立刻**用旧数据渲染（零等待）；
 *  · 后台静默重算同一组命令，算完**原子替换**快照（数字如有变化自然刷新），
 *    不挡骨架屏、不阻塞任何交互；
 *  · 只有「没有任何持久化快照」时才走原来的骨架屏全量加载路径。
 * 失效仍是事件驱动：导入相册 / 扫描入库 / 删照片 / 重建索引后调 `invalidate()`
 * （只标 dirty，不删快照 —— 失效期间旧数据继续可用）；即便漏调，后端签名
 * （张数 + 最大时间戳 + 当天日期）也会在下一次后台校验时自愈。
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

/** 持久化键：结构变化时升版本号，旧数据自然作废 */
const LS_KEY = "overview.snapshot.v1";

/** 从 localStorage 恢复快照；结构损坏 / 字段缺失时丢弃，回退全量加载 */
function restoreSnapshot(): OverviewSnapshot | null {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (!raw) return null;
    const p = JSON.parse(raw) as OverviewSnapshot;
    if (
      !p ||
      typeof p.builtAt !== "number" ||
      !p.stats ||
      !Array.isArray(p.persons) ||
      !Array.isArray(p.locations) ||
      !Array.isArray(p.categories)
    ) {
      return null;
    }
    return p;
  } catch {
    return null;
  }
}

export const useOverviewStore = defineStore("overview", () => {
  /** 最近一次构建的快照；null = 从没算过。启动时先恢复上次会话的（FEAT-153） */
  const restored = restoreSnapshot();
  const snapshot = shallowRef<OverviewSnapshot | null>(restored);
  /** 失效标记：true 时下次进入概览会重算（而不是直接读缓存） */
  const dirty = ref(true);
  /** 快照是否已与后端核对过。有恢复快照时为 false —— 首次 load 做一次后台校验 */
  const verified = ref(!restored);
  const building = ref(false);
  /** 构建失败原因（保留上一次可用快照，避免一次失败把整页清空） */
  const error = ref("");

  /** 快照落 localStorage；失败（隐私模式 / 配额满）只影响下次启动速度，不影响本次 */
  function persist(s: OverviewSnapshot) {
    try {
      localStorage.setItem(LS_KEY, JSON.stringify(s));
    } catch {
      /* 忽略：持久化是纯优化 */
    }
  }

  /** 标记失效。导入相册 / 扫描入库 / 删除照片 / 重建索引后调用。
   *  不删快照：失效期间旧数据继续渲染（SWR 的 stale），重算完成后原子替换。 */
  function invalidate() {
    dirty.value = true;
  }

  let inflight: Promise<void> | null = null;

  async function load(force = false): Promise<void> {
    // 无恢复快照时首次 load 即是全量加载，算完即为「已核对」
    if (!force && verified.value && !dirty.value && snapshot.value) return;
    if (inflight) return inflight;

    // 先渲染后校验：有恢复快照时不挡骨架屏（building=false），后台静默重算
    building.value = !snapshot.value;
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
        const next: OverviewSnapshot = { stats, persons, locations, categories, builtAt: Date.now() };
        snapshot.value = next;
        dirty.value = false;
        verified.value = true;
        persist(next);
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
