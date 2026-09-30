import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import {
  componentTone,
  gradientAverage,
  hexToRgb,
  hexToRgba,
  isDarkText,
  mixRgb,
  onBgText,
  rgbToHex,
  saturateColor,
} from "../utils/color";
import { DEFAULTS, normalizePrefs, type BackgroundStyle, type Material, type Prefs, type ThemeMode } from "../utils/prefs";
import { presetPrefs, type Preset } from "../utils/presets";

/** 偏好设置与背景图分开存储：
 *  - 背景图 data URL 可能几百 KB，若和偏好一起写，超出 localStorage 配额时会导致
 *    整个主题保存失败（表现为"下次登录设置就丢了"）。分开存，图片写失败也不影响偏好。 */
const KEY_PREFS = "pm-theme";
const KEY_IMAGE = "pm-theme-image";

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
      const prefs = normalizePrefs(old);
      try {
        localStorage.setItem(KEY_PREFS, JSON.stringify(prefs));
      } catch {
        /* 忽略 */
      }
      return prefs;
    }
    // FEAT-086：旧结构（无 gradMid/compAlpha/material）在这里归一化补齐
    return normalizePrefs(parsed);
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

/* 背景亮度/对比度与组件色调派生的纯函数已抽到 utils/color.ts（FEAT-084）：
   同一套 RGB/WCAG 工具供背景对比与组件色调共用，且可被 node:test 直接单测。 */

/**
 * 全局主题/皮肤状态。
 * 登录页固定使用设计封面；主页与其余页面共用这里的背景（纯色 / 渐变 / 背景图+透明度），
 * 并可与浅色/深色模式自由搭配。所有设置持久化到 localStorage，重启应用后仍生效。
 */
