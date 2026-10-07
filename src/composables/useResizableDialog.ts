import { computed, onBeforeUnmount, ref, watch, type Ref } from "vue";

/**
 * FEAT-096（R3 / R4a）：窗口式可拖边缩放 + 标题栏拖动
 *
 * 需求原话：「这个子图应当支持拖动边界放大缩小，参考 windows 软件框的放大缩小」，
 * 以及「原图预览界面不能全屏 / 不能拖动边界放大缩小」。
 *
 * 之前这些弹窗全是 `width: min(720px, 94vw)` 写死的，完全没有尺寸概念 ——
 * 大屏上小得看不清楚，小屏上又挤不下。现在统一成「Windows 窗口框」语义：
 * - 8 向拖边 / 拖角缩放（n / s / e / w / nw / ne / sw / se）
 * - 标题栏按住拖动可移动窗口
 * - 双击标题栏在「最大化 ↔ 原始尺寸」间切换
 * - 有最小/最大尺寸约束，最大不超过视口（否则窗口能被拖出屏幕找不回来）
 *
 * Q1 决策：同时支持拖标题栏移动。
 * Q4 决策：尺寸在本次会话内记住（组件 v-if 卸载重挂时保留，切走再回来不变），
 *          退出应用不持久化 —— 持久化反而会让用户被上次的窗口大小困住。
 */
export interface ResizableDialogOptions {
  /** 初始宽度（px） */
  initialWidth?: number;
  /** 初始高度（px） */
  initialHeight?: number;
  /** 最小宽度（px） */
  minWidth?: number;
  /** 最小高度（px） */
  minHeight?: number;
  /** 相对视口的最小高度占比（保证标题栏 + 至少一行内容可见） */
  minHeightVh?: number;
  /** 是否允许拖动/缩放 */
  enabled?: Ref<boolean> | boolean;
}

