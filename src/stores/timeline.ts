/**
 * 时间线数据缓存（FEAT-122）—— 模块级、跨页面复用
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 为什么要一个模块级缓存
 * ───────────────────────────────────────────────────────────────────────────
 * 改前每次进时间线都要 `list_timeline` 拉 15.09 MB、再在主线程对 2.5 万条做
 * 四层派生；在智慧相册的各 tab 之间来回点一次就重来一遍（BUG-2026-1027-003）。
 *
 * 现在：
 * - **骨架**（`timeline_index`，约 3 KB）整个会话留一份，页面切换不再重查；
 * - **明细**按月分页拉取，拉过的月份留在 `Map` 里，滚回去不再请求；
 * - **缩略图路径** 随明细下发，进 `thumbs` 后页面切换也不用重新问后端。
 *
 * 失效：后台扫描入库会让计数变化，由 `invalidate()` 显式清空
 * （识别到 `dirty` 时由调用方触发；本页只在挂载时校验一次）。
 */

import { reactive, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

import type { TimelineCard, TimelineIndex } from "../types/timeline";
import { getThumbs } from "../utils/thumbQueue";

/** 单次最多拉多少张（最大月 952 张，未记录 4,840 张 → 分页） */
const PAGE = 300;

interface MonthState {
  cards: TimelineCard[];
  inflight: Promise<void> | null;
  done: boolean;
}

/** 骨架缓存 */
let indexCache: TimelineIndex | null = null;
let indexInflight: Promise<TimelineIndex> | null = null;

/** 月明细缓存：`year|month` → 状态 */
const months = new Map<string, MonthState>();

/** path → 缩略图缓存路径（响应式：填充后卡片自己会显示出图） */
const thumbs = reactive(new Map<string, string>());

/** 已经挂到 get_photo_thumbs 兜底队列里的 path（防重复入队） */
const thumbPending = new Set<string>();

/**
 * 数据版本号。
 *
 * 这里刻意用「普通 Map + 版本号」而不是 `reactive(数组)`：
 * 2.5 万条卡片如果都包成 Proxy，光是代理开销就够把主线程拖住 ——
 * 改前 `monthRowMap` 里对 reactive 对象做 spread 正是这个坑。
 * 数据只在本模块里按页 append，页面只需要知道「有新数据了」，一个数字就够。
 */
export const version = ref(0);

function bump() {
  version.value += 1;
}

/** 月份缓存键 */
export function timelineKey(year: string, month: string): string {
  return `${year}|${month}`;
}

/** 当前缓存的骨架（未加载时为 null） */
export function cachedIndex(): TimelineIndex | null {
  return indexCache;
}

/** 清空全部缓存（入库/扫描后计数变化时调用） */
export function invalidate(): void {
  indexCache = null;
  indexInflight = null;
  months.clear();
  thumbs.clear();
  thumbPending.clear();
  bump();
}

/** 拉骨架（同一时刻只发一次请求） */
export async function loadIndex(force = false): Promise<TimelineIndex> {
  if (!force && indexCache) return indexCache;
  if (!force && indexInflight) return indexInflight;
  indexInflight = invoke<TimelineIndex>("timeline_index")
    .then((idx) => {
      indexCache = idx;
      bump();
      return idx;
    })
    .finally(() => {
      indexInflight = null;
    });
  return indexInflight;
}

/** 某月已缓存的卡片（顺序 = 后端 `ORDER BY p.id`） */
export function cardsOf(year: string, month: string): TimelineCard[] {
  return months.get(timelineKey(year, month))?.cards ?? [];
}

/** 缩略图路径（未就绪返回 undefined） */
export function thumbOf(path: string): string | undefined {
  return thumbs.get(path);
}

/**
 * 确保某月至少有 `need` 张已加载（按 PAGE 分页、去重、并发合并）
 *
 * @param total 该月总张数（来自骨架；用于判断是否已经拉完）
 * @param need  本次要渲染到的下标上限 + 1
 */
export async function ensureCards(
  year: string,
  month: string,
  total: number,
  need: number,
): Promise<void> {
  const key = timelineKey(year, month);
  let st = months.get(key);
  if (!st) {
    st = { cards: [], inflight: null, done: false };
    months.set(key, st);
  }
  const want = Math.min(Math.max(need, 0), total);
  if (want <= 0 || st.done || st.cards.length >= want) {
    // 已够用：但把已拿到的缩略图补进响应式表（首帧就能出图）
    if (st.cards.length) adoptThumbs(st.cards);
    return;
  }
  if (st.inflight) {
    await st.inflight;
    if (!st.done && st.cards.length < want) return ensureCards(year, month, total, need);
    return;
  }

  st.inflight = (async () => {
    const state = st as MonthState;
    try {
      const offset = state.cards.length;
      const remaining = total - offset;
      // limit=0 → 后端按「不限」处理（剩余量小于一页时一次拿完，少一次往返）
      const limit = remaining > PAGE ? PAGE : 0;
      const page = await invoke<TimelineCard[]>("timeline_month", {
        year,
        month,
        limit,
        offset,
      });
      const seen = new Set(state.cards.map((c) => c.path));
      let added = 0;
      for (const card of page) {
        if (seen.has(card.path)) continue;
        seen.add(card.path);
        state.cards.push(card);
        added += 1;
      }
      // 拉不满一页 / 已经凑够总数 → 这月拉完了
      if (page.length < (limit === 0 ? remaining : limit) || state.cards.length >= total) {
        state.done = true;
      }
      adoptThumbs(state.cards);
      if (added > 0) bump();
    } finally {
      state.inflight = null;
    }
  })();
  return st.inflight;
}

/** 把卡片自带的 thumb_path 收进响应式表（后端随行下发的，0 次额外请求） */
function adoptThumbs(cards: TimelineCard[]): void {
  for (const c of cards) {
    if (c.thumb_path && thumbs.get(c.path) !== c.thumb_path) {
      thumbs.set(c.path, c.thumb_path);
    }
  }
}

/**
 * 兜底补齐缩略图（只对后端没给 `thumb_path` 的少数照片：实测 25,000 张里 67 张）
 *
 * 改前这里承担了**全部**缩略图请求（每张卡一次 invoke，实测每次只带 1~5 条路径）；
 * 现在它是例外路径，正常浏览时一次都不会触发。
 */
export function ensureThumbs(cards: TimelineCard[]): void {
  const byAlbum = new Map<number, string[]>();
  for (const c of cards) {
    if (thumbs.has(c.path) || thumbPending.has(c.path)) continue;
    if (c.thumb_path) {
      thumbs.set(c.path, c.thumb_path);
      continue;
    }
    if (c.album_id == null) continue;
    thumbPending.add(c.path);
    const list = byAlbum.get(c.album_id) ?? [];
    list.push(c.path);
    byAlbum.set(c.album_id, list);
  }
  for (const [albumId, paths] of byAlbum) {
    void getThumbs(albumId, paths)
      .then((pairs) => {
        for (const [path, thumb] of pairs) {
          if (thumb) thumbs.set(path, thumb);
        }
      })
      .catch(() => {
        /* 拿不到就保持占位；滚出再滚回会重新入队（与改前语义一致） */
      })
      .finally(() => {
        for (const p of paths) thumbPending.delete(p);
      });
  }
}
