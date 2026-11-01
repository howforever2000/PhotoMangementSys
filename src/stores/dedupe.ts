import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  DedupeDeleteMode,
  DedupeDeleteOutcome,
  DedupeProgress,
  DedupeReport,
  DupGroup,
  HeicSupport,
} from "../types/dedupe";
import { useAlbumStore } from "./album";
import { deletablePathsOf, folderCells, reclaimableOf, sanitizeReport } from "../utils/dedupeGroups";
import { trace } from "../utils/trace";

/**
 * 重复照片清理（FEAT-143）状态
 *
 * 放在 Pinia 而不是页面里：全库扫描是**秒级到分钟级**的批处理，
 * 用户切走标签页/离开 Hub 时不该丢掉结果与勾选状态。
 *
 * 与后端职责划分：
 *   - 后端 `dedupe_scan` 只负责「算出分组」（无状态），进度走 `dedupe-progress` 事件
 *   - store 负责「保留哪张、勾了哪些、删哪一档」这些**纯前端决策**
 *
 * 勾选语义：`selected` 里放的是**待删除**的照片路径。
 * 扫描完成时的默认状态 = 每组除「保留」外全部勾上（用户要的是"默认留一张"，
 * 剩下的本来就是要清掉的），再允许逐张取消。
 */

/** 相似阈值默认值（用户确认：默认 0.98，滑杆范围仍为 0.80–1.00） */
export const DEFAULT_THRESHOLD = 0.98;

/** 旧版本的默认值：用户没动过滑杆时，localStorage 里存的就是它，读到就迁到新默认 */
const LEGACY_DEFAULT_THRESHOLD = 0.9;

/** 分组分页：一次渲染多少组（翻页才换，页内缩略图再按卡片进视口才拉） */
export const GROUPS_PER_PAGE = 12;

/** 阈值所在 localStorage 键（不换键名，避免把用户自己调过的值丢掉） */
const THRESHOLD_KEY = "pms.dedupe.threshold";

/** 阈值语义提示（跟着滑杆走，讲清调高调低的后果） */
export function thresholdHint(th: number): string {
  if (th >= 0.995) return "几乎只认同一张画面：轻微裁切 / 重压缩的副本都可能漏过";
  if (th >= 0.98) return "默认（推荐）：同一张照片的不同尺寸 / 不同压缩才会命中，几乎不会误并";
  if (th >= 0.95) return "很严格：同图不同格式 / 尺寸能命中，同场景连拍基本不会进同一组";
  if (th >= 0.9) return "偏宽松：同场景连拍可能被并入同组，删除前请逐组确认";
  if (th >= 0.85) return "宽松：相似构图会被并入同组，只适合人工逐组筛选";
  return "很宽松：会把同题材照片并组，仅适合逐组人工确认";
}

