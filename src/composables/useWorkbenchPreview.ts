/**
 * 工作台即时预览（FEAT-177）
 * ===================================================================
 * 改一个参数就想看效果，不能等整批跑完。这里把「参数 → 单张预览」封成一个组合式：
 *   - **防抖**：拖滑杆的过程中不发请求，停手 220ms 才发一次（否则一次拖拽几十个请求）；
 *   - **单飞（in-flight=1）**：同一时刻只允许一个预览请求在算。一旦发出就无法中止
 *     （IPC → HTTP → Python 侧一定会算完），所以「只丢结果」不够 —— 拖滑杆时
 *     每个 220ms 间隔都发一次，单次 300~800ms 会让在飞请求持续堆积，把两个作业
 *     worker 的 CPU 抢光。现在改成：在飞期间只记住「最新待发」，回来后再发那一个；
 *   - **缓存**：同一 (图 + 计划 + 参数) 命中缓存直接换图，来回拖动不重复算；
 *   - **只认最后一次**：请求序号对不上就丢弃，避免先发后到把新结果覆盖掉；
 *   - **失败不炸界面**：保留上一帧 + 记错误，让用户知道「预览没算出来」。
 *
 * 限制（写入产品口径，不只是实现细节）：
 *   - 只对**本机算子**的配方 / 步骤链开放；AI 模型模式逐张走云端计费，不做实时预览；
 *   - 预览长边固定 1280（够看清晰度，又快）；全量出片仍按原图尺寸。
 */
import { onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { trace } from "../utils/trace";
import { getLru, previewCacheKey, putLru, PREVIEW_CACHE_LIMIT } from "../utils/previewCache";
import type { RunPlan } from "../types/ai";

export interface PreviewRequest {
  /** 原图绝对路径 */
  path: string;
  /** 配方引擎：配方 id + 覆盖参数（值域与 op_catalog 一致，含布尔/枚举） */
  recipeId?: string;
  paramsOverride?: Record<string, Record<string, number | string | boolean>>;
  /** 自己组合：显式计划（优先于 recipeId） */
  plan?: RunPlan;
  /** 预览长边，默认 1280 */
  maxEdge?: number;
}

export interface PreviewResult {
  /** data:image/jpeg;base64,... */
  image: string;
  width: number;
  height: number;
  ms: number;
  /** 被摘掉的算子（云端步 / 模型未就绪） */
  dropped: string[];
  warnings: string[];
}

const PREVIEW_MAX_EDGE = 1280;
const DEBOUNCE_MS = 220;
/**
 * 缓存条数上限见 `utils/previewCache.PREVIEW_CACHE_LIMIT`（8 条）。
 * 每条是 1280px 的 JPEG data URL（约 340~600KB 字符串），24 条意味着 8~14MB
 * 常驻字符串、且在 JS 堆上是不可回收的大串（滑动窗口 GC 会卡）。
 */
const CACHE_LIMIT = PREVIEW_CACHE_LIMIT;

/** AOP 包装的预览请求（只包 invoke 本身：缓存命中不产生日志）。 */
const fetchPreview = trace(
  "工坊.即时预览",
  (req: PreviewRequest): Promise<PreviewResult> =>
    invoke<PreviewResult>("ai_preview", {
      path: req.path,
      recipeId: req.recipeId ?? null,
      paramsOverride: req.paramsOverride ?? null,
      plan: req.plan ?? null,
      maxEdge: req.maxEdge ?? PREVIEW_MAX_EDGE,
    }),
);

function cacheKey(req: PreviewRequest): string {
  return previewCacheKey({
    path: req.path,
    recipeId: req.recipeId,
    maxEdge: req.maxEdge ?? PREVIEW_MAX_EDGE,
    plan: req.plan,
    paramsOverride: req.paramsOverride,
  });
}

export function useWorkbenchPreview() {
  /** 最近一次成功的预览图（data URL）；null = 还没有 */
  const src = ref<string | null>(null);
  const previewing = ref(false);
  const lastMs = ref(0);
  const dropped = ref<string[]>([]);
  const error = ref("");

  const cache = new Map<string, string>();
  let seq = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  /** 当前在飞的请求数（恒 ≤1）；单飞是防止拖滑杆时 Python 端堆积的关键 */
  let inflight = false;
  /** 在飞期间最多记住「最新一条」待发请求，中间的全部丢弃 */
  let pending: PreviewRequest | null = null;

  function putCache(key: string, value: string): void {
    putLru(cache, key, value, CACHE_LIMIT);
  }

  function readCache(key: string): string | undefined {
    return getLru(cache, key);
  }

  /** 真正发请求的入口（已处理单飞与序号丢弃） */
  async function dispatch(req: PreviewRequest): Promise<void> {
    const key = cacheKey(req);
    const hit = readCache(key);
    if (hit) {
      src.value = hit;
      previewing.value = false;
      error.value = "";
      return;
    }
    inflight = true;
    const mine = ++seq;
    previewing.value = true;
    error.value = "";
    try {
      const res = await fetchPreview(req);
      if (mine !== seq) return; // 已经有更新的请求，丢弃这次的旧结果
      src.value = res.image;
      lastMs.value = res.ms;
      dropped.value = res.dropped ?? [];
      putCache(key, res.image);
    } catch (e) {
      if (mine !== seq) return;
      error.value = String(e);
    } finally {
      if (mine === seq) previewing.value = false;
      inflight = false;
      drain();
    }
  }

  /** 在飞结束后补发「最新待发」那一条（单飞队列的出队点） */
  function drain(): void {
    const next = pending;
    pending = null;
    if (next == null) return;
    void dispatch(next);
  }

  /** 立即请求一次（不走防抖），供「切换照片」这种明确动作使用 */
  async function run(req: PreviewRequest): Promise<void> {
    const key = cacheKey(req);
    const hit = readCache(key);
    if (hit) {
      src.value = hit;
      previewing.value = false;
      error.value = "";
      return;
    }
    if (inflight) {
      // 在飞：只保留最新一条，不排队、不叠加
      pending = req;
      return;
    }
    await dispatch(req);
  }

  /** 防抖请求：参数变更时调它 */
  function schedule(req: PreviewRequest): void {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      void run(req);
    }, DEBOUNCE_MS);
  }

  /** 清空当前预览（切方案 / 换图时用，避免拿旧图冒充新图） */
  function reset(): void {
    if (timer) clearTimeout(timer);
    timer = null;
    pending = null;
    seq += 1;
    src.value = null;
    previewing.value = false;
    dropped.value = [];
    error.value = "";
  }

  // 页面卸载后防抖定时器仍会到点发请求（220ms 窗口），还会把 data URL 缓存留在
  // 闭包里；这里统一清掉，避免「离开工作台还在算预览」。
  onUnmounted(() => {
    reset();
    cache.clear();
  });

  return { src, previewing, lastMs, dropped, error, schedule, run, reset };
}

