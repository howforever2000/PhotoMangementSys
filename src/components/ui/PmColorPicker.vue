<script setup lang="ts">
/**
 * 应用内取色器（替代 `<input type="color">` 的系统/浏览器弹窗）
 *
 * 为什么自己做一个：系统取色弹窗的「历史颜色」由系统/Chromium 自己维护，
 * 选得越多越长、既不能限制数量也不能删除（用户实测会堆到一大排）。
 * 这里把历史收成 5 格（最近使用在前），悬停色块右上角出现 × 可删。
 *
 * FEAT-118：**删掉 64 格预设色块，改成色盘**。
 * 原来那套「8 列色相 × 5 行明度」是从系统弹窗抄来的心智，但它有两个问题：
 *   ① 只有 64 个离散值，想"比这格再亮一点"就只能手输色号；
 *   ② 一屏 64 个色块比界面本身还艳，取色器成了全站最花的地方。
 * 现在只有一个色相环 + 内嵌饱和度/明度方块：任意颜色都是拖出来的，
 * 剩下的离散集合只保留「最近用过」（最多 5 个，可删）。
 *
 * 结构：左上色盘 → 下方最近用过 → 色号输入 → 右下 取消 / 选择。
 * 点「选择」才提交，避免拖动过程中每一帧都改主题。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useThemeStore } from "../../stores/theme";
import { normalizeHex, hexToHsv, hsvToHex, type Hsv } from "../../utils/color";

const props = defineProps<{
  visible: boolean;
  /** 当前生效的颜色（#rrggbb） */
  modelValue: string;
  /** 弹窗标题，缺省「选择颜色」 */
  title?: string;
}>();

const emit = defineEmits<{
  (e: "update:modelValue", value: string): void;
  (e: "close"): void;
}>();

const theme = useThemeStore();

/* ---------------- 最近使用（上限 5） ---------------- */
const RECENT_KEY = "pms.colorPicker.recent";
const MAX_RECENT = 5;

function readRecent(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
    if (!Array.isArray(raw)) return [];
    const out: string[] = [];
    for (const v of raw) {
      if (typeof v !== "string") continue;
      const hex = normalizeHex(v, "");
      if (!hex || out.includes(hex)) continue;
      out.push(hex);
      if (out.length >= MAX_RECENT) break;
    }
    return out;
  } catch {
    return [];
  }
}

const recent = ref<string[]>(readRecent());

function writeRecent() {
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(recent.value));
  } catch {
    /* localStorage 满/被禁用：本次会话仍可用，只是不持久化 */
  }
}

function pushRecent(hex: string) {
  if (!hex) return;
  recent.value = [hex, ...recent.value.filter((c) => c !== hex)].slice(0, MAX_RECENT);
  writeRecent();
}

function removeRecent(hex: string, e: MouseEvent) {
  e.stopPropagation();
  recent.value = recent.value.filter((c) => c !== hex);
  writeRecent();
}

/* ---------------- 草稿与提交 ---------------- */
const draft = ref(normalizeHex(props.modelValue));

watch(
  () => props.visible,
  (v) => {
    if (!v) return;
    draft.value = normalizeHex(props.modelValue);
    hsv.value = hexToHsv(draft.value);
    recent.value = readRecent();
    void nextTick(() => hexInput.value?.focus());
  },
);

