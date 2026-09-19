import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";

/** 偏好设置与背景图分开存储：
 *  - 背景图 data URL 可能几百 KB，若和偏好一起写，超出 localStorage 配额时会导致
 *    整个主题保存失败（表现为"下次登录设置就丢了"）。分开存，图片写失败也不影响偏好。 */
const KEY_PREFS = "pm-theme";
const KEY_IMAGE = "pm-theme-image";

export type ThemeMode = "light" | "dark";
export type BackgroundStyle = "image" | "gradient" | "color";

interface Prefs {
  mode: ThemeMode;
  bgStyle: BackgroundStyle;
  bgColor: string;
  gradFrom: string;
  gradTo: string;
  gradAngle: number;
  bgOpacity: number;
}

const DEFAULTS: Prefs = {
  mode: "light",
  bgStyle: "color",
  bgColor: "#f5f6f8",
  gradFrom: "#396cd8",
  gradTo: "#8a3ffc",
  gradAngle: 135,
  bgOpacity: 0.45,
};

/** 深色模式对应的默认纯色背景 */
const DARK_BG = "#1c202b";

function loadPrefs(): Prefs {
  try {
    const raw = localStorage.getItem(KEY_PREFS);
    if (!raw) return { ...DEFAULTS };
    const parsed = JSON.parse(raw);
    // 旧版本把背景图 data URL 存在偏好里，容易超出配额导致保存失败：迁移到单独的 key
    if (parsed && typeof parsed === "object" && "bgImage" in parsed) {
      const old = parsed as Record<string, unknown>;
      const oldImg = old["bgImage"];
      delete old["bgImage"];
      if (typeof oldImg === "string" && oldImg.startsWith("data:image") && !localStorage.getItem(KEY_IMAGE)) {
        try {
          localStorage.setItem(KEY_IMAGE, oldImg);
        } catch {
          /* 图片过大则丢弃，不影响偏好 */
        }
      }
      const prefs = { ...DEFAULTS, ...old } as Prefs;
      try {
        localStorage.setItem(KEY_PREFS, JSON.stringify(prefs));
      } catch {
        /* 忽略 */
      }
      return prefs;
    }
    return { ...DEFAULTS, ...parsed };
  } catch {
    return { ...DEFAULTS };
  }
}

function loadImage(): string {
  try {
    return localStorage.getItem(KEY_IMAGE) ?? "";
  } catch {
    return "";
  }
}

/* ---------- 背景亮度/对比度计算工具（BUG-2026-0919-002） ---------- */