export const useDedupeStore = defineStore("dedupe", () => {
  /* ---------------- 参数（跨会话记住：用户调过阈值就不用每次再调） ---------------- */
  const threshold = ref(loadThreshold());
  const includeExact = ref(loadBool("pms.dedupe.exact", true));
  const includeSimilar = ref(loadBool("pms.dedupe.similar", true));

  /* ---------------- 运行态 ---------------- */
  const running = ref(false);
  const stopping = ref(false);
  const stage = ref<string>("");
  const progressDone = ref(0);
  const progressTotal = ref(0);
  const progressText = ref("");
  const error = ref("");
  /** 后端算出的原始分组（删除后会被就地裁剪） */
  const report = ref<DedupeReport | null>(null);
  /** 用户改选的「保留」：groupKey → path */
  const keepOverride = ref<Record<string, string>>({});
  /** 待删除路径 */
  const selected = ref<Set<string>>(new Set());
  /** 删除中（防重复点击） */
  const deleting = ref(false);
  /** 上一轮删除的失败明细（要如实呈现，不能静默） */
  const lastFailures = ref<string[]>([]);
  /** HEIC 解码能力（缺 HEIF 扩展时顶部提示） */
  const heic = ref<HeicSupport | null>(null);
  /** 分组分页（一页 12 组；页内缩略图再按视口热加载） */
  const page = ref(0);

  let unlisten: UnlistenFn | null = null;

  /* ---------------- 派生 ---------------- */

  const groups = computed<DupGroup[]>(() => report.value?.groups ?? []);

  const pageCount = computed(() => Math.max(1, Math.ceil(groups.value.length / GROUPS_PER_PAGE)));

  const pageGroups = computed<DupGroup[]>(() =>
    groups.value.slice(page.value * GROUPS_PER_PAGE, (page.value + 1) * GROUPS_PER_PAGE),
  );

  /** 某组当前保留的那张 */
  function keepOf(group: DupGroup): string {
    return keepOverride.value[group.key] || group.keep_path;
  }

  /**
   * 某组「可删」的路径
   *
   * 硬规则（见 utils/dedupeGroups.deletablePathsOf）：保留那张永远不在里面，
   * 保留所在文件夹里的其它副本也不在里面 —— 勾了也删不到想留的那张。
   */
  function deletableOf(group: DupGroup): string[] {
    return deletablePathsOf(group, keepOf(group));
  }

  /** 某组的文件夹卡片（渲染单位：一个文件夹一张卡） */
  function cellsOf(group: DupGroup) {
    return folderCells(group, keepOf(group));
  }

  const selectedCount = computed(() => selected.value.size);

  /** 已勾选照片的总体积（用户最关心的"能省多少空间"） */
  const selectedBytes = computed(() => {
    let sum = 0;
    for (const g of groups.value) {
      for (const it of g.items) {
        if (selected.value.has(it.path)) sum += it.size_bytes;
      }
    }
    return sum;
  });

  /** 全部可删照片的总体积 */
  const reclaimableBytes = computed(() =>
    groups.value.reduce((n, g) => n + reclaimableOf(g, keepOf(g)), 0),
  );

  /** 是否有"未勾选任何可删项"的情况（底部按钮据此禁用） */
  const canDelete = computed(() => !deleting.value && selectedCount.value > 0);

  /* ---------------- actions ---------------- */

  threshold.value = clampThreshold(threshold.value);

  function clampThreshold(v: number): number {
    if (!Number.isFinite(v)) return DEFAULT_THRESHOLD;
    return Math.min(1, Math.max(0.8, Math.round(v * 100) / 100));
  }

  function setThreshold(v: number) {
    threshold.value = clampThreshold(v);
    localStorage.setItem(THRESHOLD_KEY, String(threshold.value));
  }

  function setIncludeExact(v: boolean) {
    includeExact.value = v;
    localStorage.setItem("pms.dedupe.exact", v ? "1" : "0");
  }

  function setIncludeSimilar(v: boolean) {
    includeSimilar.value = v;
    localStorage.setItem("pms.dedupe.similar", v ? "1" : "0");
  }

  async function refreshHeicSupport() {
    try {
      heic.value = await invoke<HeicSupport>("heic_support");
    } catch {
      heic.value = null;
    }
  }

  /** 开扫：清空上一轮结果与勾选，注册事件监听 */
  async function scan() {
    if (running.value) return;
    if (!includeExact.value && !includeSimilar.value) {
      error.value = "请至少勾选一种判据（完全重复 / 画面相似）";
      return;
    }
    error.value = "";
    lastFailures.value = [];
    report.value = null;
    keepOverride.value = {};
    selected.value = new Set();
    running.value = true;
    stopping.value = false;
    stage.value = "collect";
    progressDone.value = 0;
    progressTotal.value = 0;
    progressText.value = "正在准备…";

    await ensureListener();
    try {
      const r = await invoke<DedupeReport>("dedupe_scan", {
        threshold: threshold.value,
        includeExact: includeExact.value,
        includeSimilar: includeSimilar.value,
      });
      // 先清洗再入 store：组内同路径只留一张 + 掉掉没有可删副本的组
      const cleaned = sanitizeReport(r);
      report.value = cleaned;
      heic.value = cleaned.heic;
      // 默认勾选：除保留那张与保留文件夹里的其它副本外全部选中
      const next = new Set<string>();
      for (const g of cleaned.groups) {
        for (const p of deletablePathsOf(g, g.keep_path)) next.add(p);
      }
      selected.value = next;
    } catch (e) {
      const msg = String(e);
      // 用户主动停止不算错误（后端回 "已停止"）
      if (msg.includes("已停止")) {
        error.value = "";
      } else {
        error.value = msg;
      }
    } finally {
      running.value = false;
      stopping.value = false;
      detachListener();
    }
  }

  /** 停止当前扫描（后端在下一个检查点收手） */
  async function stop() {
    if (!running.value || stopping.value) return;
    stopping.value = true;
    progressText.value = "正在停止…";
    try {
      await invoke("dedupe_cancel");
    } catch (e) {
      error.value = String(e);
    }
  }

  async function ensureListener() {
    if (unlisten) return;
    // 进度台账（FEAT-153）：阶段切换必记，阶段内按 10% 抽记
    // —— 逐条 console.log 会在 2.5 万张的库上把控制台刷到卡。
    let stamp = { stage: "", bucket: -1 };
    unlisten = await listen<DedupeProgress>("dedupe-progress", (ev) => {
      const p = ev.payload;
      stage.value = p.stage;
      progressDone.value = p.done;
      progressTotal.value = p.total;
      progressText.value = p.message;
      const bucket = p.total > 0 ? Math.floor((p.done / p.total) * 10) : 0;
      if (p.stage !== stamp.stage) {
        stamp = { stage: p.stage, bucket };
        console.log(`[dedupe] 阶段 → ${p.stage} | ${p.done}/${p.total} | ${p.message}`);
      } else if (bucket !== stamp.bucket) {
        stamp = { stage: p.stage, bucket };
        console.log(`[dedupe] ${p.stage} ${p.done}/${p.total} · ${p.message}`);
      }
    });
  }

  function detachListener() {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
  }

  /* ---------------- 勾选操作 ---------------- */

  function isSelected(path: string): boolean {
    return selected.value.has(path);
  }

  function toggle(path: string) {
    const next = new Set(selected.value);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    selected.value = next;
  }

  /** 全选可删（保留那张与保留文件夹里的副本永远不可删） */
  function selectAll() {
    const next = new Set<string>();
    for (const g of groups.value) {
      for (const p of deletableOf(g)) next.add(p);
    }
    selected.value = next;
  }

  function clearSelection() {
    selected.value = new Set();
  }

  /** 组内全选 / 全不选 */
  function selectGroup(group: DupGroup, on: boolean) {
    const next = new Set(selected.value);
    for (const p of deletableOf(group)) {
      if (on) next.add(p);
      else next.delete(p);
    }
    selected.value = next;
  }

  /**
   * 整张文件夹卡片勾选 / 取消
   *
   * 只动该文件夹里**允许删**的路径：保留文件夹的卡片根本不给勾，这里再卡一道，
   * 即使 UI 传了 isKeepFolder 的 dir 也不会误删想留的那张。
   */
  function selectCell(group: DupGroup, dir: string, on: boolean) {
    const key = dir.toLowerCase();
    const allowed = new Set(deletableOf(group));
    const next = new Set(selected.value);
    for (const it of group.items) {
      if (it.dir.toLowerCase() !== key || !allowed.has(it.path)) continue;
      if (on) next.add(it.path);
      else next.delete(it.path);
    }
    selected.value = next;
  }

  function setPage(p: number) {
    page.value = Math.min(pageCount.value - 1, Math.max(0, p));
  }

  /**
   * 改选「保留」
   *
   * 换保留会把"可删集合"整体重算：旧保留所在文件夹变成普通文件夹、新保留所在文件夹变成保护文件夹，
   * 所以直接按最新 kept 重建 —— 逐条加/删容易漏掉跨文件夹的那一批。
   */
  function setKeep(group: DupGroup, path: string) {
    if (keepOf(group) === path) return;
    keepOverride.value = { ...keepOverride.value, [group.key]: path };
    const keep = path;
    const deletable = new Set(deletablePathsOf(group, keep));
    // 已在别组勾选的项保持不动，只重算本组
    const next = new Set(selected.value);
    for (const it of group.items) next.delete(it.path);
    for (const p of deletable) next.add(p);
    selected.value = next;
  }

  /* ---------------- 删除 ---------------- */

  /**
   * 删除已勾选照片
   *
   * - `records`：只删记录的展示资格（文件留在磁盘，可从「最近删除」恢复）
   * - `trash`：把文件移入系统回收站（可从回收站恢复），同时级联清扫描记录与缩略图
   *
   * 删除成功后从内存里的分组中裁掉已删项，并重新计算可释放体积；
   * 组内只剩 1 张时整组消失。
   */
  async function deleteSelected(mode: DedupeDeleteMode): Promise<DedupeDeleteOutcome | null> {
    if (!canDelete.value) return null;
    deleting.value = true;
    error.value = "";
    lastFailures.value = [];
    const paths = [...selected.value];
    try {
      let outcome: DedupeDeleteOutcome;
      if (mode === "trash") {
        outcome = await invoke<DedupeDeleteOutcome>("delete_photos_to_trash", { paths });
      } else {
        // 「仅删记录」按相册分组调用：只有带 album 的删除才会把照片移出网格视图
        const byAlbum = new Map<number, string[]>();
        for (const g of groups.value) {
          for (const it of g.items) {
            if (!selected.value.has(it.path)) continue;
            const list = byAlbum.get(it.album_id) ?? [];
            list.push(it.path);
            byAlbum.set(it.album_id, list);
          }
        }
        outcome = { requested: paths.length, deleted: 0, failed: 0, failed_paths: [] };
        for (const [albumId, list] of byAlbum) {
          const r = await invoke<DedupeDeleteOutcome>("delete_photo_records", {
            albumId,
            paths: list,
          });
          outcome.deleted += r.deleted;
          outcome.failed += r.failed;
          outcome.failed_paths.push(...r.failed_paths);
        }
      }
      lastFailures.value = outcome.failed_paths.slice(0, 50);

      // 只裁掉**成功**的那些（失败项保留在结果里，用户可以重试）
      const failed = new Set(outcome.failed_paths);
      const removed = new Set(paths.filter((p) => !failed.has(p)));
      applyRemoved(removed);
      selected.value = new Set(paths.filter((p) => failed.has(p)));

      // 相册张数变了 → 刷新主页/相册墙读数
      try {
        await useAlbumStore().fetchAlbums();
      } catch {
        /* 读数刷新失败不影响删除结果 */
      }
      return outcome;
    } catch (e) {
      error.value = String(e);
      return null;
    } finally {
      deleting.value = false;
    }
  }

  /** 把已删除的路径从分组里裁掉，并收拾空组 / 重新计算体积 */
  function applyRemoved(removed: Set<string>) {
    const r = report.value;
    if (!r) return;
    const nextGroups: DupGroup[] = [];
    for (const g of r.groups) {
      const items = g.items.filter((it) => !removed.has(it.path));
      if (items.length < 2) continue;
      const keep = items.some((it) => it.path === keepOf(g)) ? keepOf(g) : items[0].path;
      const next: DupGroup = { ...g, keep_path: keep, items, reclaimable_bytes: 0 };
      // 删完之后已经没有任何可删副本（剩下的都在保留文件夹里）→ 整组退场
      if (deletablePathsOf(next, keep).length === 0) continue;
      next.reclaimable_bytes = reclaimableOf(next, keep);
      nextGroups.push(next);
    }
    report.value = { ...r, groups: nextGroups };
    if (page.value > 0 && page.value >= pageCount.value) page.value = pageCount.value - 1;
    console.log(
      `[dedupe] 结果裁剪：移除 ${removed.size} 张 · 剩余 ${nextGroups.length} 组（原 ${r.groups.length} 组）`,
    );
  }

  /** 清空结果（重新扫描前由 UI 显式调用，避免误以为还有未处理组） */
  function reset() {
    report.value = null;
    keepOverride.value = {};
    selected.value = new Set();
    error.value = "";
    lastFailures.value = [];
    page.value = 0;
  }

  return {
    // 参数
    threshold,
    includeExact,
    includeSimilar,
    setThreshold,
    setIncludeExact,
    setIncludeSimilar,
    // 运行态
    running,
    stopping,
    stage,
    progressDone,
    progressTotal,
    progressText,
    error,
    report,
    heic,
    deleting,
    lastFailures,
    selected,
    groups,
    page,
    pageCount,
    pageGroups,
    // 派生
    keepOf,
    deletableOf,
    cellsOf,
    selectedCount,
    selectedBytes,
    reclaimableBytes,
    canDelete,
    isSelected,
    // actions
    refreshHeicSupport,
    // AOP 出入口日志（FEAT-153）：与后端 [dedupe] 台账同一条排障链路
    scan: trace("dedupe.scan", scan),
    stop: trace("dedupe.stop", stop),
    toggle,
    selectAll,
    clearSelection,
    selectGroup,
    selectCell,
    setKeep,
    setPage,
    deleteSelected: trace("dedupe.deleteSelected", deleteSelected),
    reset,
  };
});

function loadThreshold(): number {
  const raw = localStorage.getItem(THRESHOLD_KEY);
  const n = raw == null ? NaN : Number(raw);
  if (!Number.isFinite(n)) return DEFAULT_THRESHOLD;
  // 旧默认值（用户没动过滑杆）→ 迁到新默认；用户自己调过的其它值保留
  if (Math.abs(n - LEGACY_DEFAULT_THRESHOLD) < 1e-6) {
    localStorage.setItem(THRESHOLD_KEY, String(DEFAULT_THRESHOLD));
    return DEFAULT_THRESHOLD;
  }
  return n;
}

function loadBool(key: string, fallback: boolean): boolean {
  const raw = localStorage.getItem(key);
  if (raw == null) return fallback;
  return raw === "1" || raw === "true";
}
