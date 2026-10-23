import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import {
  componentTone,
  contrastRatio,
  glassBandContrast,
  gradientAverage,
  hexToRgb,
  hexToRgba,
  applyTemperature,
  isDarkText,
  linkOnGlass,
  mixRgb,
  onBgText,
  relLum,
  rgbToHex,
  saturateColor,
  type Rgb,
} from "../utils/color";
import { DEFAULTS, isDefaultLook, normalizePrefs, type BackgroundStyle, type Material, type Prefs, type ThemeMode } from "../utils/prefs";
import { presetPrefs, type Preset } from "../utils/presets";
/* FEAT-094：默认背景图 = 登录页那张启动封面。作为构建产物里的内置资源引用
   （Vite 会给出带哈希的 URL），**不写 localStorage** —— 否则 176KB 的 data URL
   很容易撞上存储配额，进而把整套偏好保存都拖掉（本文件开头的键分离就是这么来的）。 */
import defaultWallpaper from "../../covers/login-sunset.jpg";

/** 偏好设置与背景图分开存储：
 *  - 背景图 data URL 可能几百 KB，若和偏好一起写，超出 localStorage 配额时会导致
 *    整个主题保存失败（表现为"下次登录设置就丢了"）。分开存，图片写失败也不影响偏好。 */
const KEY_PREFS = "pm-theme";
const KEY_IMAGE = "pm-theme-image";

/** 深色模式对应的默认纯色背景（setMode 在亮/暗之间同步底色时用） */
const DARK_BG = "#1a1428";

/** 默认背景图（启动封面）的构建产物 URL */
export const DEFAULT_WALLPAPER = defaultWallpaper;

