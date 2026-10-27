<script setup lang="ts">
/**
 * 主题 / 皮肤设置弹窗（FEAT-086 重构）
 *
 * 双页签结构：
 *  - 预定义效果：三段渐变预设（源自设计图），一键整套联动
 *    （渐变三色标 + 配套组件色 + 组件透明度 + 材质）；材质选项本身已移到自定义页；
 *  - 自定义效果：原主题弹窗全部能力迁入 + 组件透明度滑块 + 卡片/仪表墙材质。
 *
 * 取色统一走 PmColorPicker（自绘）：最近使用固定 5 格、悬停可删，
 * 避免系统取色弹窗的历史颜色无限堆积。
 *
 * 由 Home.vue 渲染并 teleport 到 body；样式走全局 .pm-*（pm-dialog.css）。
 */
import { onBeforeUnmount, onMounted, ref, computed } from "vue";
import { DEFAULT_WALLPAPER, useThemeStore } from "../stores/theme";
import { normalizeHex } from "../utils/color";
import { ALL_PRESETS, PRESET_GROUPS, matchesPreset, presetGradient } from "../utils/presets";
import PmColorPicker from "./ui/PmColorPicker.vue";

const emit = defineEmits<{ close: [] }>();
const theme = useThemeStore();

/* ---------------- 页签（role=tablist + 方向键，dev-memory 4.2 无障碍） ---------------- */
const TABS = [
  { id: "preset", label: "预定义效果" },
  { id: "custom", label: "自定义效果" },
] as const;
type TabId = (typeof TABS)[number]["id"];
const activeTab = ref<TabId>("preset");

function onTabKey(e: KeyboardEvent) {
  if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
  e.preventDefault();
  activeTab.value = activeTab.value === "preset" ? "custom" : "preset";
}

/* ---------------- 预定义效果（两组：深色 / 浅色） ---------------- */
function applyPreset(p: (typeof ALL_PRESETS)[number]) {
  theme.applyPreset(p);
}

/** 当前偏好命中的预设 id（跨两组查，决定选中态高亮）。
 *  matchesPreset 的入参已收窄为 Pick<Prefs, 6 个字段>，这里只摊平它真正读的字段，
 *  不必把 store 整个 $state 传进去。 */
const activePresetId = computed(() => {
  const all = PRESET_GROUPS.flatMap((g) => g.items);
  const hit = all.find((p) =>
    matchesPreset(p, {
      bgStyle: theme.bgStyle,
      gradFrom: theme.gradFrom,
      gradMid: theme.gradMid,
      gradTo: theme.gradTo,
      compColor: theme.compColor,
      compAlpha: theme.compAlpha,
    }),
  );
  return hit?.id ?? "";
});

/** 角度滑块可用性：背景必须处于渐变态（纯色/背景图下改角度无可见效果 → 禁用） */
const angleEnabled = computed(() => theme.bgStyle === "gradient");

/* ---------------- 取色器（自绘，替代系统弹窗的无限历史色） ----------------
 * `<input type="color">` 会把历史颜色交给系统/浏览器维护：选得越多越长，
 * 既不能限制为 5 个也不能删。改用 PmColorPicker（最近 5 个、悬停可删）。 */
type ColorField = "compColor" | "bgColor" | "gradFrom" | "gradMid" | "gradTo";
const colorTarget = ref<{ field: ColorField; title: string } | null>(null);
const colorValue = computed(() => (colorTarget.value ? theme[colorTarget.value.field] : "#ffffff"));

function openColor(field: ColorField, title: string) {
  colorTarget.value = { field, title };
}

function commitColor(v: string) {
  if (!colorTarget.value) return;
  theme[colorTarget.value.field] = v;
  theme.persist();
}

/* ---------------- 自定义效果（原弹窗能力迁入） ---------------- */
const bgFileInput = ref<HTMLInputElement | null>(null);

