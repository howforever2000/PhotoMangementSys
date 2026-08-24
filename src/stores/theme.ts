import { defineStore } from "pinia";
import { computed, ref } from "vue";

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

  /* ---------- 文字/卡片配色（跟随浅色/深色模式） ---------- */

  const isDark = computed(() => mode.value === "dark");
  const textColor = computed(() => (isDark.value ? "#f5f7ff" : "#1f2733"));
  const subTextColor = computed(() =>
    isDark.value ? "rgba(214,221,240,.72)" : "rgba(60,70,90,.75)",
  );
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
    cardStyle,
    persist,
    saveImage,
    reset,
    setMode,
  };
});