function hexToRgb(hex: string): [number, number, number] {
  let h = hex.replace("#", "").trim();
  if (h.length === 3) h = h.split("").map((c) => c + c).join("");
  const n = Number.parseInt(h.slice(0, 6) || "000000", 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** WCAG 相对亮度（sRGB 线性化） */
function relLum(rgb: [number, number, number]): number {
  const f = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * f(rgb[0]) + 0.7152 * f(rgb[1]) + 0.0722 * f(rgb[2]);
}

function contrastRatio(a: number, b: number): number {
  const hi = Math.max(a, b);
  const lo = Math.min(a, b);
  return (hi + 0.05) / (lo + 0.05);
}

function mixRgb(a: [number, number, number], b: [number, number, number], t: number): [number, number, number] {
  return [
    Math.round(a[0] + (b[0] - a[0]) * t),
    Math.round(a[1] + (b[1] - a[1]) * t),
    Math.round(a[2] + (b[2] - a[2]) * t),
  ];
}

const TEXT_DARK: [number, number, number] = [0x1f, 0x27, 0x33]; // 浅色模式主文字
const TEXT_LIGHT: [number, number, number] = [0xf5, 0xf7, 0xff]; // 深色模式主文字

/** 文字色是否属于「深色文字」（决定阴影方向） */
function isDarkText(color: string): boolean {
  const [r, g, b] = hexToRgb(color);
  return r + g + b < 384;
}

/**
 * 全局主题/皮肤状态。
 * 登录页固定使用设计封面；主页与其余页面共用这里的背景（纯色 / 渐变 / 背景图+透明度），
 * 并可与浅色/深色模式自由搭配。所有设置持久化到 localStorage，重启应用后仍生效。
 */
export const useThemeStore = defineStore("theme", () => {
  const saved = loadPrefs();
  const mode = ref<ThemeMode>(saved.mode);
  const bgStyle = ref<BackgroundStyle>(saved.bgStyle);
  const bgColor = ref(saved.bgColor);
  const gradFrom = ref(saved.gradFrom);
  const gradTo = ref(saved.gradTo);
  const gradAngle = ref(saved.gradAngle);
  const bgOpacity = ref(saved.bgOpacity);
  const bgImage = ref(loadImage());

  function persist() {
    const prefs: Prefs = {
      mode: mode.value,
      bgStyle: bgStyle.value,
      bgColor: bgColor.value,
      gradFrom: gradFrom.value,
      gradTo: gradTo.value,
      gradAngle: gradAngle.value,
      bgOpacity: bgOpacity.value,
    };
    try {
      localStorage.setItem(KEY_PREFS, JSON.stringify(prefs));
    } catch {
      /* 忽略 */
    }
  }

  /** 单独保存背景图（压缩后的 data URL），失败不影响其他偏好 */
  function saveImage(data: string) {
    bgImage.value = data;
    try {
      if (data) localStorage.setItem(KEY_IMAGE, data);
      else localStorage.removeItem(KEY_IMAGE);
    } catch {
      /* 图片过大等：保留内存中的值，仅不持久化 */
    }
  }

  /** 切换亮暗模式：若纯色背景仍是另一模式的默认值（用户未自定义），则同步换成对应色调，
   *  避免「深色模式 + 浅色背景」这种不可读组合 */
  function setMode(next: ThemeMode) {
    const pairDefault = next === "dark" ? DEFAULTS.bgColor : DARK_BG;
    if (bgColor.value === pairDefault || bgColor.value === DEFAULTS.bgColor || bgColor.value === DARK_BG) {
      bgColor.value = next === "dark" ? DARK_BG : DEFAULTS.bgColor;
    }
    mode.value = next;
    persist();
  }

  function reset() {
    mode.value = DEFAULTS.mode;
    bgStyle.value = DEFAULTS.bgStyle;
    bgColor.value = DEFAULTS.bgColor;
    gradFrom.value = DEFAULTS.gradFrom;
    gradTo.value = DEFAULTS.gradTo;
    gradAngle.value = DEFAULTS.gradAngle;
    bgOpacity.value = DEFAULTS.bgOpacity;
    saveImage("");
    persist();
  }

  /* ---------- 背景层样式（App.vue 全局背景，作用于除登录页外的所有页面） ---------- */

  /** 底层：纯色或渐变；背景图模式下作为图片底色 */
  const layerBase = computed(() => {
    if (bgStyle.value === "gradient") {
      return { background: `linear-gradient(${gradAngle.value}deg, ${gradFrom.value}, ${gradTo.value})` };
    }
    return { background: bgColor.value };
  });

  /** 图片层：仅背景图模式有值，透明度只淡化图片不影响文字 */
  const layerImage = computed(() => {
    if (bgStyle.value !== "image" || !bgImage.value) return null;
    return {
      backgroundImage: `url(${bgImage.value})`,
      backgroundSize: "cover",
      backgroundPosition: "center",
      opacity: bgOpacity.value,
    };
  });

  /* ---------- 文字配色：两层模型（BUG-2026-0919-002 / BUG-2026-0919-004） ----------
   * 教训：v1 曾把 --color-text 整体改成「与页面背景对比」，但绝大多数文字实际
   * 落在卡片上（卡片底色由模式决定）——深色模式 + 浅色页面背景时卡片文字被
   * 翻成深色，反而看不清（用户截图回归）。
   * 正确模型：
   *   - textColor / subTextColor：跟随浅/深模式 → 用于卡片/面板等**自有底色**区域；
   *   - onBgColor / onBgSubColor：与**实际页面背景**（纯色/渐变/背景图均色×透明度）
   *     做对比度计算 → 仅用于直接落在页面背景上的标题/说明文字。
   * onBg 规则（按用户要求）：a.与所选模式的文字色尽量一致（对比 ≥4.5:1 原样用）；
   * b.不足 4.5:1 时在黑/白两端取对比更高的一端。 */

  const isDark = computed(() => mode.value === "dark");

  /** 背景图平均色（异步采样；null=尚未算出，先按底层纯色处理） */
  const bgImageAvg = ref<[number, number, number] | null>(null);

  function sampleBgImage(dataUrl: string) {
    if (!dataUrl || typeof document === "undefined") {
      bgImageAvg.value = null;
      return;
    }
    const img = new Image();
    img.onload = () => {
      try {
        const N = 32;
        const cv = document.createElement("canvas");
        cv.width = N;
        cv.height = N;
        const ctx = cv.getContext("2d", { willReadFrequently: true })!;
        ctx.drawImage(img, 0, 0, N, N);
        const d = ctx.getImageData(0, 0, N, N).data;
        let r = 0, g = 0, b = 0;
        const px = d.length / 4;
        for (let i = 0; i < d.length; i += 4) {
          r += d[i];
          g += d[i + 1];
          b += d[i + 2];
        }
        bgImageAvg.value = [Math.round(r / px), Math.round(g / px), Math.round(b / px)];
      } catch {
        bgImageAvg.value = null;
      }
    };
    img.onerror = () => (bgImageAvg.value = null);
    img.src = dataUrl;
  }
  watch(bgImage, sampleBgImage, { immediate: true });

  /** 实际背景的 RGB（图层叠加后的等效色） */
  const effectiveBg = computed<[number, number, number]>(() => {
    const base = hexToRgb(bgColor.value);
    if (bgStyle.value === "gradient") {
      return mixRgb(hexToRgb(gradFrom.value), hexToRgb(gradTo.value), 0.5);
    }
    if (bgStyle.value === "image") {
      // 图片层以 bgOpacity 叠在底层纯色之上：等效色 = 图片均色*α + 底色*(1-α)
      const avg = bgImageAvg.value;
      if (!avg) return base;
      return mixRgb(avg, base, 1 - bgOpacity.value);
    }
    return base;
  });

  /** 卡片/面板内文字：跟随模式（卡片底色也由模式决定，永远对比充足） */
  const textColor = computed(() => (isDark.value ? "#f5f7ff" : "#1f2733"));
  const subTextColor = computed(() =>
    isDark.value ? "rgba(225,232,255,.86)" : "rgba(36,48,68,.88)",
  );

  /** 页面背景上的文字：与实际背景做对比度计算（a 尽量一致 / b 对比明显） */
  const onBgColor = computed(() => {
    const bgLum = relLum(effectiveBg.value);
    const preferred = isDark.value ? TEXT_LIGHT : TEXT_DARK;
    if (contrastRatio(relLum(preferred), bgLum) >= 4.5) {
      return isDark.value ? "#f5f7ff" : "#1f2733";
    }
    const pickLight = contrastRatio(relLum(TEXT_LIGHT), bgLum) >= contrastRatio(relLum(TEXT_DARK), bgLum);
    return pickLight ? "#f5f7ff" : "#1f2733";
  });
  const onBgSubColor = computed(() => {
    const [r, g, b] = hexToRgb(onBgColor.value);
    return `rgba(${r},${g},${b},.8)`;
  });

  /* 把「页面背景文字色」写到 body 内联 CSS 变量，供各页头部 title/subtitle 消费；
     --color-text* 不再内联覆盖——恢复由 main.css 的模式类控制（卡片语境）。 */
  function applyTextVars() {
    if (typeof document === "undefined") return;
    const body = document.body;
    body.style.setProperty("--color-on-bg", onBgColor.value);
    body.style.setProperty("--color-on-bg-2", onBgSubColor.value);
    // 背景图模式加一层与文字同向的细描边阴影，抵抗图片亮斑（星空亮部等）
    const onImage = bgStyle.value === "image" && !!bgImage.value;
    body.classList.toggle("theme-on-image", onImage);
    body.style.setProperty(
      "--pm-text-shadow",
      isDarkText(onBgColor.value)
        ? "0 1px 3px rgba(255,255,255,.28)"
        : "0 1px 3px rgba(0,0,0,.38)",
    );
  }
  watch([onBgColor, bgStyle, bgImage], applyTextVars, { immediate: true });

  /* ---------- 把深/浅色模式同步到 body ----------
     这样 main.css 中的 `body.theme-dark { --color-text: ... }` 才能覆盖全局，
     让 `color: inherit` 的元素也跟随主题（修复此前深色模式全局文字隐身的 Bug）。*/
  function applyBodyTheme(dark: boolean) {
    if (typeof document === "undefined") return;
    document.body.classList.toggle("theme-dark", dark);
  }
  // 初始化同步一次（覆盖刷新场景）
  applyBodyTheme(mode.value === "dark");
  watch(mode, (m) => applyBodyTheme(m === "dark"));
  /** 卡片/容器底色：深色模式用深色实底，浅色模式用白色实底，保证图标与文字始终可读 */
  const cardStyle = computed(() =>
    isDark.value
      ? {
          background: "rgba(30,34,46,.92)",
          border: "1px solid rgba(255,255,255,.09)",
        }
      : {
          background: "rgba(255,255,255,.94)",
          border: "1px solid rgba(0,0,0,.07)",
        },
  );

  return {
    mode,
    bgStyle,
    bgColor,
    gradFrom,
    gradTo,
    gradAngle,
    bgOpacity,
    bgImage,
    layerBase,
    layerImage,
    isDark,
    textColor,
    subTextColor,
    onBgColor,
    onBgSubColor,
    cardStyle,
    persist,
    saveImage,
    reset,
    setMode,
  };
});