/** 组件色调色号输入：支持 #ffffff / ffffff / #fff；非法输入回滚为当前值 */
function setCompHex(e: Event) {
  const el = e.target as HTMLInputElement;
  const next = normalizeHex(el.value, theme.compColor);
  theme.compColor = next;
  el.value = next;
  theme.persist();
}

function setBgStyle(v: string) {
  theme.bgStyle = v as "image" | "gradient" | "color";
  theme.persist();
}

/** 压缩选中的图片：最长边限制 1920px、JPEG 0.85，避免 data URL 撑爆 localStorage */
function compressImage(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file);
    const img = new Image();
    img.onload = () => {
      const max = 1920;
      const scale = Math.min(1, max / Math.max(img.width, img.height));
      const w = Math.max(1, Math.round(img.width * scale));
      const h = Math.max(1, Math.round(img.height * scale));
      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      canvas.getContext("2d")!.drawImage(img, 0, 0, w, h);
      URL.revokeObjectURL(url);
      resolve(canvas.toDataURL("image/jpeg", 0.85));
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("图片读取失败"));
    };
    img.src = url;
  });
}

async function onBgFile(e: Event) {
  const input = e.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  try {
    const data = await compressImage(file);
    theme.saveImage(data);
    theme.bgStyle = "image";
    theme.persist();
  } catch (err) {
    console.error("背景图设置失败:", err);
  }
  input.value = "";
}

function clearBgImage() {
  theme.saveImage("");
  if (theme.bgStyle === "image") {
    theme.bgStyle = "color";
    theme.persist();
  }
}

function chooseBgImage() {
  bgFileInput.value?.click();
}

/* ---------------- Esc 关闭（组件自持，Home 只管自己另外两个弹窗） ---------------- */
function onKey(e: KeyboardEvent) {
  if (e.key === "Escape") {
    // 取色器叠在本弹窗之上：Esc 先关它（PmColorPicker 自己处理），
    // 这里必须让行，否则一次 Esc 会把两层一起关掉。
    if (colorTarget.value) return;
    e.preventDefault();
    e.stopPropagation();
    emit("close");
  }
}
onMounted(() => document.addEventListener("keydown", onKey, true));
onBeforeUnmount(() => document.removeEventListener("keydown", onKey, true));
</script>