export function useResizableDialog(options: ResizableDialogOptions = {}) {
  const {
    initialWidth = 760,
    initialHeight = 560,
    minWidth = 420,
    minHeight = 260,
    minHeightVh = 30,
  } = options;
  const enabled = options.enabled ?? true;
  const isEnabled = computed(() => (typeof enabled === "boolean" ? enabled : enabled.value));

  /* ---------------- 状态 ---------------- */

  /** 窗口位置（相对视口左上角） */
  const x = ref(0);
  const y = ref(0);
  const width = ref(initialWidth);
  const height = ref(initialHeight);

  /** 全屏标记：全屏时忽略 x/y/width/height，铺满视口 */
  const fullscreen = ref(false);
  /** 全屏前的尺寸，用于退出全屏时还原 */
  const savedBox = ref<{ x: number; y: number; width: number; height: number } | null>(null);

  /** 拖动状态 */
  const dragging = ref(false);
  const activeHandle = ref<string | null>(null);

  /* ---------------- 约束 ---------------- */

  /** 视口最大可用宽高（留 16px 外边距，窗口不贴死屏幕边） */
  const viewportMax = computed(() => ({
    w: Math.max(minWidth, window.innerWidth - 16),
    h: Math.max(minHeight, window.innerHeight - 16),
  }));

  const sizeStyle = computed(() => {
    if (fullscreen.value) {
      return { left: "0px", top: "0px", width: "100vw", height: "100vh" };
    }
    return {
      left: `${Math.round(x.value)}px`,
      top: `${Math.round(y.value)}px`,
      width: `${Math.round(width.value)}px`,
      height: `${Math.round(height.value)}px`,
    };
  });

  const minH = computed(() => Math.max(minHeight, (window.innerHeight * minHeightVh) / 100));

  /* ---------------- 几何计算 ---------------- */

  /** 首次显示时居中（只做一次，后续由用户拖动决定） */
  let centered = false;
  function ensureCentered() {
    if (centered || fullscreen.value) return;
    centered = true;
    const vp = viewportMax.value;
    width.value = Math.min(initialWidth, vp.w);
    height.value = Math.min(initialHeight, vp.h, Math.max(minH.value, vp.h));
    x.value = Math.max(8, Math.round((window.innerWidth - width.value) / 2));
    y.value = Math.max(8, Math.round((window.innerHeight - height.value) / 2));
  }

  /**
   * 从一个把手拖到 (px, py)：算出新的位置与尺寸
   *
   * 八个方向的几何各不相同，但只有两个自由度需要决策：
   * - **是否移动对边**：n / nw / ne 会拖动上边界，对边（bottom）应保持不动；
   * - **是否修正位置**：只动上/左边时，对锚点（原点）的偏移要被抵消，否则
   *   窗口会在拖动中"越拖越跑"（尺寸变了但左上角没反向补偿，视觉上像是错位）。
   */
  function applyDrag(handle: string, px: number, py: number, ox: number, oy: number) {
    const dx = px - ox;
    const dy = py - oy;
    const movesLeft = handle.includes("w");
    const movesRight = handle.includes("e");
    const movesTop = handle.includes("n");
    const movesBottom = handle.includes("s");

    const vp = viewportMax.value;
    let nw = width.value;
    let nh = height.value;
    let nx = x.value;
    let ny = y.value;

    if (movesLeft) {
      // 宽度 = 右边界 - 新左边界；钳到 [minWidth, vp.w]
      const right = x.value + width.value;
      nw = clamp(right - px, minWidth, vp.w);
      nx = right - nw;
    } else if (movesRight) {
      nw = clamp(width.value + dx, minWidth, vp.w);
    }

    if (movesTop) {
      const bottom = y.value + height.value;
      nh = clamp(bottom - py, minH.value, vp.h);
      ny = bottom - nh;
    } else if (movesBottom) {
      nh = clamp(height.value + dy, minH.value, vp.h);
    }

    // 全屏态拖动 = 退出全屏并落到指针位置，避免"松手后窗口跳回原处"
    if (fullscreen.value) {
      fullscreen.value = false;
      savedBox.value = null;
      x.value = clamp(px - width.value / 2, 0, vp.w - width.value);
      y.value = clamp(py - 24, 0, vp.h - height.value);
      return;
    }

    width.value = nw;
    height.value = nh;
    x.value = clamp(nx, 0, vp.w - nw);
    y.value = clamp(ny, 0, vp.h - nh);
  }

  /** 拖动标题栏移动窗口（不改变尺寸） */
  function applyMove(px: number, py: number, ox: number, oy: number) {
    if (fullscreen.value) {
      // 全屏时按住标题栏拖动 = 退出全屏并跟随指针（Windows 行为）
      fullscreen.value = false;
      savedBox.value = null;
      x.value = clamp(px - width.value / 2, 0, viewportMax.value.w - width.value);
      y.value = Math.max(0, py - 24);
      return;
    }
    const vp = viewportMax.value;
    x.value = clamp(x.value + (px - ox), 0, vp.w - width.value);
    y.value = clamp(y.value + (py - oy), 0, vp.h - height.value);
  }

  function clamp(v: number, lo: number, hi: number): number {
    return Math.min(hi, Math.max(lo, v));
  }

  /* ---------------- 事件绑定 ---------------- */

  let startX = 0;
  let startY = 0;

  function onPointerDown(e: PointerEvent, handle: string) {
    if (!isEnabled.value || e.button !== 0) return;
    e.preventDefault();
    // 把手在边框上，pointer capture 挂到把手自身才能持续收到 move
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    dragging.value = true;
    activeHandle.value = handle;
    startX = e.clientX;
    startY = e.clientY;
    ensureCentered();
  }

  function onTitlePointerDown(e: PointerEvent) {
    if (!isEnabled.value || e.button !== 0) return;
    // 标题栏上的按钮（关闭/全屏等）不应触发拖动
    const target = e.target as HTMLElement | null;
    if (target?.closest("button")) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    dragging.value = true;
    activeHandle.value = "move";
    startX = e.clientX;
    startY = e.clientY;
    ensureCentered();
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging.value) return;
    const handle = activeHandle.value;
    if (!handle) return;
    if (handle === "move") applyMove(e.clientX, e.clientY, startX, startY);
    else applyDrag(handle, e.clientX, e.clientY, startX, startY);
  }

  function onPointerUp() {
    dragging.value = false;
    activeHandle.value = null;
  }

  /* ---------------- 全屏 / 还原 ---------------- */

  /** 切换全屏（Q2：进出全屏保持信息面板的收起状态，由调用方的 metaCollapsed 自行维持） */
  function toggleFullscreen() {
    if (fullscreen.value) {
      fullscreen.value = false;
      if (savedBox.value) {
        x.value = savedBox.value.x;
        y.value = savedBox.value.y;
        width.value = savedBox.value.width;
        height.value = savedBox.value.height;
        savedBox.value = null;
      }
    } else {
      savedBox.value = { x: x.value, y: y.value, width: width.value, height: height.value };
      fullscreen.value = true;
    }
  }

  /** 双击标题栏 = 最大化 / 还原（Windows 行为） */
  function onTitleDoubleClick() {
    if (!isEnabled.value) return;
    toggleFullscreen();
  }

  /** 复位到初始尺寸并居中（「还原默认」入口） */
  function reset() {
    fullscreen.value = false;
    savedBox.value = null;
    centered = false;
    ensureCentered();
  }

  /* ---------------- 视口变化 ---------------- */

  /** 视口缩小后把窗口收进可视区，避免窗口被拖到屏幕外找不回来 */
  function onResize() {
    if (fullscreen.value) return;
    const vp = viewportMax.value;
    width.value = Math.min(width.value, vp.w);
    height.value = Math.min(height.value, vp.h);
    x.value = clamp(x.value, 0, Math.max(0, vp.w - width.value));
    y.value = clamp(y.value, 0, Math.max(0, vp.h - height.value));
  }

  watch(isEnabled, (on) => {
    if (on) ensureCentered();
  });

  onBeforeUnmount(() => {
    window.removeEventListener("resize", onResize);
  });
  window.addEventListener("resize", onResize);

  return {
    // 状态
    x,
    y,
    width,
    height,
    fullscreen,
    dragging,
    // 样式
    sizeStyle,
    // 动作
    ensureCentered,
    toggleFullscreen,
    reset,
    // 事件
    handleDown: onPointerDown,
    titleDown: onTitlePointerDown,
    onPointerMove,
    onPointerUp,
    onTitleDoubleClick: onTitleDoubleClick,
  };
}
