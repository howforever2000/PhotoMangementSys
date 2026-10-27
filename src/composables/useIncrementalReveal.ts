/**
 * useIncrementalReveal —— 按"屏幕"增量渲染长列表（FEAT-117）
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 要解决的问题
 * ───────────────────────────────────────────────────────────────────────────
 * 时间线的缩略图是懒的（IntersectionObserver 才拉图），但**DOM 节点是急的**：
 * `v-for` 一次把 2.5 万张照片全部建成 <figure>，一屏之外还压着几万个节点。
 * 浏览器要排版它们、Vue 要为它们建响应式依赖、每张卡片还要挂一个 observer ——
 * 三重开销叠加，进页面就是几秒白屏。
 *
 * 懒加载图片 ≠ 懒加载列表。这一层补的就是后者。
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 为什么按"屏幕数"而不是"条数"
 * ───────────────────────────────────────────────────────────────────────────
 * "每次 100 条"这种阈值的问题：100 条在一屏 6 张的宽屏上是 16 屏，
 * 在一屏 30 张的窄屏上只有 3 屏 —— 同一个数字在两种屏幕上体验完全不同。
 * 用户要的是"大概几屏的量"，那就直接按像素算：
 *
 *   预算 = 视口高度 × screens          （默认 5 屏）
 *   不断累加单元直到 Σcost(unit) ≥ 预算
 *
 * 于是无论屏幕多大、行高多少，首屏渲染出来的**视觉长度**都是 5 屏，
 * 滚到底再追加 stepScreens 屏。单元可以是月份、年份、任意块 ——
 * 只要调用方给得出"这个单元大概多高"。
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 用法
 * ───────────────────────────────────────────────────────────────────────────
 *   const { visible, hasMore, loadMore, reset } = useIncrementalReveal({
 *     units,
 *     costOf: (m) => m.height,
 *   });
 *   // 模板里 v-for="u in visible"
 *   // 底部放一个 sentinel，滚到它附近自动 loadMore
 *
 * 注意：单元列表变化（切筛选、数据到达）时**必须**调用 reset()，
 * 否则已渲染的预算会持续累积，最后又变回一次全渲染。
 */
import {
  computed,
  onBeforeUnmount,
  onMounted,
  ref,
  watch,
  type Ref,
} from "vue";

export interface IncrementalRevealOptions<U> {
  /** 全部单元（保持原有顺序；顺序即渲染顺序） */
  units: Ref<U[]> | { value: U[] };
  /** 单个单元的估算像素高度（用于把"屏幕"换算成"单元个数"） */
  costOf: (unit: U, index: number) => number;
  /** 首屏预渲染多少个屏幕的高度，默认 5 */
  screens?: number;
  /** 每次追加多少屏，默认 2（越小越省，但滚到底会频繁触发） */
  stepScreens?: number;
  /** 距文档底部多少像素开始追加，默认 900 */
  thresholdPx?: number;
  /** 为 false 时不接管滚动（由调用方自己调 loadMore），默认 true */
  auto?: boolean;
}

export function useIncrementalReveal<U>(opts: IncrementalRevealOptions<U>) {
  const screens = opts.screens ?? 5;
  const stepScreens = opts.stepScreens ?? 2;
  const thresholdPx = opts.thresholdPx ?? 900;
  const auto = opts.auto ?? true;

  /** 已批准的像素预算（随滚动增长） */
  const budget = ref(0);
  let lastViewport = 0;

  function viewportH(): number {
    if (typeof window === "undefined") return 800;
    return window.innerHeight || 800;
  }

  /**
   * 把预算换算成"要渲染的单元数"。
   *
   * 关键细节：预算可能落在某个单元**内部**（比如第 7 个月只渲一半），
   * 这里一律"整单元纳入" —— 月份是从中间切开就很难看的最小语义单位。
   * 因此实际渲染高度会略大于预算，这是有意的（宁可多渲一个月，不要半截）。
   */
  const visibleCount = computed(() => {
    const list = opts.units.value;
    const target = budget.value;
    let sum = 0;
    for (let i = 0; i < list.length; i++) {
      if (sum >= target) return i;
      sum += Math.max(1, opts.costOf(list[i], i));
    }
    return list.length;
  });

  const visible = computed(() => opts.units.value.slice(0, visibleCount.value));
  const hasMore = computed(() => visibleCount.value < opts.units.value.length);

  function reset() {
    budget.value = viewportH() * screens;
    lastViewport = viewportH();
  }

  function loadMore() {
    if (!hasMore.value) return;
    budget.value += viewportH() * stepScreens;
  }

  /** 视口高度变化超过 15% 才重算（移动端地址栏收放会频繁触发 resize） */
  function onResize() {
    const v = viewportH();
    if (Math.abs(v - lastViewport) / lastViewport < 0.15) return;
    lastViewport = v;
    // 保留"已渲染到哪"而不是粗暴重置：用户正在看第 12 个月，不能把他弹回顶部
    const renderedUnits = visibleCount.value;
    budget.value = 0;
    const list = opts.units.value;
    let sum = 0;
    for (let i = 0; i < renderedUnits && i < list.length; i++) {
      sum += Math.max(1, opts.costOf(list[i], i));
    }
    budget.value = Math.max(sum, v * screens);
  }

  function onScroll() {
    if (!hasMore.value) return;
    const doc = document.documentElement;
    const rest = doc.scrollHeight - window.scrollY - window.innerHeight;
    if (rest <= thresholdPx) loadMore();
  }

  onMounted(() => {
    reset();
    if (!auto) return;
    window.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onResize);
  });
  onBeforeUnmount(() => {
    window.removeEventListener("scroll", onScroll);
    window.removeEventListener("resize", onResize);
  });

  /* 单元数量增加（数据到达 / 追加加载）时，若当前已渲染到底部就自动多给一屏，
     避免"数据来了但界面还是空的"这种卡住的感觉。 */
  watch(
    () => opts.units.value.length,
    (n, old) => {
      /* 数据后到（首次进页面时 rows 为空，随后才填上）：从零到有必须重算预算，
         否则 budget 是按"空列表"算的，界面停在 0 个月不动。 */
      if (old === 0 && n > 0) reset();
    },
  );

  return { visible, visibleCount, hasMore, loadMore, reset };
}
