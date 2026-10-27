<script setup lang="ts">
/**
 * 应用内取色器（替代 `<input type="color">` 的系统/浏览器弹窗）
 *
 * 为什么自己做一个：系统取色弹窗的「历史颜色」由系统/Chromium 自己维护，
 * 选得越多越长、既不能限制数量也不能删除（用户实测会堆到一大排）。
 * 这里把历史收成 5 格（最近使用在前），悬停色块右上角出现 × 可删。
 *
 * 结构对齐系统弹窗的直觉：上方色板点选 → 下方「自定义」放最近用过的颜色 →
 * 右下角 取消 / 选择。点「选择」才提交，避免误点色板直接改主题。
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useThemeStore } from "../../stores/theme";
import { normalizeHex, rgbToHex, hslToRgb } from "../../utils/color";

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

/* ---------------- 色板（8 列 × 5 行，按色相分列、按明度分行） ---------------- */
const PALETTE: string[] = (() => {
  // 与系统取色弹窗相同的心智：一列一个色相，从上到下由浅到深；最后一列为灰阶。
  const columns = [
    { h: 210, s: 0.62 },
    { h: 145, s: 0.62 },
    { h: 50, s: 0.78 },
    { h: 25, s: 0.82 },
    { h: 0, s: 0.76 },
    { h: 300, s: 0.52 },
    { h: 30, s: 0.34 },
    { h: 0, s: 0 },
  ];
  const lights = [0.82, 0.69, 0.56, 0.42, 0.27];
  const out: string[] = [];
  for (const l of lights) {
    for (const c of columns) out.push(rgbToHex(hslToRgb(c.h, c.s, l)));
  }
  return out;
})();

/* ---------------- 草稿与提交 ---------------- */
const draft = ref(normalizeHex(props.modelValue));

watch(
  () => props.visible,
  (v) => {
    if (!v) return;
    draft.value = normalizeHex(props.modelValue);
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

        <div class="cp-grid" role="group" aria-label="预设颜色">
          <button
            v-for="c in PALETTE"
            :key="c"
            type="button"
            class="cp-swatch"
            :class="{ 'on': draft === c }"
            :style="{ background: c }"
            :title="c"
            :aria-label="`选择颜色 ${c}`"
            @click="pick(c)"
            @dblclick="onSwatchDblClick(c)"
          >
            <span v-if="draft === c" class="cp-check" aria-hidden="true">✓</span>
          </button>
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
.cp-grid {
  display: grid;
  grid-template-columns: repeat(8, 1fr);
  gap: 4px;
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
