<script setup lang="ts">
/**
 * 主题 / 皮肤设置弹窗（FEAT-086 重构）
 *
 * 双页签结构：
 *  - 预定义效果：5 套三段渐变预设（源自设计图），一键整套联动
 *    （渐变三色标 + 配套组件色 + 组件透明度 + 磨砂材质）；
 *  - 自定义效果：原主题弹窗全部能力迁入 + 组件透明度滑块 + 液态/磨砂材质二选一。
 *
 * 由 Home.vue 渲染并 teleport 到 body；样式走全局 .pm-*（pm-dialog.css）。
 */
import { onBeforeUnmount, onMounted, ref, computed } from "vue";
import { useThemeStore } from "../stores/theme";
import { normalizeHex } from "../utils/color";
import { PRESETS, matchesPreset, presetGradient } from "../utils/presets";

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

/* ---------------- 预定义效果 ---------------- */
function applyPreset(p: (typeof PRESETS)[number]) {
  theme.applyPreset(p);
}

/** 当前偏好命中的预设 id（决定选中态高亮） */
const activePresetId = computed(() => {
  const cur = theme.$state;
  const hit = PRESETS.find((p) => matchesPreset(p, { ...cur, mode: theme.mode }));
  return hit?.id ?? "";
});

/** 角度滑块可用性：背景必须处于渐变态（纯色/背景图下改角度无可见效果 → 禁用） */
const angleEnabled = computed(() => theme.bgStyle === "gradient");

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
            5 套设计渐变 · 点击整套应用（背景 + 组件色 + 透明度 + 磨砂质感）
          </p>
          <div class="pm-presets">
            <button
              v-for="p in PRESETS"
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
                <span class="pm-preset-badge">磨砂 · {{ Math.round(p.compAlpha * 100) }}%</span>
              </span>
              <span class="pm-preset-dots">
                <i v-for="c in p.colors" :key="c" :style="{ background: c }"></i>
                <i :style="{ background: p.compColor }" title="组件色"></i>
              </span>
              <span v-if="activePresetId === p.id" class="pm-preset-check" aria-hidden="true">✓</span>
            </button>
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
            <div class="pm-section-title">玻璃材质</div>
            <div class="pm-seg">
              <button
                type="button"
                :class="{ on: theme.material === 'frosted' }"
                @click="theme.material = 'frosted'; theme.persist()"
              >
                磨砂玻璃
              </button>
              <button
                type="button"
                :class="{ on: theme.material === 'liquid' }"
                @click="theme.material = 'liquid'; theme.persist()"
              >
                液态玻璃
              </button>
              <button
                type="button"
                :class="{ on: theme.material === 'glazed' }"
                @click="theme.material = 'glazed'; theme.persist()"
              >
                釉瓷玻璃
              </button>
            </div>
            <p class="pm-hint" style="margin: 6px 0 0">
              磨砂：高模糊 + 粗颗粒喷砂 · 液态：低模糊 + 流体高光 · 釉瓷：陶瓷釉面开片 + 镜面天光（全局生效）
            </p>
          </div>

          <div class="pm-section">
            <div class="pm-section-title">组件色调</div>
            <p class="pm-hint pm-comp-hint">卡片 / 面板等组件的底色 · 默认白色</p>
            <div class="pm-color-row">
              <input
                type="color"
                v-model="theme.compColor"
                @change="theme.persist()"
                aria-label="组件色调色块（点击打开调色盘）"
              />
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
                min="0.15"
                max="0.9"
                step="0.01"
                aria-label="组件玻璃透明度（不透明度）"
                @change="theme.persist()"
              />
              <b>{{ Math.round(theme.compAlpha * 100) }}%</b>
            </label>

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
              <input type="color" v-model="theme.bgColor" @change="theme.persist()" />
              <span class="pm-mono">{{ theme.bgColor }}</span>
            </div>
          </div>

          <div v-else-if="theme.bgStyle === 'gradient'" class="pm-section">
            <div class="pm-section-title">渐变配色（三段）</div>
            <div class="pm-grade">
              <input type="color" v-model="theme.gradFrom" @change="theme.persist()" aria-label="渐变起始色" />
              <input type="color" v-model="theme.gradMid" @change="theme.persist()" aria-label="渐变中段色" />
              <input type="color" v-model="theme.gradTo" @change="theme.persist()" aria-label="渐变结束色" />
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
  </teleport>
</template>