export const useThemeStore = defineStore("theme", () => {
  const saved = loadPrefs();
  /* 基础色调入口已移除 + 整体转暗（Q4-A）：模式固定为默认值。
     否则旧 localStorage 里的 light 会让新视觉世界对老安装不生效。 */
  const mode = ref<ThemeMode>(DEFAULTS.mode);
  // 归一化：保证始终是合法小写 hex / 合法数值（坏值回落默认，FEAT-086 走 normalizePrefs）
  const compColor = ref(saved.compColor);
  /** FEAT-086：组件玻璃不透明度（容器基准；面板/弹层按固定偏移派生） */
  const compAlpha = ref(saved.compAlpha);
  /** FEAT-087：全局饱和度缩放（1 = 原始色）——统一风格的总闸门 */
  const saturation = ref(saved.saturation);
  /** FEAT-086：玻璃材质（磨砂/液态），预设一律磨砂 */
  const material = ref<Material>(saved.material);
  const bgStyle = ref<BackgroundStyle>(saved.bgStyle);
  const bgColor = ref(saved.bgColor);
  const gradFrom = ref(saved.gradFrom);
  /** FEAT-086：三段渐变中段色标 */
  const gradMid = ref(saved.gradMid);
  const gradTo = ref(saved.gradTo);
  const gradAngle = ref(saved.gradAngle);
  const bgOpacity = ref(saved.bgOpacity);
  const bgImage = ref(loadImage());

  function persist() {
    const prefs: Prefs = {
      mode: mode.value,
      compColor: compColor.value,
      compAlpha: compAlpha.value,
      saturation: saturation.value,
      material: material.value,
      bgStyle: bgStyle.value,
      bgColor: bgColor.value,
      gradFrom: gradFrom.value,
      gradMid: gradMid.value,
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

  /** FEAT-086：一键套用预定义效果（整套联动：三段渐变 + 组件色 + 透明度 + 磨砂） */
  function applyPreset(p: Preset) {
    const patch = presetPrefs(p);
    bgStyle.value = patch.bgStyle!;
    gradFrom.value = patch.gradFrom!;
    gradMid.value = patch.gradMid!;
    gradTo.value = patch.gradTo!;
    gradAngle.value = patch.gradAngle!;
    compColor.value = patch.compColor!;
    compAlpha.value = patch.compAlpha!;
    material.value = patch.material!;
    persist();
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
    compColor.value = DEFAULTS.compColor;
    compAlpha.value = DEFAULTS.compAlpha;
    saturation.value = DEFAULTS.saturation;
    material.value = DEFAULTS.material;
    bgStyle.value = DEFAULTS.bgStyle;
    bgColor.value = DEFAULTS.bgColor;
    gradFrom.value = DEFAULTS.gradFrom;
    gradMid.value = DEFAULTS.gradMid;
    gradTo.value = DEFAULTS.gradTo;
    gradAngle.value = DEFAULTS.gradAngle;
    bgOpacity.value = DEFAULTS.bgOpacity;
    saveImage("");
    persist();
  }

  /* ---------- FEAT-087：全局饱和度统一管线 ----------
     用户选定的**基础色**（组件色调 / 背景纯色 / 渐变三色标）：
     所有下游派生（玻璃等效底、文字对比、图层 CSS）一律读这两个访问器，
     因此饱和度一处生效、对比度也跟着重算（saturateColor 只改 S 不改 L，
     文字对比不会被饱和度调整弄崩）。
     品牌色/语义色族另在 applyAccentVars 里下发（含组件里已注册的 var 引用）。 */
  const sat = (hex: string) => saturateColor(hex, saturation.value);
  const compColorEff = computed(() => sat(compColor.value));
  const bgColorEff = computed(() => sat(bgColor.value));
  const gradFromEff = computed(() => sat(gradFrom.value));
  const gradMidEff = computed(() => sat(gradMid.value));
  const gradToEff = computed(() => sat(gradTo.value));

  /* ---------- 背景层样式（App.vue 全局背景，作用于除登录页外的所有页面） ---------- */

  /** 底层：纯色或三段渐变；背景图模式下作为图片底色 */
  const layerBase = computed(() => {
    if (bgStyle.value === "gradient") {
      // FEAT-086：三段色标（旧数据无中段时 normalizePrefs 已补出中点，兼容两段视觉）
      return {
        background: `linear-gradient(${gradAngle.value}deg, ${gradFromEff.value}, ${gradMidEff.value}, ${gradToEff.value})`,
      };
    }
    return { background: bgColorEff.value };
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

  /** 实际背景的 RGB（图层叠加后的等效色；三段渐变取分段积分均值） */
  const effectiveBg = computed<[number, number, number]>(() => {
    const base = hexToRgb(bgColorEff.value);
    if (bgStyle.value === "gradient") {
      return gradientAverage(hexToRgb(gradFromEff.value), hexToRgb(gradMidEff.value), hexToRgb(gradToEff.value));
    }
    if (bgStyle.value === "image") {
      // 图片层以 bgOpacity 叠在底层纯色之上：等效色 = 图片均色*α + 底色*(1-α)
      const avg = bgImageAvg.value;
      if (!avg) return base;
      return mixRgb(avg, base, 1 - bgOpacity.value);
    }
    return base;
  });

  /**
   * 液态/釉瓷填充色（FEAT-086 表面光学 + 釉瓷扩展）：向白提亮呈现「湿玻璃/瓷面」。
   * 液态掺白 20%；釉瓷掺白 30%（瓷面更亮更硬挺）。磨砂维持组件原色。
   * 在 compFill 入口收口（而非只改 cardBg）：--color-surface/--glass-bg（类路径）与
   * cardStyle（内联路径）同一出口，文字对比计算 glassRgb 也用它，三处永远一致。
   * 注：不采用方案里的 color-mix(...white) —— 它会把 alpha 从 0.42 推到 0.536，
   * 用户调的 compAlpha 滑块被材质嘴改；这里只提亮色相、alpha 恒由滑块决定。
   */
  const fillColor = computed(() =>
    material.value === "liquid"
      ? rgbToHex(mixRgb(hexToRgb(compColorEff.value), [255, 255, 255], 0.2))
      : material.value === "glazed"
        ? rgbToHex(mixRgb(hexToRgb(compColorEff.value), [255, 255, 255], 0.3))
        : compColorEff.value,
  );

  /** 玻璃等效底色：组件色调以 **compAlpha**（FEAT-086 可调）叠在**实际页面背景**上。
   *  文字对比必须对着这个等效色算——玻璃是半透明的，只拿色调原色判断会误判
   *  （深墨绿玻璃叠在白背景上其实是中灰，该配深色字而不是浅色字）。
   *  fillColor 参与：液态掺白后等效底色变浅，文字取向必须跟着变。 */
  const glassRgb = computed(() =>
    mixRgb(effectiveBg.value, hexToRgb(fillColor.value), compAlpha.value),
  );
  const glassTone = computed(() => componentTone(rgbToHex(glassRgb.value)));

  /** 容器/面板内文字：随玻璃等效底色自动取深/浅，保证 ≥4.5:1（方案 §四.4） */
  const textColor = computed(() => (glassTone.value.onDark ? "#f5f7ff" : "#1f2733"));
  const subTextColor = computed(() =>
    glassTone.value.onDark ? "rgba(228,235,255,.92)" : "rgba(36,48,68,.9)",
  );

  /** 页面背景上的文字：与实际背景做对比度计算
   *  （a 尽量一致 / b 不足 4.5 取更高 / c 双双不足黑白兜底 —— FEAT-086 预设中调背景可达 4.26:1） */
  const onBgColor = computed(() => onBgText(effectiveBg.value, isDark.value));
  const onBgSubColor = computed(() => {
    const [r, g, b] = hexToRgb(onBgColor.value);
    return `rgba(${r},${g},${b},.8)`;
  });
  /** 页面背景上文字的明暗方向：浅背景→深字（true）。供各页 msVars/tsVars 等
   *  页面级变量二分面板底/边框/悬停色的取向（旧写法按 mode.isDark 二分，
   *  在浅色预设 + 恒定 dark mode 下永远下发白字，是「文字融进浅背景」的根因） */
  const onBgDark = computed(() => isDarkText(onBgColor.value));

  /* 把「页面背景文字色」写到 body 内联 CSS 变量，供各页头部 title/subtitle 消费；
     --color-text* 不再内联覆盖——恢复由 main.css 的模式类控制（卡片语境）。 */
  function applyTextVars() {
    if (typeof document === "undefined") return;
    const body = document.body;
    body.style.setProperty("--color-on-bg", onBgColor.value);
    body.style.setProperty("--color-on-bg-2", onBgSubColor.value);
    // 背景上的链接色：body.theme-dark 的 --color-link 恒浅蓝（玻璃卡语境），
    // 落在页面背景上的链接需要另一套 —— 浅背景→深蓝，深背景→浅蓝
    body.style.setProperty("--color-link-on-bg", sat(onBgDark.value ? "#2f5cc2" : "#8ab4ff"));
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
  watch([onBgColor, onBgDark, bgStyle, bgImage], applyTextVars, { immediate: true });

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

  /* ---------- 玻璃材质切换（FEAT-086）：磨砂 frosted / 液态 liquid ----------
     body 挂 mat-* 类，main.css 据此覆写 --glass-blur / --glass-saturate / 高光层；
     卡片（cardStyle 内联）与 .glass-surface（类）两条消费路径同时生效（全局 Q3）。 */
  function applyMaterial(m: Material) {
    if (typeof document === "undefined") return;
    document.body.classList.toggle("mat-frosted", m === "frosted");
    document.body.classList.toggle("mat-liquid", m === "liquid");
    document.body.classList.toggle("mat-glazed", m === "glazed");
  }
  applyMaterial(material.value);
  watch(material, applyMaterial);

  /* ---------- 组件色调下发（FEAT-084/085/086）：容器 = 玻璃 ----------
     玻璃底色 = 组件色调以 compAlpha（可调）叠在页面背景上（Q2-A），
     文字色对**玻璃等效底色**取对比更高的一侧；所有值常驻下发，
     换背景色/背景图/组件色调/透明度/材质都会重新推导。 */

  /** 玻璃分层透明度（由可调 compAlpha 派生）：
   *  容器 = compAlpha（默认 0.42）/ 次级面板 +0.24 / 弹层 +0.36，封顶 0.95。
   *  必须声明在 applyCompColor 的 immediate watch 之前（TDZ：否则首帧就抛
   *  Cannot access 'containerAlpha' before initialization —— 冒烟实测踩过）。 */
  const containerAlpha = computed(() => compAlpha.value);
  const panelAlpha = computed(() => Math.min(0.95, compAlpha.value + 0.24));
  const dialogAlpha = computed(() => Math.min(0.95, compAlpha.value + 0.36));

  function applyCompColor() {
    if (typeof document === "undefined") return;
    const body = document.body;
    const tone = glassTone.value;
    body.style.setProperty("--color-surface", compFill(containerAlpha.value));
    body.style.setProperty("--color-surface-2", compFill(panelAlpha.value));
    body.style.setProperty("--color-border", tone.border);
    body.style.setProperty("--color-text", tone.text);
    body.style.setProperty("--color-text-2", tone.text2);
    body.style.setProperty("--color-text-3", tone.text3);
    /* 别名令牌必须在同一层下发：`--glass-bg: var(--color-surface)` 写在 :root 时，
       会用 :root 的值（白）在 :root 就算完，子元素继承到的是已解析的白底 */
    body.style.setProperty("--glass-bg", compFill(containerAlpha.value));
    body.style.setProperty("--liquid-bg", compFill(panelAlpha.value));
    body.style.setProperty("--glass-border", tone.border);
  }
  watch([compColor, compAlpha, effectiveBg], applyCompColor, { immediate: true });

  /* ---------- FEAT-087：品牌色 / 语义色族下发 ----------
     组件里已把硬编码色值注册成 var(--color-*)（codemod 扫描替换），
     这里用同一个 sat() 管线重发一遍 —— 饱和度滑块一处生效，
     按钮/链接/圆点/进度条/危提示全部跟着变，风格自然统一。
     基值与 main.css :root 保持一致（saturation=1 时与旧版逐像素相同）。 */
  function applyAccentVars() {
    if (typeof document === "undefined") return;
    const body = document.body;
    const P = "#396cd8"; // 品牌主色（main.css :root --color-primary）
    const PH = "#2f5cc2";
    const D = "#d13438";
    const DH = "#b92e33";
    const OK = "#15803d";
    const OKV = "#16a34a"; // 进度/成功强调绿
    const W = "#b45309";
    body.style.setProperty("--color-primary", sat(P));
    body.style.setProperty("--color-primary-hover", sat(PH));
    body.style.setProperty("--color-danger", sat(D));
    body.style.setProperty("--color-danger-hover", sat(DH));
    /* 成功/警示色在 main.css 里是**模式双值**（深色提亮）——这里必须镜像同一套基值，
       否则内联覆写会把深色档压回浅色档（曾把「已入库」徽标的亮绿 #4ade80
       改成深绿 #15803d，对比度回落，被逐像素回归抓到）。 */
    body.style.setProperty("--color-ok", sat(isDark.value ? "#4ade80" : OK));
    body.style.setProperty("--color-warn", sat(isDark.value ? "#fbbf24" : W));
    /* “实底”系列：组件原本硬编码的深色档（白字/亮底配套），不随模式变
       —— 保持与旧版逐像素一致，同时仍受饱和度统一控制 */
    body.style.setProperty("--color-ok-solid", sat(OK));
    body.style.setProperty("--color-warn-solid", sat(W));
    body.style.setProperty("--color-ok-vivid", sat(OKV));
    /* 深底上的语义文字色：单独一档（提亮档）。
       浅色档的 #e03131 在深绿玻璃上仅 3.7:1（11px 文字不达 AA），
       此处给 AA 亮红；仍走同一饱和度管线，亮度锁定所以对比恒定 */
    body.style.setProperty("--color-ok-text", sat(isDark.value ? "#6ed27a" : "#2f9e44"));
    body.style.setProperty("--color-danger-text", sat(isDark.value ? "#f87171" : "#c92a2a"));
    body.style.setProperty("--color-warn-text", sat(isDark.value ? "#f59e0b" : "#b45309"));
    /* 糖果彩虹色板：hero 横幅 / 入口卡 / 回忆故事卡的渐变基色。
       这些渐变串在 <script> 里拼装（内联 :style），所以必须注册成令牌才能被
       饱和度统一带动（否则滑块只影响按钮/背景，糖果卡原地不动）。 */
    body.style.setProperty("--candy-cream", sat("#fffcbd"));
    body.style.setProperty("--candy-pink", sat("#f075c7"));
    body.style.setProperty("--candy-blue", sat("#65d5f9"));
    body.style.setProperty("--candy-rose", sat("#fb6d9b"));
    body.style.setProperty("--candy-indigo", sat("#505fdd"));
    body.style.setProperty("--candy-violet", sat("#3a36e4"));
    /* 玻璃卡语境链接色（body.theme-dark 恒为浅蓝 #8ab4ff）：直接覆写 --color-link，
       组件里的 var(--color-link) 引用自动跟随；仅做饱和度缩放（不改 HSL 亮度） */
    body.style.setProperty("--color-link", sat(isDark.value ? "#8ab4ff" : P));
  }
  watch([saturation, isDark], applyAccentVars, { immediate: true });

  /** 组件色调按透明度渲染：容器 / 次级面板 / 弹层（液态经 fillColor 提亮，见上） */
  function compFill(alpha: number) {
    return hexToRgba(fillColor.value, alpha);
  }
  const glassFill = computed(() => compFill(containerAlpha.value));
  const panelFill = computed(() => compFill(panelAlpha.value));
  const dialogFill = computed(() => compFill(dialogAlpha.value));

  /** 卡片底色（颜色值）：即玻璃填充 */
  const cardBg = computed(() => compFill(containerAlpha.value));
  /** 卡片描边（颜色值）：玻璃边缘内高光，方向随等效底色明暗 */
  const cardBorder = computed(() => glassTone.value.border);
  /** 卡片/容器整套样式（内联 style 直接消费）：玻璃材质三件套。
   *  液态叠 135° 流体高光（--liquid-highlight），釉瓷叠顶部镜面带（--glazed-highlight） */
  const cardStyle = computed(() => ({
    backgroundColor: cardBg.value,
    backgroundImage:
      material.value === "liquid"
        ? "var(--liquid-highlight)"
        : material.value === "glazed"
          ? "var(--glazed-highlight)"
          : "var(--frosted-highlight)",
    border: `1px solid ${cardBorder.value}`,
    /* 修补：内联路径此前缺 -webkit- 前缀，Safari/WebView 下零模糊 */
    WebkitBackdropFilter: "blur(var(--glass-blur)) saturate(var(--glass-saturate))",
    backdropFilter: "blur(var(--glass-blur)) saturate(var(--glass-saturate))",
    boxShadow: "var(--shadow-1)",
  }));

  /** 弹窗面板变量包（--pm-*）：Home 基本信息弹窗与 ThemeDialog 共用（FEAT-086）。
   *  弹层 = 玻璃 + 高模糊 + 三级深阴影；颜色随组件色调/材质实时重推。 */
  const dialogVarStyle = computed(() => ({
    background: dialogFill.value,
    backdropFilter: "blur(var(--glass-blur)) saturate(var(--glass-saturate))",
    boxShadow: "var(--shadow-3)",
    border: `1px solid ${cardBorder.value}`,
    color: textColor.value,
    "--pm-text": textColor.value,
    "--pm-label": subTextColor.value,
    "--pm-hint": subTextColor.value,
    "--pm-input-bg": compFill(0.34),
    "--pm-input-border": cardBorder.value,
    "--pm-input-disabled-bg": compFill(0.16),
    "--pm-btn-bg": compFill(0.2),
    "--pm-btn-color": textColor.value,
    "--pm-btn-hover": compFill(0.32),
    "--pm-soft-border": cardBorder.value,
    "--pm-danger-hover": "color-mix(in srgb, var(--color-danger) 22%, transparent)",
  }));

  return {
    mode,
    compColor,
    compAlpha,
    saturation,
    material,
    bgStyle,
    bgColor,
    gradFrom,
    gradMid,
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
    onBgDark,
    cardStyle,
    cardBg,
    cardBorder,
    compFill,
    glassFill,
    panelFill,
    dialogFill,
    dialogVarStyle,
    persist,
    saveImage,
    reset,
    setMode,
    applyPreset,
  };
});