/** 背景图遮罩的取色与四个端点（layerScrim 与「可读性读数」共用，避免两处漂移） */
const SCRIM_RGB: Rgb = [10, 8, 20];
const SCRIM_STOPS = [0.52, 0.34, 0.26, 0.38];
/** 遮罩的等效不透明度（四端点均值）：把背景图折算成「等效色带」时用它 */
const SCRIM_MEAN_ALPHA = SCRIM_STOPS.reduce((a, b) => a + b, 0) / SCRIM_STOPS.length;

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
  /* Q3 色温：**模块已停用**（色相调节交给调色盘，滑块功能重叠且只能走单一暖冷轴）。
     连同 applyTemperature 的九个接入点一起保留，只把值钉死为 0。

     为什么不能连字段一起删：老安装的 localStorage 里可能已写入 temperature，
     若从 Prefs 结构里拿掉它，normalizePrefs 会把整套偏好打回默认
     （背景图、预设选择全丢，且不可逆）。字段留着、值恒 0 ⇒ 外观逐像素不变。

     为什么保留管线：将来若要把色温做成调色盘预设（如「暖光/冷光/中性」三档），
     只需改 temp() 的入参，其余接入点无需改动。applyTemperature 在 t=0 时严格
     返回原色（color.test.ts 已对全站在用的每个色值断言过），所以停用状态
     零开销、零视觉变化。 */
  /** FEAT-086：卡片材质（普通/液态），预设一律普通卡片 */
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

  /* 生效的背景图：用户自定义图优先，否则回落到内置的启动封面。
     必须声明在前（放在后面会被 layerImage / watch 的 immediate 提前引用 → TDZ，
     FEAT-086 的 containerAlpha 踩过同一个坑）。 */
  const bgImageEff = computed(() => bgImage.value || DEFAULT_WALLPAPER);
  /** 是否在用内置封面（决定主题弹窗默认卡的高亮状态）——第二个参数是「用户有没有自定义图」 */
  const isStartupCover = computed(() => isDefaultLook(prefsSnapshot(), !!bgImage.value));

  /** 当前偏好的纯对象快照（persist 与「是否默认外观」共用一个来源，避免两处字段清单漂移） */
  function prefsSnapshot(): Prefs {
    return {
      mode: mode.value,
      compColor: compColor.value,
      compAlpha: compAlpha.value,
      saturation: saturation.value,
      temperature: 0, /* 色温模块停用：永远写 0，旧值不再被读回（见上方说明） */
      material: material.value,
      bgStyle: bgStyle.value,
      bgColor: bgColor.value,
      gradFrom: gradFrom.value,
      gradMid: gradMid.value,
      gradTo: gradTo.value,
      gradAngle: gradAngle.value,
      bgOpacity: bgOpacity.value,
    };
  }

  function persist() {
    try {
      localStorage.setItem(KEY_PREFS, JSON.stringify(prefsSnapshot()));
    } catch {
      /* 忽略 */
    }
  }

  /** FEAT-086：一键套用预定义效果（整套联动：三段渐变 + 组件色 + 透明度 + 普通卡片） */
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
    /* FEAT-103：预设现在自带深浅模式（深浅配对）。
       必须走 setMode 而不是直接 mode.value = ...，因为 setMode 还负责
       "把纯色背景从另一模式的默认值同步过去"（避免深色模式 + 浅色纯底色
       这种不可读组合）。直接赋值会让用户切到浅色预设后，如果之前用的是
       纯色背景，仍停在深色底上。
       顺序：先设定预设色值（上面），再切模式 —— 切模式时 bgColor 的判断
       不依赖 gradFrom/gradTo（它只管纯色），两者不冲突。 */
    if (patch.mode && patch.mode !== mode.value) {
      setMode(patch.mode);
    }
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

  /* ---------- Q3：色温管线（已停用，见下） ----------
     色温滑块已按用户决定移除（色相调节交给调色盘，滑块功能重叠且只能走单一轴）。
     但**管线本身刻意保留**，只把值钉死为 0：

     为什么不能连字段一起删：老安装的 localStorage 里可能已写入 temperature，
     若从 Prefs 结构里拿掉它，normalizePrefs 会把整套偏好打回默认
     （背景图、预设选择全丢，且不可逆）。字段留着、值恒 0 ⇒ 外观逐像素不变。

     为什么保留 applyTemperature 的调用点：将来若要把色温做成调色盘预设
     （比如「暖光/冷光/中性」三档），只需改这一行的入参，其余九个接入点无需改动。
     applyTemperature 在 temperature=0 时严格返回原色（color.test.ts 已断言），
     所以停用状态零开销、零视觉变化。 */
  const temp = (hex: string) => applyTemperature(sat(hex), DEFAULTS.temperature);
  /* 经色温后的四个背景/组件入口。中间层不再是 compColorEff 这类「只过饱和度」的
     访问器 —— 保留它们会让两条管线并存、下游随时可能取错一条（那正是本次要消除的）。 */
  const compColorTemp = computed(() => temp(compColor.value));
  const bgColorTemp = computed(() => temp(bgColor.value));
  const gradFromTemp = computed(() => temp(gradFrom.value));
  const gradMidTemp = computed(() => temp(gradMid.value));
  const gradToTemp = computed(() => temp(gradTo.value));

  /* ---------- 背景层样式（App.vue 全局背景，作用于除登录页外的所有页面） ---------- */

  /** 底层：纯色或三段渐变；背景图模式下作为图片底色 */
  const layerBase = computed(() => {
    if (bgStyle.value === "gradient") {
      // FEAT-086：三段色标（旧数据无中段时 normalizePrefs 已补出中点，兼容两段视觉）
      return {
        background: `linear-gradient(${gradAngle.value}deg, ${gradFromTemp.value}, ${gradMidTemp.value}, ${gradToTemp.value})`,
      };
    }
    return { background: bgColorTemp.value };
  });

  /** 图片层：仅背景图模式有值，透明度只淡化图片不影响文字 */
  const layerImage = computed(() => {
    if (bgStyle.value !== "image") return null;
    return {
      backgroundImage: `url(${bgImageEff.value})`,
      backgroundSize: "cover",
      backgroundPosition: "center",
      opacity: bgOpacity.value,
    };
  });

  /**
   * 背景图上的遮罩（FEAT-094）。
   * 页面级文字（页头标题、面包屑、返回/主页按钮、相册统计）是直掽压在背景上的，
   * 而壁纸的局部亮度差很大（落日图：底部深靖山 0.05 → 天空亮带 0.55，相差 10 倍），
   * 主题只能给一个文字色 —— 实测白字在天空带上只有 3.3~4.1:1（不够 4.5）。
   * 所以补一层**上重下轻**的遮罩：页头永远在最上方 → 上端压得较重；
   * 中部留给卡片（卡片自带玻璃底），底部轻收以免画面发死。
   * 这里不用渐变背景那种「按均值算对比」的自适应 —— 局部亮块靠遮罩硬压，
   * 与登录页（.auth-overlay）同一思路。
   */
  const layerScrim = computed(() => {
    if (bgStyle.value !== "image") return null;
    const [a0, a1, a2, a3] = SCRIM_STOPS;
    const c = rgbToHex(SCRIM_RGB);
    return {
      backgroundImage:
        `linear-gradient(180deg, ${hexToRgba(c, a0)} 0%, ${hexToRgba(c, a1)} 34%, ` +
        `${hexToRgba(c, a2)} 66%, ${hexToRgba(c, a3)} 100%)`,
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
  /* 背景图的「暗 / 中 / 亮」三条分位色带（FEAT-094）：界面要如实告诉你
     「当前组件透明度下最差的那条带还剩多少对比」——只看均值会掩盖局部亮块的危害。 */
  const bgImageBands = ref<Rgb[] | null>(null);
  function sampleBgImage(dataUrl: string) {
    if (!dataUrl || typeof document === "undefined") {
      bgImageAvg.value = null;
      bgImageBands.value = null;
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
        const colors: Rgb[] = [];
        for (let i = 0; i < d.length; i += 4) {
          r += d[i];
          g += d[i + 1];
          b += d[i + 2];
          colors.push([d[i], d[i + 1], d[i + 2]]);
        }
        bgImageAvg.value = [Math.round(r / px), Math.round(g / px), Math.round(b / px)];
        // 分位采样：P05 / P50 / P95 三条带，抗单像素噪声
        colors.sort((a, c) => relLum(a) - relLum(c));
        const at = (t: number) => colors[Math.min(colors.length - 1, Math.floor(t * colors.length))];
        bgImageBands.value = [at(0.05), at(0.5), at(0.95)];
      } catch {
        bgImageAvg.value = null;
        bgImageBands.value = null;
      }
    };
    img.onerror = () => {
      bgImageAvg.value = null;
      bgImageBands.value = null;
    };
    img.src = dataUrl;
  }
  watch(bgImageEff, sampleBgImage, { immediate: true });

  /** 实际背景的 RGB（图层叠加后的等效色；三段渐变取分段积分均值） */
  const effectiveBg = computed<[number, number, number]>(() => {
    const base = hexToRgb(bgColorTemp.value);
    if (bgStyle.value === "gradient") {
      return gradientAverage(
        hexToRgb(gradFromTemp.value),
        hexToRgb(gradMidTemp.value),
        hexToRgb(gradToTemp.value),
      );
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
   * 液态填充色（FEAT-086 表面光学 / FEAT-101 M3）：向白提亮 12% 呈现「湿玻璃」。
   * 普通卡片维持组件原色。
   *
   * M3：20% → 12%。掺白是「奶雾感」的来源，掺到 20% 时卡面偏奶白、
   * 看不出背后在透——「玻璃」变成了「上漆的板」。降到 12% 保住湿感，
   * 把透明度还给玻璃；文字对比由 glassRgb 同步重算，不需另调。
   *
   * 在 compFill 入口收口（而非只改 cardBg）：--color-surface/--glass-bg（类路径）与
   * cardStyle（内联路径）同一出口，文字对比计算 glassRgb 也用它，三处永远一致。
   * 注：不采用方案里的 color-mix(...white) —— 它会把 alpha 从 0.42 推到 0.536，
   * 用户调的 compAlpha 滑块被材质嘴改；这里只提亮色相，
   * alpha 只由滑块 + 材质偏移（effectiveAlpha）决定，不让“掺白”顺手把透明度也改了。
   */
  const fillColor = computed(() =>
    material.value === "liquid"
      ? rgbToHex(mixRgb(hexToRgb(compColorTemp.value), [255, 255, 255], 0.12))
      : compColorTemp.value,
  );

  /**
   * FEAT-103 P0：液态的名义透明度**不再大幅下调**（0.12 → 0.04），且加 0.58 下限。
   *
   * 改动理由（与 M3 的取舍不同，不是推翻它，而是补一条它没看到的约束）：
   * M3 想要「更通透」，但那时卡片还是**全屏铺开的主要容器** —— 降通退 −0.12 后，
   * 背景壁纸的光会从卡片下半部透上来，把数据区（图表、柱阵）压成一层灰。
   * 本次重设计之后玻璃只用于**浮层**（搜索、抽屉、灯箱工具条、弹窗），
   * 这些层的后面是**内容（照片/页面）而不是纯壁纸**，于是：
   *   · 降 0.12 会让照片的颜色直接涌进搜索胶囊里，胶囊内的字看不清；
   *   · 降 0.04 既能保住一点点「透」，又不会让底层照片影响可读性。
   * 0.58 下限：低于此值时，大面积深色照片后面的浮层会变成中灰死区
   * （黑白两种文字都上不了 4.5:1），这是实测过的。
   *
   * 安全阀逻辑完整保留 —— 它才是真正做判断的地方，上面的 0.04 只是把「意愿」调小。
   */
  const LIQUID_ALPHA_DROP = 0.04;
  const LIQUID_ALPHA_FLOOR = 0.58;
  const effectiveAlpha = computed(() => {
    const base = compAlpha.value;
    if (material.value !== "liquid") return base;
    // floor 只抬底、不压顶：用户把滑块拉得很高（>0.62）时不应被“降回来”
    const dropped = Math.max(LIQUID_ALPHA_FLOOR, base - LIQUID_ALPHA_DROP);
    if (Math.abs(dropped - base) < 1e-6) return base;
    const fill = hexToRgb(fillColor.value);
    const ratioAt = (a: number) => {
      const glass = mixRgb(effectiveBg.value, fill, a);
      const tone = componentTone(rgbToHex(glass));
      return contrastRatio(relLum(hexToRgb(tone.text)), relLum(glass));
    };
    const baseRatio = ratioAt(base);
    const droppedRatio = ratioAt(dropped);
    if (baseRatio < 4.5) return droppedRatio >= baseRatio ? dropped : base;
    return droppedRatio >= 4.5 ? dropped : base;
  });

  /** 玻璃等效底色：组件色调以 **有效 alpha**（滑块 + 材质偏移）叠在**实际页面背景**上。
   *  文字对比必须对着这个等效色算——玻璃是半透明的，只拿色调原色判断会误判
   *  （深墨绿玻璃叠在白背景上其实是中灰，该配深色字而不是浅色字）。
   *  fillColor 参与：液态掺白后等效底色变浅，文字取向必须跟着变。 */
  const glassRgb = computed(() =>
    mixRgb(effectiveBg.value, hexToRgb(fillColor.value), effectiveAlpha.value),
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
    // 背景上的链接色：body.theme-dark 的 --color-link 恒为浅金（玻璃卡语境），
    // 落在页面背景上的链接需要另一套 —— 浅背景→深金，深背景→浅金
    body.style.setProperty("--color-link-on-bg", sat(onBgDark.value ? "#8a5a12" : "#ffd9a0"));
    // 背景图模式加一层与文字同向的细描边阴影，抵抗图片亮斑（星空亮部等）
    const onImage = bgStyle.value === "image" && !!bgImageEff.value;
    body.classList.toggle("theme-on-image", onImage);
    body.style.setProperty(
      "--pm-text-shadow",
      isDarkText(onBgColor.value)
        ? "0 1px 3px rgba(255,255,255,.28)"
        : "0 1px 3px rgba(0,0,0,.38)",
    );
  }
  watch([onBgColor, onBgDark, bgStyle, bgImageEff], applyTextVars, { immediate: true });

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

  /* ---------- 卡片材质切换（FEAT-086 / FEAT-100）：普通 plain / 液态 liquid ----------
     body 挂 mat-* 类，main.css 据此覆写 --glass-blur / --glass-saturate / 高光层；
     卡片（cardStyle 内联）与 .glass-surface（类）两条消费路径同时生效（全局 Q3）。
     旧版本的 frosted / glazed 已在 prefs.normalizePrefs 里回落为 plain。 */
  function applyMaterial(m: Material) {
    if (typeof document === "undefined") return;
    document.body.classList.toggle("mat-plain", m === "plain");
    document.body.classList.toggle("mat-liquid", m === "liquid");
  }
  applyMaterial(material.value);
  watch(material, applyMaterial);

  /* ---------- 组件色调下发（FEAT-084/085/086）：容器 = 玻璃 ----------
     玻璃底色 = 组件色调以 effectiveAlpha（滑块 + 材质偏移）叠在页面背景上（Q2-A），
     文字色对**玻璃等效底色**取对比更高的一侧；所有值常驻下发，
     换背景色/背景图/组件色调/透明度/材质都会重新推导。 */

  /** 玻璃分层透明度（由滑块 compAlpha + 材质偏移派生）：
   *  容器 = effectiveAlpha / 次级面板 +0.24 / 弹层 +0.36，封顶 0.95。
   *  必须声明在 applyCompColor 的 immediate watch 之前（TDZ：否则首帧就抛
   *  Cannot access 'containerAlpha' before initialization —— 冒烟实测踩过）。 */
  const containerAlpha = computed(() => effectiveAlpha.value);
  const panelAlpha = computed(() => Math.min(0.95, effectiveAlpha.value + 0.24));
  const dialogAlpha = computed(() => Math.min(0.95, effectiveAlpha.value + 0.36));

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
  watch([compColor, compAlpha, effectiveBg, material], applyCompColor, { immediate: true });

  /* ---------- FEAT-087：品牌色 / 语义色族下发 ----------
     组件里已把硬编码色值注册成 var(--color-*)（codemod 扫描替换），
     这里用同一个 sat() 管线重发一遍 —— 饱和度滑块一处生效，
     按钮/链接/圆点/进度条/危提示全部跟着变，风格自然统一。
     基值与 main.css :root 保持一致（saturation=1 时与旧版逐像素相同）。 */
  function applyAccentVars() {
    if (typeof document === "undefined") return;
    const body = document.body;
    const P = "#e8a33d"; // 品牌主色（main.css :root --color-primary）：FEAT-094 由蓝改「落日金」
    const PH = "#f2b355";
    /* 金底白字只有 2.16:1 ⇒ 填充类必须配墨字；金色当**文字**写在深玻璃上时
       用浅金档 #ffd9a0（≈7:1），不能直接用填充金（≈3.7:1 不够 4.5）。 */
    const PRIMARY_TEXT = "#ffd9a0";
    const ON_PRIMARY = "#2a1a08";
    const LINK_LIGHT = "#8a5a12"; // 浅背景上的链接（深金）
    const D = "#d13438";
    const DH = "#b92e33";
    const OK = "#15803d";
    const OKV = "#16a34a"; // 进度/成功强调绿
    const W = "#b45309";
    /* 品牌主色 + candy 色板走**饱和度 → 色温**双管线（审美色，可以被染色温）。
       语义色族（成功绿/危险红/警示橙）只走饱和度、**绝不接色温**：
       色温是沿色相轴推的，危险红推暖就成橙红、成功绿推冷就成青绿，
       状态色的含义会被染色改掉。饱和度无此问题（红仍是红，只改浓淡）。 */
    body.style.setProperty("--color-primary", temp(P));
    body.style.setProperty("--color-primary-hover", temp(PH));
    /* FEAT-094 新增两档：金填充上允许读的文字色 / 深底上的金色文字档 */
    body.style.setProperty("--color-on-primary", ON_PRIMARY);
    body.style.setProperty("--color-primary-text", temp(PRIMARY_TEXT));
    body.style.setProperty("--color-link-text-light", LINK_LIGHT);
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
       饱和度 + 色温统一带动（否则滑块只影响按钮/背景，糖果卡原地不动）。 */
    body.style.setProperty("--candy-cream", temp("#fffcbd"));
    body.style.setProperty("--candy-pink", temp("#f075c7"));
    body.style.setProperty("--candy-blue", temp("#65d5f9"));
    body.style.setProperty("--candy-rose", temp("#fb6d9b"));
    body.style.setProperty("--candy-indigo", temp("#505fdd"));
    body.style.setProperty("--candy-violet", temp("#3a36e4"));
    /* 玻璃卡语境的链接色（Q5=A 的 Bug 修复）：**按玻璃等效底色现算**，不再用 isDark 二分。
       旧写法给一个固定深金（LINK_LIGHT #8a5a12），但链接实际落在玻璃上 —— 浅玻璃 +
       恒定 dark mode 正好是最差组合，实测 5 套预设只有 2.20~2.64:1（AA 需 4.5:1），
       Home 的「进入 →」因此发灰看不见。现在深底自动用浅金档（观感零变化），
       浅底固定金相、在亮度轴上扫到首个达标色（utils/color.ts 的 linkOnGlass）。 */
    body.style.setProperty("--color-link", sat(linkOnGlass(rgbToHex(glassRgb.value))));
  }
  // temperature 已钉死为 0（色温模块停用），不再作为依赖 —— 少一个 watch 触发源
  watch([saturation, isDark, glassRgb], applyAccentVars, { immediate: true });

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
  /** 普通卡片关闭 backdrop-filter；液态保留模糊 + 饱和。
   *  内联 cardStyle 会覆盖 CSS 规则，所以这里必须与材质同步下发 "none"。 */
  const glassBackdrop = computed(() =>
    material.value === "plain"
      ? "none"
      : "blur(var(--glass-blur)) saturate(var(--glass-saturate))",
  );

  /** 卡片/容器整套样式（内联 style 直接消费）：材质两件套。
   *  液态的高光/光池/暗角全部引用全局令牌（--liquid-highlight 等 + ::before/::after 层），
   *  这里只下发「内联路径独有的那一部分」：模糊与填充。
   *  —— 材质层的元素级 mask/滤镜走伪元素，cardStyle 不重复它们，
   *  避免同一样式在类路径与内联路径下画出两套不同的光学。 */
  const cardStyle = computed(() => ({
    backgroundColor: cardBg.value,
    /* 描边/阴影不再内联：内联 border/box-shadow 会盖掉 :hover 的 border-color 与
       var(--shadow-2)。所有 cardStyle 消费方都挂了 .glass-card，统一由类规则下发。 */
    /* 修补：内联路径此前缺 -webkit- 前缀，Safari/WebView 下零模糊 */
    WebkitBackdropFilter: glassBackdrop.value,
    backdropFilter: glassBackdrop.value,
  }));

  /** 弹窗面板变量包（--pm-*）：Home 基本信息弹窗与 ThemeDialog 共用（FEAT-086）。
   *  弹层 = 玻璃 + 高模糊 + 三级深阴影；颜色随组件色调/材质实时重推。 */
  const dialogVarStyle = computed(() => ({
    background: dialogFill.value,
    WebkitBackdropFilter: glassBackdrop.value,
    backdropFilter: glassBackdrop.value,
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

  /**
   * 当前背景 + 当前组件透明度下，「最差的那条色带」上的文字对比（FEAT-094）。
   * 主题弹窗把它如实显示在透明度滑块下面 —— 是这个值（而不是拍脑袋的下限）
   * 决定用户能不能把透明度拉到 0：背景越均匀、越暗，这个值越高，0 就真的可行。
   * 放在 effectiveBg / fillColor 之后声明：computed 虽惰性，但同层 watch 可能提前触发。
   */
  const readability = computed(() => {
    const scrim = rgbToHex(SCRIM_RGB);
    /* 背景图：色带要按**实际呈现**折算 —— 先按 bgOpacity 叠到底色上，再过一层遮罩（取端点均值）。
       直接用原始壁纸色带会把读数报得过于悲观（实测过：真实 6.7:1 被报成 1.5:1，等于误报）。 */
    const imageBands = (bgImageBands.value ?? []).map((b) =>
      mixRgb(mixRgb(b, hexToRgb(bgColorTemp.value), 1 - bgOpacity.value), hexToRgb(scrim), SCRIM_MEAN_ALPHA),
    );
    const bands: Rgb[] =
      bgStyle.value === "gradient"
        ? [hexToRgb(gradFromTemp.value), hexToRgb(gradMidTemp.value), hexToRgb(gradToTemp.value)]
        : bgStyle.value === "image" && imageBands.length
          ? imageBands
          : [hexToRgb(bgColorTemp.value)];
    const avg =
      bgStyle.value === "gradient"
        ? gradientAverage(
            hexToRgb(gradFromTemp.value),
            hexToRgb(gradMidTemp.value),
            hexToRgb(gradToTemp.value),
          )
        : bgStyle.value === "image"
          ? mixRgb(effectiveBg.value, hexToRgb(scrim), SCRIM_MEAN_ALPHA)
          : effectiveBg.value;
    return glassBandContrast(bands, avg, fillColor.value, effectiveAlpha.value);
  });

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
    layerScrim,
    bgImageEff,
    isStartupCover,
    isDark,
    textColor,
    subTextColor,
    onBgColor,
    onBgSubColor,
    onBgDark,
    cardStyle,
    cardBg,
    cardBorder,
    readability,
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