const draftValid = computed(() => /^#[0-9a-f]{6}$/.test(draft.value));

function pick(hex: string) {
  draft.value = hex;
}

/** 手工输入色号：合法才同步草稿；非法中间态留在输入框里，失焦时回滚 */
function onHexInput(e: Event) {
  const el = e.target as HTMLInputElement;
  const normalized = normalizeHex(el.value.trim(), "");
  if (normalized) draft.value = normalized;
}

function blurHex(e: Event) {
  const el = e.target as HTMLInputElement;
  el.value = draft.value;
}

function confirm() {
  if (!draftValid.value) return;
  pushRecent(draft.value);
  emit("update:modelValue", draft.value);
  emit("close");
}

/** 色板双击 = 直接选择，省一次点击 */
function onSwatchDblClick(hex: string) {
  draft.value = hex;
  confirm();
}

/* ══════════════════ 色盘（色相环 + 内嵌饱和度/明度方块）══════════════════
   两套坐标必须分开算：
     环：色相是**角度**（把指针位置换算成 atan2 的极角）
     块：饱和度是**横轴**、明度是**纵轴**（明度向上增长，与直觉一致）
   拖动过程只改 draft（hex），不放每帧 emit —— 主题重算是一次全站 CSS 变量下发，
   拖色盘时每帧都发会把主线程占满，色盘自己会卡住。 */
const hsv = ref<Hsv>(hexToHsv(normalizeHex(props.modelValue)));

/** 外部（hex 输入 / 最近色 / 打开弹窗）改了 draft → 反向同步色盘 */
watch(draft, (hex) => {
  const cur = hsvToHex(hsv.value);
  if (cur === hex) return;
  hsv.value = hexToHsv(hex);
});

const ringEl = ref<HTMLElement | null>(null);
const svEl = ref<HTMLElement | null>(null);

/** 从指针位置取归一化坐标（useX 越界夹到 0~1） */
function pointIn(el: HTMLElement, e: PointerEvent) {
  const r = el.getBoundingClientRect();
  const x = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
  const y = Math.min(1, Math.max(0, (e.clientY - r.top) / r.height));
  return { x, y };
}

function dragRing(e: PointerEvent, el: HTMLElement | null = ringEl.value) {
  if (!el) return;
  const { x, y } = pointIn(el, e);
  // atan2 的 0° 指向右侧；色相环的 0°（红）放在顶部，所以 +90°
  const deg = (Math.atan2(y - 0.5, x - 0.5) * 180) / Math.PI + 90;
  hsv.value = { ...hsv.value, h: ((deg % 360) + 360) % 360 };
  draft.value = hsvToHex(hsv.value);
}

function dragSv(e: PointerEvent, el: HTMLElement | null = svEl.value) {
  if (!el) return;
  const { x, y } = pointIn(el, e);
  hsv.value = { ...hsv.value, s: x, v: 1 - y };
  draft.value = hsvToHex(hsv.value);
}

/** 指针按下即生效，并挂一个只在拖动期间存在的 move 监听（松手就摘） */
function startDrag(kind: "ring" | "sv", e: PointerEvent) {
  e.preventDefault();
  const el = kind === "ring" ? ringEl.value : svEl.value;
  if (!el) return;
  (kind === "ring" ? dragRing : dragSv)(e, el);
  const move = (ev: PointerEvent) => (kind === "ring" ? dragRing : dragSv)(ev, el);
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
}

/** 色盘顶部的"当前色"圆点在环上的角度与在方块里的坐标 */
const ringKnob = computed(() => {
  const rad = ((hsv.value.h - 90) * Math.PI) / 180;
  return {
    left: `${50 + Math.cos(rad) * 50}%`,
    top: `${50 + Math.sin(rad) * 50}%`,
  };
});
const svKnob = computed(() => ({
  left: `${hsv.value.s * 100}%`,
  top: `${(1 - hsv.value.v) * 100}%`,
}));
/** 内嵌方块：左到右白→纯色相，下到上透明→黑 */
const svBg = computed(() => {
  const pure = hsvToHex({ h: hsv.value.h, s: 1, v: 1 });
  return {
    background: `linear-gradient(to top, #000, rgba(0,0,0,0)), linear-gradient(to right, #fff, ${pure})`,
  };
});

const hexInput = ref<HTMLInputElement | null>(null);

/* ---------------- Esc（必须拦住，否则会连着把主题弹窗一起关掉） ---------------- */
function onKey(e: KeyboardEvent) {
  if (!props.visible) return;
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
  <Teleport to="body">
    <div v-if="visible" class="cp-mask" @click.self="emit('close')">
      <div
        class="cp-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="cp-title"
        :style="theme.dialogVarStyle"
      >
        <div class="cp-head">
          <h3 id="cp-title">{{ title || "选择颜色" }}</h3>
          <button class="cp-x" type="button" aria-label="关闭" @click="emit('close')">×</button>
        </div>

        <!-- 色盘（FEAT-118 替代 64 格色块）：
             外环 = 色相，内方块 = 饱和度 × 明度。两块都能拖，指针按下即生效。 -->
        <div class="cp-wheel-wrap">
          <div
            ref="ringEl"
            class="cp-ring"
            role="slider"
            tabindex="0"
            aria-label="色相"
            aria-valuemin="0"
            aria-valuemax="360"
            :aria-valuenow="Math.round(hsv.h)"
            @pointerdown="startDrag('ring', $event)"
            @keydown.left.prevent="hsv = { ...hsv, h: (hsv.h + 355) % 360 }; draft = hsvToHex(hsv)"
            @keydown.right.prevent="hsv = { ...hsv, h: (hsv.h + 5) % 360 }; draft = hsvToHex(hsv)"
          >
            <span class="cp-ring-knob" :style="ringKnob"></span>
          </div>
          <div
            ref="svEl"
            class="cp-sv"
            role="slider"
            tabindex="0"
            aria-label="饱和度与明度"
            :style="svBg"
            @pointerdown="startDrag('sv', $event)"
            @keydown.left.prevent="hsv = { ...hsv, s: Math.max(0, hsv.s - 0.02) }; draft = hsvToHex(hsv)"
            @keydown.right.prevent="hsv = { ...hsv, s: Math.min(1, hsv.s + 0.02) }; draft = hsvToHex(hsv)"
            @keydown.up.prevent="hsv = { ...hsv, v: Math.min(1, hsv.v + 0.02) }; draft = hsvToHex(hsv)"
            @keydown.down.prevent="hsv = { ...hsv, v: Math.max(0, hsv.v - 0.02) }; draft = hsvToHex(hsv)"
          >
            <span class="cp-sv-knob" :style="svKnob"></span>
          </div>
        </div>

        <div class="cp-recent-head">
          <span class="cp-recent-title">自定义</span>
          <span class="cp-recent-hint">最近使用，最多 {{ MAX_RECENT }} 个 · 悬停色块可删除</span>
        </div>

        <div class="cp-recent-row">
          <div v-for="c in recent" :key="c" class="cp-recent-item">
            <button
              type="button"
              class="cp-swatch"
              :class="{ on: draft === c }"
              :style="{ background: c }"
              :title="c"
              :aria-label="`选择最近使用的颜色 ${c}`"
              @click="pick(c)"
              @dblclick="onSwatchDblClick(c)"
            >
              <span v-if="draft === c" class="cp-check" aria-hidden="true">✓</span>
            </button>
            <button
              type="button"
              class="cp-del"
              :aria-label="`删除最近使用的颜色 ${c}`"
              title="删除该颜色"
              @click="removeRecent(c, $event)"
            >
              ×
            </button>
          </div>

          <button
            type="button"
            class="cp-add"
            title="把当前颜色加入最近使用"
            aria-label="把当前颜色加入最近使用"
            @click="pushRecent(draft)"
          >
            +
          </button>

          <p v-if="!recent.length" class="cp-empty">选过的颜色会出现在这里（最多 5 个）</p>
        </div>

        <div class="cp-hex-row">
          <span class="cp-preview" :style="{ background: draft }" aria-hidden="true"></span>
          <input
            ref="hexInput"
            class="cp-hex pm-mono"
            type="text"
            :value="draft"
            maxlength="7"
            spellcheck="false"
            aria-label="颜色色号，如 #ffffff"
            @input="onHexInput"
            @blur="blurHex"
            @keydown.enter.prevent="confirm"
          />
          <span class="cp-hint-inline">支持 #rgb / #rrggbb</span>
        </div>

        <div class="cp-actions">
          <button class="pm-btn" type="button" @click="emit('close')">取消</button>
          <button class="pm-btn pm-btn-primary" type="button" :disabled="!draftValid" @click="confirm">
            选择
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.cp-mask {
  position: fixed;
  inset: 0;
  z-index: 1010; /* 必须高于 .pm-modal(1000)：它常叠在主题弹窗之上 */
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
  background: rgba(10, 14, 28, 0.38);
  backdrop-filter: blur(3px);
}
.cp-dialog {
  width: min(520px, 100%);
  max-height: 88vh;
  overflow-y: auto;
  padding: 20px 22px 18px;
  border-radius: 18px;
}
.cp-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 14px;
}
.cp-head h3 {
  margin: 0;
  font-size: 17px;
  font-weight: 600;
  color: var(--pm-text);
}
.cp-x {
  width: 28px;
  height: 28px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--pm-hint);
  font-size: 20px;
  line-height: 1;
  cursor: pointer;
}
.cp-x:hover {
  background: var(--pm-btn-hover);
  color: var(--pm-text);
}
/* ── 色盘 ─────────────────────────────────────────────────────────────
   环用 conic-gradient + radial mask 挖空中心（比 SVG 环省事，且天然平滑）；
   内方块叠两层线性渐变。两者都不吃 backdrop-filter，拖动不重排。 */