<template>
  <teleport to="body">
    <div class="pm-modal" @click.self="emit('close')">
      <div class="pm-dialog pm-dialog-wide" role="dialog" aria-modal="true" :style="theme.dialogVarStyle">
        <div class="pm-dialog-head">
          <h3>主题 / 皮肤</h3>
          <span class="pm-hint">主页与各页共用，设置自动保存</span>
        </div>

        <!-- 顶部页签 -->
        <div class="pm-tabs" role="tablist" aria-label="主题设置页签" @keydown="onTabKey">
          <button
            v-for="t in TABS"
            :id="`tab-${t.id}`"
            :key="t.id"
            class="pm-tab"
            type="button"
            role="tab"
            :aria-selected="activeTab === t.id"
            :aria-controls="`panel-${t.id}`"
            :tabindex="activeTab === t.id ? 0 : -1"
            @click="activeTab = t.id"
          >
            {{ t.label }}
          </button>
        </div>

        <!-- ============ 预定义效果 ============ -->
        <div
          v-show="activeTab === 'preset'"
          id="panel-preset"
          class="pm-section"
          role="tabpanel"
          aria-labelledby="tab-preset"
        >
          <p class="pm-hint" style="margin: -4px 0 12px">
            默认外观 = 启动封面（与登录页同一张壁纸 + 落日紫金）；下面预设按**深浅分组**，
            点一下整套应用（渐变 + 组件色 + 模式）。「松绿混色」等彩色预设还会改变
            智慧相册「全局概览」仪表墙的强调色与光晕。
          </p>

          <!-- FEAT-112：深浅模式开关已移除（用户反馈“深色/浅色切换没用”——
               实际上预设自身已携带 mode，单独一个开关只会造出“深色模式 + 浅色预设”
               这种错配态；要换到浅色直接点浅色组里的预设即可）。 -->

          <!-- 材质选项已从预定义页移除（用户反馈：预定义只负责配色/氛围，
               材质属于「自定义效果」的个人口味；且两者的材质语义不同，
               放在预设页容易让人以为会随预设一起换）。见自定义页「卡片材质」。 -->

          <!-- 默认卡（FEAT-094）：点一下回到「启动封面 + 落日紫金」，与底部的「恢复默认」等价。
               以前重置后 5 张预设卡会全部掉高亮（选中态判据只认渐变），这里把默认态补成一张卡。 -->
          <button
            class="pm-preset pm-preset-default"
            :class="{ on: theme.isStartupCover }"
            type="button"
            :aria-pressed="theme.isStartupCover"
            @click="theme.reset()"
          >
            <span
              class="pm-preset-swatch pm-preset-cover"
              :style="{ backgroundImage: `url(${DEFAULT_WALLPAPER})` }"
            ></span>
            <span class="pm-preset-name">
              <span>启动封面（默认）</span>
              <span class="pm-preset-badge">壁纸 · {{ Math.round(theme.bgOpacity * 100) }}%</span>
            </span>
            <span class="pm-preset-dots">
              <i style="background: #2b2140"></i>
              <i style="background: #e8a33d"></i>
              <i style="background: #ffd9a0"></i>
            </span>
            <span v-if="theme.isStartupCover" class="pm-preset-check" aria-hidden="true">✓</span>
          </button>

          <!--
            分组排版（C10）：默认卡通栏独占一行，两组渐变各带一个分组标题。
            为什么加标题而不是加筛选/折叠控件：10 张卡一屏能看完，
            而筛选器会引入第二份状态（「当前筛了哪组」）需要与主题偏好分开持久化，
            对一个一次性浏览的场景不值这个复杂度。
            标题用弱化文字 + 分隔线而非背景块 —— 避免「卡片套卡片」的层级噪点。
          -->
          <div v-for="g in PRESET_GROUPS" :key="g.group" class="pm-preset-group">
            <div class="pm-preset-group-head">
              <span class="pm-preset-group-title">{{ g.title }}</span>
              <span class="pm-preset-group-note">{{ g.note }}</span>
            </div>
            <div class="pm-presets">
              <button
                v-for="p in g.items"
                :key="p.id"
                class="pm-preset"
                :class="{ on: activePresetId === p.id }"
                type="button"
                :aria-pressed="activePresetId === p.id"
                @click="applyPreset(p)"
              >
                <span
                  class="pm-preset-swatch"
                  :style="{ background: presetGradient(p, theme.gradAngle) }"
                ></span>
                <span class="pm-preset-name">
                  <span>{{ p.name }}</span>
                  <span class="pm-preset-badge">{{ p.mode === 'dark' ? '深色' : '浅色' }} · {{ Math.round(p.compAlpha * 100) }}%</span>
                </span>
                <span class="pm-preset-dots">
                  <i v-for="c in p.colors" :key="c" :style="{ background: c }"></i>
                  <i :style="{ background: p.compColor }" title="组件色"></i>
                  <i v-if="p.accent" :style="{ background: p.accent }" title="仪表墙强调色"></i>
                </span>
                <span v-if="activePresetId === p.id" class="pm-preset-check" aria-hidden="true">✓</span>
              </button>
            </div>
          </div>

          <!-- 角度：作用于当前预设渐变；未套用预设（背景非渐变）时禁用，避免“改了没反应” -->
          <label class="pm-range" :class="{ 'pm-range-off': !angleEnabled }">
            <span>角度</span>
            <input
              type="range"
              v-model.number="theme.gradAngle"
              min="0"
              max="360"
              :disabled="!angleEnabled"
              aria-label="预设渐变角度"
              @change="theme.persist()"
            />
            <b>{{ theme.gradAngle }}°</b>
          </label>
          <p v-if="!angleEnabled" class="pm-hint" style="margin: 4px 0 0">
            先点一套预设应用后即可调角度
          </p>
        </div>

        <!-- ============ 自定义效果 ============ -->
        <div
          v-show="activeTab === 'custom'"
          id="panel-custom"
          role="tabpanel"
          aria-labelledby="tab-custom"
        >
          <div class="pm-section">
            <div class="pm-section-title">材质</div>
            <div class="pm-seg">
              <button
                type="button"
                :class="{ on: theme.material === 'plain' }"
                @click="theme.material = 'plain'; theme.persist()"
              >
                普通卡片
              </button>
              <button
                type="button"
                :class="{ on: theme.material === 'liquid' }"
                @click="theme.material = 'liquid'; theme.persist()"
              >
                液态玻璃
              </button>
            </div>
            <p class="pm-hint" style="margin: 6px 0 0">
              普通：无模糊、无纹理的卡片 · 液态：边缘折射 + 壁厚高光
              · 全局生效：卡片、面板、仪表墙 widget 一并用这一套材质
            </p>

          </div>

          <div class="pm-section">
            <div class="pm-section-title">组件色调</div>
            <p class="pm-hint pm-comp-hint">卡片 / 面板等组件的底色 · 默认白色</p>
            <div class="pm-color-row">
              <button
                class="pm-swatch-btn"
                type="button"
                :style="{ background: theme.compColor }"
                aria-label="组件色调色块（点击打开调色盘）"
                title="点击选择颜色"
                @click="openColor('compColor', '组件色调')"
              ></button>
              <input
                class="pm-hex pm-mono"
                type="text"
                :value="theme.compColor"
                maxlength="7"
                spellcheck="false"
                aria-label="组件色调色号，如 #ffffff"
                @change="setCompHex"
                @keydown.enter="setCompHex"
              />
            </div>
            <label class="pm-range">
              <span>组件透明度</span>
              <input
                type="range"
                v-model.number="theme.compAlpha"
                min="0"
                max="0.9"
                step="0.01"
                aria-label="组件玻璃透明度（不透明度）"
                @change="theme.persist()"
              />
              <b>{{ Math.round(theme.compAlpha * 100) }}%</b>
            </label>
            <!-- FEAT-094：透明度能拉到 0，但代价当场说清楚 ——
                 文字色是按背景**均值**推的，卡片的填充就是那层「把局部差异抹平」的底；
                 这条读数逐带算过，比一个拍脑袋的下限诚实。 -->
            <p class="pm-hint" :class="{ 'pm-warn': theme.readability.worst < 4.5 }" style="margin: 2px 0 0">
              实测最深色带上的文字对比 ≈ {{ theme.readability.worst.toFixed(1) }}:1
              <template v-if="theme.readability.worst < 4.5">
                （低于 4.5:1：当前背景明暗差较大，透明度低时暗带上的字会看不清——调高透明度，或换深色/深渐变色背景）
              </template>
            </p>

            <!-- FEAT-087：全局饱和度 —— 一处控制背景/组件色调/品牌色/语义色的浓淡，统一风格 -->
            <label class="pm-range">
              <span>全局饱和度</span>
              <input
                type="range"
                v-model.number="theme.saturation"
                min="0.4"
                max="1.5"
                step="0.05"
                aria-label="全局饱和度缩放"
                @change="theme.persist()"
              />
              <b>{{ Math.round(theme.saturation * 100) }}%</b>
            </label>
            <p class="pm-hint" style="margin: 6px 0 0">
              统一调色：背景 / 组件色调 / 按钮与状态色同时生效，只改浓淡不改明暗（100% = 原始色）
            </p>
            <!--
              Q3 的「色温」滑块已移除（用户决定）：色相调节交给上面的调色盘，
              滑块与色盘功能重叠，且滑块只能沿单一暖冷轴走、表达力反而更窄。

              但 utils/prefs.ts 的 temperature 字段与 utils/color.ts 的 applyTemperature
              **刻意保留**：老安装的 localStorage 里可能已写了这个字段，
              若从 Prefs 结构里删掉它，normalizePrefs 会把整套偏好打回默认
              （背景图、预设选择全丢，不可逆）。保留字段、值恒为 0 才是零风险做法。
            -->
          </div>

          <div class="pm-section">
            <div class="pm-section-title">背景样式</div>
            <div class="pm-seg">
              <button
                v-for="opt in [
                  { id: 'image', label: '背景图' },
                  { id: 'gradient', label: '渐变色' },
                  { id: 'color', label: '纯色' },
                ]"
                :key="opt.id"
                type="button"
                :class="{ on: theme.bgStyle === opt.id }"
                @click="setBgStyle(opt.id)"
              >
                {{ opt.label }}
              </button>
            </div>
          </div>

          <div v-if="theme.bgStyle === 'color'" class="pm-section">
            <label class="pm-section-title">选择颜色</label>
            <div class="pm-color-row">
              <button
                class="pm-swatch-btn"
                type="button"
                :style="{ background: theme.bgColor }"
                aria-label="背景纯色（点击打开调色盘）"
                title="点击选择颜色"
                @click="openColor('bgColor', '背景颜色')"
              ></button>
              <span class="pm-mono">{{ theme.bgColor }}</span>
            </div>
          </div>

          <div v-else-if="theme.bgStyle === 'gradient'" class="pm-section">
            <div class="pm-section-title">渐变配色（三段）</div>
            <div class="pm-grade">
              <button
                class="pm-swatch-btn"
                type="button"
                :style="{ background: theme.gradFrom }"
                aria-label="渐变起始色（点击打开调色盘）"
                title="起始色"
                @click="openColor('gradFrom', '渐变起始色')"
              ></button>
              <button
                class="pm-swatch-btn"
                type="button"
                :style="{ background: theme.gradMid }"
                aria-label="渐变中段色（点击打开调色盘）"
                title="中段色"
                @click="openColor('gradMid', '渐变中段色')"
              ></button>
              <button
                class="pm-swatch-btn"
                type="button"
                :style="{ background: theme.gradTo }"
                aria-label="渐变结束色（点击打开调色盘）"
                title="结束色"
                @click="openColor('gradTo', '渐变结束色')"
              ></button>
            </div>
            <label class="pm-range">
              <span>角度</span>
              <input type="range" v-model.number="theme.gradAngle" min="0" max="360" @change="theme.persist()" />
              <b>{{ theme.gradAngle }}°</b>
            </label>
          </div>

          <div v-else class="pm-section">
            <div class="pm-section-title">背景图与透明度</div>
            <button class="pm-btn" type="button" @click="chooseBgImage">选择本地图片</button>
            <input ref="bgFileInput" type="file" accept="image/*" class="pm-hidden-input" @change="onBgFile" />
            <button
              v-if="theme.bgImage"
              class="pm-btn pm-btn-clear"
              type="button"
              @click="clearBgImage"
            >
              清除背景图
            </button>
            <label class="pm-range">
              <span>透明度</span>
              <input type="range" v-model.number="theme.bgOpacity" min="0.05" max="1" step="0.05" @change="theme.persist()" />
              <b>{{ Math.round(theme.bgOpacity * 100) }}%</b>
            </label>
            <div
              v-if="theme.bgImage"
              class="pm-preview"
              :style="{ backgroundImage: `url(${theme.bgImage})` }"
            ></div>
          </div>
        </div>

        <div class="pm-actions">
          <button class="pm-btn" type="button" @click="theme.reset()">恢复默认</button>
          <button class="pm-btn pm-btn-primary" type="button" @click="emit('close')">完成</button>
        </div>
      </div>
    </div>

    <PmColorPicker
      :visible="!!colorTarget"
      :model-value="colorValue"
      :title="colorTarget?.title"
      @update:model-value="commitColor"
      @close="colorTarget = null"
    />
  </teleport>
</template>