.cp-wheel-wrap {
  position: relative;
  /* FEAT-118 反馈：色盘是这块弹窗的唯一操作对象，做小了拖动精度也差 ——
     264px 是弹窗内容宽（440 - 2×24 内边距 = 392）下留足呼吸的尺寸。
     环宽、内方块、两个滑块全部用百分比，改这一个数字就等比例放大。 */
  width: 264px;
  height: 264px;
  margin: 2px auto 4px;
  flex-shrink: 0;
}
.cp-ring {
  position: absolute;
  inset: 0;
  border-radius: 50%;
  cursor: crosshair;
  touch-action: none;
  /* from 90deg：让 0°（红）落在顶部，与指针换算的 +90° 偏移对齐 */
  background: conic-gradient(
    from 90deg,
    #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000
  );
  /* 挖空中心：62% 以内透明，63% 起实心 —— 1% 的过渡边避免锯齿 */
  -webkit-mask-image: radial-gradient(circle at 50% 50%, transparent 66%, #000 67%);
  mask-image: radial-gradient(circle at 50% 50%, transparent 66%, #000 67%);
}
.cp-ring:focus-visible,
.cp-sv:focus-visible {
  outline: 2px solid var(--c-accent, #e8a33d);
  outline-offset: 3px;
}
.cp-ring-knob {
  position: absolute;
  width: 16px;
  height: 16px;
  margin: -8px 0 0 -8px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.45), 0 2px 6px rgba(0, 0, 0, 0.45);
  pointer-events: none;
}
/* 内方块：边长必须 ≤ 环内半径 × √2，否则四个角会钻到环下面。
   实测环内半径 ≈ 122px（264px 的 46%，环带约 10px）⇒ 边长上限 ≈ 172px = 65%。
   取 62%（164px，对角 116px）留 6px 呼吸 —— 再大四角就会压到环上。 */
.cp-sv {
  position: absolute;
  left: 19%;
  top: 19%;
  width: 62%;
  height: 62%;
  border-radius: 10px;
  cursor: crosshair;
  touch-action: none;
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.18), 0 2px 8px rgba(0, 0, 0, 0.28);
}
.cp-sv-knob {
  position: absolute;
  width: 14px;
  height: 14px;
  margin: -7px 0 0 -7px;
  border-radius: 50%;
  background: transparent;
  box-shadow: 0 0 0 2px #fff, inset 0 0 0 1px rgba(0, 0, 0, 0.4);
  pointer-events: none;
}
.cp-swatch {
  position: relative;
  height: 34px;
  border: 1px solid rgba(0, 0, 0, 0.12);
  border-radius: 6px;
  cursor: pointer;
  transition:
    transform var(--dur-2) var(--ease-out),
    box-shadow var(--dur-2) var(--ease-out);
}
.cp-swatch:hover {
  transform: translateY(-1px);
  box-shadow: 0 3px 10px rgba(0, 0, 0, 0.28);
}
.cp-swatch:focus-visible {
  outline: 2px solid var(--color-primary);
  outline-offset: 2px;
}
.cp-swatch.on {
  box-shadow:
    0 0 0 2px var(--pm-text),
    0 0 0 4px var(--color-primary);
}
.cp-check {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #fff;
  font-size: 14px;
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.65);
  pointer-events: none;
}
.cp-recent-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  margin: 18px 0 8px;
}
.cp-recent-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--pm-label);
}
.cp-recent-hint {
  font-size: 11px;
  color: var(--pm-hint);
}
.cp-recent-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 36px;
  flex-wrap: wrap;
}
.cp-recent-item {
  position: relative;
  width: 44px;
  height: 36px;
}
.cp-recent-item .cp-swatch {
  width: 100%;
  height: 100%;
}
/* × 只在悬停/键盘聚焦时出现，平时不干扰色块本身 */
.cp-del {
  position: absolute;
  top: -6px;
  right: -6px;
  width: 16px;
  height: 16px;
  padding: 0;
  border: 1px solid rgba(0, 0, 0, 0.25);
  border-radius: 50%;
  background: #fff;
  color: #444;
  font-size: 11px;
  line-height: 13px;
  cursor: pointer;
  opacity: 0;
  transform: scale(0.82);
  transition:
    opacity var(--dur-2) var(--ease-out),
    transform var(--dur-2) var(--ease-out);
}
.cp-recent-item:hover .cp-del,
.cp-del:focus-visible {
  opacity: 1;
  transform: scale(1);
}
.cp-del:hover {
  background: var(--color-danger, #d9534f);
  border-color: transparent;
  color: #fff;
}
.cp-add {
  width: 44px;
  height: 36px;
  border: 1px dashed var(--pm-soft-border);
  border-radius: 6px;
  background: var(--pm-input-bg);
  color: var(--pm-hint);
  font-size: 20px;
  line-height: 1;
  cursor: pointer;
}
.cp-add:hover {
  border-color: var(--color-primary);
  color: var(--color-primary);
}
.cp-empty {
  margin: 0;
  font-size: 12px;
  color: var(--pm-hint);
}
.cp-hex-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 16px;
}
.cp-preview {
  width: 40px;
  height: 36px;
  flex-shrink: 0;
  border: 1px solid var(--pm-soft-border);
  border-radius: 8px;
}
.cp-hex {
  width: 120px;
  height: 36px;
  padding: 0 10px;
  background: var(--pm-input-bg);
  border: 1px solid var(--pm-input-border);
  border-radius: 8px;
  color: var(--pm-text);
  outline: none;
}
.cp-hex:focus {
  border-color: var(--color-primary);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-primary) 25%, transparent);
}
.cp-hint-inline {
  font-size: 11px;
  color: var(--pm-hint);
}
.cp-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 18px;
}
.pm-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
