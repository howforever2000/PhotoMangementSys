<script setup lang="ts">
/**
 * 分类管理（原子组件，v5 语义分类）—— 智能相册「内容分类」页的 ⚙ 入口
 *
 * 主从布局（左列表 / 右编辑），自包含：
 *   - 左侧：分类列表（规则分类 / 内置预设 / 我的分类）+「＋ 新建分类」
 *   - 右侧：编辑器 —— 名称、图标、关键词 chips、排除词、匹配强度阈值滑块
 *           + **实时预览**（命中数 / 分数分布 / 样张缩略图，改词或拖滑块自动刷新，不落库）
 *   - 底部动作：保存（保存即自动重建该分类命中）、重建该分类、删除
 *
 * 规则分类（source=builtin，人物/扫街/夜景/文档）由 YOLO 检测 / 影调 / OCR 产出，
 * 不参与语义匹配：只允许改名/改图标/启用停用，不能改关键词，也不可删除。
 *
 * 复用：contentStore（list/save/delete/preview/rebuild/thumb 管线）、PhotoLightbox 不需要；
 * 样张用 get_photo_thumbs 批量取（FEAT-044 缓存命中 0 IO）。
 */
import { computed, ref, watch } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";
import { useContentStore } from "../stores/content";
import type { CategoryInput, CategoryOverview, CategoryPreview } from "../types/content";
import {
  MATCH_DEFAULT,
  MATCH_HELP,
  MATCH_MAX,
  MATCH_PRESETS,
  matchToRel,
  relToMatch,
} from "../utils/matchScore";

const props = defineProps<{ categories: CategoryOverview[] }>();
const emit = defineEmits<{ (e: "close"): void; (e: "changed"): void }>();

const theme = useThemeStore();
const notify = useNotify();
const contentStore = useContentStore();

/** 常用图标（点选即填入，避免用户敲 emoji） */
const ICONS = ["🏷️", "🐾", "🐱", "🐶", "🍜", "🌸", "🏞️", "☁️", "🏛️", "🚗", "🌅", "❄️", "🌊", "🌃", "📱", "🥂", "🧒", "🏃", "🎂", "✈️"];

/* -------------------- 编辑态 -------------------- */
/** 当前编辑的分类 id（null = 新建草稿） */
const editingId = ref<number | null>(null);
const form = ref<CategoryInput>(emptyForm());
const isBuiltin = computed(
  () => props.categories.find((c) => c.id === editingId.value)?.source === "builtin",
);
/** 关键词输入框 */
const kwInput = ref("");
const exInput = ref("");
/** 保存中 / 预览中 */
const saving = ref(false);
const previewing = ref(false);
const rebuilding = ref(false);
const preview = ref<CategoryPreview | null>(null);
const previewError = ref("");
/** 样张缩略图（path → 本地可显示文件） */
const thumbMap = ref<Record<string, string>>({});

function emptyForm(): CategoryInput {
  return {
    name: "",
    icon: "🏷️",
    keywords: [],
    exclude_keywords: [],
    threshold: matchToRel(MATCH_DEFAULT),
    sort_order: 0,
    enabled: true,
  };
}

/** 档位与换算统一来自 utils/matchScore（唯一真相点） */
const THRESHOLD_PRESETS = MATCH_PRESETS;

/** 滑块绑定值：UI 用 0~100 的「AI 匹配度」，写回表单时换算成内部 rel（后端零迁移） */
const matchValue = computed({
  get: () => relToMatch(form.value.threshold),
  set: (v: number) => {
    form.value.threshold = matchToRel(Number(v));
  },
});

const list = computed(() =>
  [...props.categories].sort((a, b) => {
    const ab = a.source === "builtin" ? 0 : 1;
    const bb = b.source === "builtin" ? 0 : 1;
    if (ab !== bb) return ab - bb;
    return b.count - a.count;
  }),
);

function pick(c: CategoryOverview) {
  editingId.value = c.id;
  form.value = {
    name: c.name,
    icon: c.icon,
    keywords: [...c.keywords],
    exclude_keywords: [...(c.exclude_keywords ?? [])],
    threshold: c.threshold,
    sort_order: 0,
    enabled: c.enabled,
  };
  kwInput.value = "";
  exInput.value = "";
  void refreshPreview();
}

function fileUrl(p: string): string {
  return p ? convertFileSrc(p) : "";
}

function newCategory() {
  editingId.value = null;
  form.value = emptyForm();
  kwInput.value = "";
  exInput.value = "";
  preview.value = null;
  previewError.value = "";
}

/* -------------------- 关键词编辑 -------------------- */
function addKeyword() {
  const v = kwInput.value.trim();
  if (!v) return;
  if (form.value.keywords.includes(v)) {
    kwInput.value = "";
    return;
  }
  if (form.value.keywords.length >= 30) {
    notify.warning("关键词已达上限", "单个分类最多 30 个关键词");
    return;
  }
  form.value.keywords.push(v);
  kwInput.value = "";
}
function removeKeyword(k: string) {
  form.value.keywords = form.value.keywords.filter((x) => x !== k);
}
function addExclude() {
  const v = exInput.value.trim();
  if (!v || form.value.exclude_keywords.includes(v)) {
    exInput.value = "";
    return;
  }
  form.value.exclude_keywords.push(v);
  exInput.value = "";
}
function removeExclude(k: string) {
  form.value.exclude_keywords = form.value.exclude_keywords.filter((x) => x !== k);
}

/* -------------------- 实时预览（防抖，不落库） -------------------- */
let previewTimer: number | undefined;

function schedulePreview() {
  if (isBuiltin.value) return; // 规则分类不走语义匹配
  window.clearTimeout(previewTimer);
  previewTimer = window.setTimeout(() => void refreshPreview(), 400);
}

async function refreshPreview() {
  if (isBuiltin.value) {
    preview.value = null;
    return;
  }
  if (!form.value.keywords.length) {
    preview.value = null;
    previewError.value = "";
    return;
  }
  previewing.value = true;
  previewError.value = "";
  try {
    preview.value = await contentStore.previewCategory(form.value);
    await loadSampleThumbs();
  } catch (e) {
    preview.value = null;
    previewError.value = String(e);
  } finally {
    previewing.value = false;
  }
}

/** 样张缩略图（按相册分组批量取，复用 FEAT-044 缓存管线） */
async function loadSampleThumbs() {
  const samples = preview.value?.samples ?? [];
  const byAlbum = new Map<number, string[]>();
  for (const s of samples) {
    if (thumbMap.value[s.path]) continue;
    const aid = s.album_id ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    byAlbum.get(aid)!.push(s.path);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, paths]) => {
      try {
        const pairs = await invoke<[string, string][]>("get_photo_thumbs", { albumId: aid, paths });
        for (const [path, thumb] of pairs) if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
      } catch {
        /* 缺图不阻塞：样张回退占位 */
      }
    }),
  );
}

watch(
  () => [form.value.keywords.join("|"), form.value.exclude_keywords.join("|"), form.value.threshold],
  () => schedulePreview(),
);
/* -------------------- 保存 / 删除 / 重建 -------------------- */
async function save() {
  if (!form.value.name.trim()) {
    notify.warning("请填写分类名称", "");
    return;
  }
  if (!isBuiltin.value && !form.value.keywords.length) {
    notify.warning("请至少写一个关键词", "语义分类靠关键词匹配照片，例如「一只猫」「雪山」");
    return;
  }
  saving.value = true;
  try {
    const row = await contentStore.saveCategory(editingId.value, form.value);
    notify.success(
      editingId.value ? "分类已更新" : "分类已创建",
      `${row.name} · 当前命中 ${row.count} 张`,
    );
    editingId.value = row.id;
    emit("changed");
  } catch (e) {
    notify.error("保存失败", String(e));
  } finally {
    saving.value = false;
  }
}

async function remove() {
  if (editingId.value == null || isBuiltin.value) return;
  const target = props.categories.find((c) => c.id === editingId.value);
  if (!window.confirm(`删除分类「${target?.name ?? ""}」？已命中的照片不会被删除。`)) return;
  try {
    await contentStore.deleteCategory(editingId.value);
    notify.success("分类已删除", "");
    newCategory();
    emit("changed");
  } catch (e) {
    notify.error("删除失败", String(e));
  }
}

async function rebuild() {
  if (editingId.value == null) return;
  rebuilding.value = true;
  try {
    const rep = await contentStore.rebuildCategories(editingId.value);
    if (rep.empty_index) {
      notify.warning("还没有语义索引", "请先到「扫描中心」执行含「语义向量」的扫描");
    } else {
      notify.success("该分类已重建", `命中 ${rep.hits} 张 · ${rep.ms} ms`);
    }
    await refreshPreview();
    emit("changed");
  } catch (e) {
    notify.error("重建失败", String(e));
  } finally {
    rebuilding.value = false;
  }
}

/** 内部 rel → 展示用匹配度整数（0~100） */
function fmtScore(v: number): string {
  return String(relToMatch(v));
}

/** 高级用户对照：展示对应的原始净增益值 */
function rawScore(v: number): string {
  return v.toFixed(3);
}
</script>

<template>
  <Teleport to="body">
    <div class="cm-mask" @click.self="emit('close')">
      <div class="cm-dialog" :style="theme.cardStyle">
        <!-- 头部 -->
        <header class="cm-head">
          <h3>⚙ 分类管理</h3>
          <span class="cm-sub">规则分类由人物检测 / 影调 / OCR 产出；预设与自建分类由语义关键词匹配</span>
          <button class="cm-x" title="关闭" @click="emit('close')">✕</button>
        </header>

        <div class="cm-body">
          <!-- 左：分类列表 -->
          <aside class="cm-list">
            <button class="cm-add" @click="newCategory">＋ 新建分类</button>
            <button
              v-for="c in list"
              :key="c.id"
              class="cm-item"
              :class="{ on: c.id === editingId }"
              @click="pick(c)"
            >
              <span class="cm-item-icon">{{ c.icon || "🏷️" }}</span>
              <span class="cm-item-name">{{ c.name }}</span>
              <span class="cm-item-count">{{ c.count }}</span>
              <span v-if="c.source === 'builtin'" class="cm-item-tag">规则</span>
            </button>
          </aside>

          <!-- 右：编辑器 -->
          <section class="cm-edit">
            <div class="cm-row">
              <label class="cm-label">名称</label>
              <input v-model="form.name" class="cm-input" maxlength="20" placeholder="如：我家的猫 / 婚礼 / 滑雪" />
              <input v-model="form.icon" class="cm-icon" maxlength="4" title="图标（emoji）" />
            </div>
            <div class="cm-icons">
              <button
                v-for="i in ICONS"
                :key="i"
                class="cm-ico"
                :class="{ on: form.icon === i }"
                @click="form.icon = i"
              >
                {{ i }}
              </button>
            </div>

            <template v-if="!isBuiltin">
              <!-- 关键词 -->
              <div class="cm-row cm-row-top">
                <label class="cm-label">关键词</label>
                <div class="cm-chips">
                  <span v-for="k in form.keywords" :key="k" class="cm-chip">
                    {{ k }}<button class="cm-chip-x" @click="removeKeyword(k)">✕</button>
                  </span>
                  <input
                    v-model="kwInput"
                    class="cm-chip-input"
                    placeholder="输入短语后回车，如「雪山」「一只猫」"
                    @keydown.enter.prevent="addKeyword"
                    @blur="addKeyword"
                  />
                </div>
              </div>

              <!-- 排除词 -->
              <div class="cm-row cm-row-top">
                <label class="cm-label">排除词</label>
                <div class="cm-chips">
                  <span v-for="k in form.exclude_keywords" :key="k" class="cm-chip ex">
                    {{ k }}<button class="cm-chip-x" @click="removeExclude(k)">✕</button>
                  </span>
                  <input
                    v-model="exInput"
                    class="cm-chip-input"
                    placeholder="可选：压掉误召回（如「热狗」）"
                    @keydown.enter.prevent="addExclude"
                    @blur="addExclude"
                  />
                </div>
              </div>

              <!-- AI 匹配度（0~100，用户视角；内部仍存 rel，零迁移） -->
              <div class="cm-row cm-row-top">
                <label class="cm-label">AI 匹配度</label>
                <div class="cm-slider-wrap">
                  <input
                    v-model.number="matchValue"
                    class="cm-slider"
                    type="range"
                    min="0"
                    :max="MATCH_MAX"
                    step="5"
                    :title="`等价于原始净增益 ${rawScore(form.threshold)}`"
                  />
                  <span class="cm-thr">{{ matchValue }}</span>
                  <button
                    v-for="p in THRESHOLD_PRESETS"
                    :key="p.label"
                    class="cm-preset"
                    :class="{ on: matchValue === p.value }"
                    :title="p.hint"
                    @click="matchValue = p.value"
                  >
                    {{ p.label }} {{ p.value }}
                  </button>
                </div>
              </div>
              <p class="cm-hint">
                {{ MATCH_HELP }}<br />
                预览里的「分布」与每张样张括号里的数字都用同一刻度，便于对照着调。
                <span class="cm-dim">（高级：原始净增益 {{ rawScore(form.threshold) }}）</span>
              </p>
            </template>

            <p v-else class="cm-hint">
              这是由识别通道产出的规则分类（人物/扫街/夜景/文档），不参与语义关键词匹配，
              只能改名 / 换图标 / 启用停用。想按自己的标准分类，请「＋ 新建分类」。
            </p>

            <!-- 预览 -->
            <div v-if="!isBuiltin" class="cm-preview">
              <div class="cm-preview-head">
                <b>实时预览</b>
                <span v-if="previewing" class="cm-dim">计算中…</span>
                <template v-else-if="preview">
                  <span class="cm-hit">命中 <b>{{ preview.count }}</b> / {{ preview.total }} 张</span>
                  <span class="cm-dim">
                    命中分数分布：p50 {{ fmtScore(preview.p50) }} · p90 {{ fmtScore(preview.p90) }} ·
                    p99 {{ fmtScore(preview.p99) }} · 最高 {{ fmtScore(preview.max) }}
                    （与左侧「AI 匹配度」同一刻度）
                  </span>
                </template>
                <span v-else-if="previewError" class="cm-err">{{ previewError }}</span>
                <span v-else class="cm-dim">添加关键词后自动预览</span>
              </div>
              <div v-if="preview?.samples.length" class="cm-samples">
                <figure v-for="s in preview.samples" :key="s.photo_hash" class="cm-sample" :title="`命中「${s.matched_keyword}」· AI 匹配度 ${fmtScore(s.score)}`">
                  <img v-if="thumbMap[s.path]" :src="fileUrl(thumbMap[s.path])" loading="lazy" alt="" />
                  <span v-else class="cm-sample-ph">🖼</span>
                  <figcaption>{{ s.matched_keyword }}</figcaption>
                </figure>
              </div>
              <p v-else-if="preview && !preview.count" class="cm-hint">
                当前「AI 匹配度」下没有命中。可换更具体的关键词（如「一只猫」而不是「猫」），或把匹配度调低。
              </p>
            </div>

            <!-- 动作 -->
            <footer class="cm-actions">
              <button v-if="editingId != null && !isBuiltin" class="cm-btn danger" @click="remove">删除分类</button>
              <button
                v-if="editingId != null"
                class="cm-btn"
                :disabled="rebuilding"
                @click="rebuild"
              >
                {{ rebuilding ? "重建中…" : "🔄 重建该分类" }}
              </button>
              <span class="cm-flex"></span>
              <button class="cm-btn" @click="emit('close')">关闭</button>
              <button class="cm-btn primary" :disabled="saving" @click="save">
                {{ saving ? "保存中…" : editingId == null ? "创建分类" : "保存修改" }}
              </button>
            </footer>
          </section>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.cm-mask {
  position: fixed;
  inset: 0;
  z-index: 1080;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}
.cm-dialog {
  width: min(1040px, 96vw);
  height: min(680px, 92vh);
  border-radius: 16px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 20px 50px rgba(0, 0, 0, 0.32);
}
.cm-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 14px 18px;
  border-bottom: 1px solid rgba(127, 127, 127, 0.2);
}
.cm-head h3 {
  margin: 0;
  font-size: 16px;
}
.cm-sub {
  font-size: 11.5px;
  opacity: 0.6;
  flex: 1;
}
.cm-x {
  border: none;
  background: transparent;
  color: inherit;
  font-size: 16px;
  cursor: pointer;
  opacity: 0.6;
}
.cm-x:hover { opacity: 1; }

.cm-body {
  flex: 1;
  min-height: 0;
  display: flex;
}
.cm-list {
  width: 236px;
  flex: 0 0 auto;
  border-right: 1px solid rgba(127, 127, 127, 0.2);
  padding: 10px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.cm-add {
  padding: 8px 10px;
  margin-bottom: 6px;
  border-radius: 9px;
  border: 1px dashed rgba(106, 141, 240, 0.7);
  background: transparent;
  color: inherit;
  cursor: pointer;
  font-size: 12.5px;
}
.cm-add:hover { background: rgba(106, 141, 240, 0.1); }
.cm-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 9px;
  border-radius: 9px;
  border: 1px solid transparent;
  background: transparent;
  color: inherit;
  cursor: pointer;
  font-size: 12.5px;
  text-align: left;
}
.cm-item:hover { background: rgba(127, 127, 127, 0.12); }
.cm-item.on {
  background: rgba(106, 141, 240, 0.16);
  border-color: rgba(106, 141, 240, 0.6);
}
.cm-item-icon { flex: 0 0 auto; }
.cm-item-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cm-item-count {
  font-size: 11px;
  opacity: 0.6;
}
.cm-item-tag {
  font-size: 10px;
  padding: 0 5px;
  border-radius: 999px;
  background: rgba(60, 90, 200, 0.25);
}

.cm-edit {
  flex: 1;
  min-width: 0;
  padding: 16px 18px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.cm-row {
  display: flex;
  align-items: center;
  gap: 10px;
}
.cm-row-top { align-items: flex-start; }
.cm-label {
  flex: 0 0 62px;
  font-size: 12.5px;
  font-weight: 600;
  padding-top: 6px;
}
.cm-input {
  flex: 1;
  padding: 7px 10px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.35);
  background: transparent;
  color: inherit;
  font-size: 13px;
}
.cm-icon {
  width: 52px;
  text-align: center;
  padding: 7px 4px;
  border-radius: 8px;
  border: 1px solid rgba(127, 127, 127, 0.35);
  background: transparent;
  color: inherit;
  font-size: 16px;
}
.cm-icons {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  padding-left: 72px;
}
.cm-ico {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  border: 1px solid transparent;
  background: transparent;
  cursor: pointer;
  font-size: 14px;
}
.cm-ico:hover { background: rgba(127, 127, 127, 0.15); }
.cm-ico.on { border-color: rgba(106, 141, 240, 0.8); background: rgba(106, 141, 240, 0.14); }

.cm-chips {
  flex: 1;
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 4px 0;
}
.cm-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  border-radius: 999px;
  font-size: 12px;
  background: rgba(106, 141, 240, 0.14);
  border: 1px solid rgba(106, 141, 240, 0.4);
}
.cm-chip.ex {
  background: rgba(224, 120, 49, 0.12);
  border-color: rgba(224, 120, 49, 0.4);
}
.cm-chip-x {
  border: none;
  background: transparent;
  color: inherit;
  cursor: pointer;
  opacity: 0.6;
  font-size: 11px;
  padding: 0;
}
.cm-chip-x:hover { opacity: 1; }
.cm-chip-input {
  flex: 1;
  min-width: 200px;
  padding: 5px 8px;
  border-radius: 8px;
  border: 1px dashed rgba(127, 127, 127, 0.4);
  background: transparent;
  color: inherit;
  font-size: 12.5px;
}

.cm-slider-wrap {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.cm-slider { flex: 1; min-width: 180px; }
.cm-thr {
  font-family: ui-monospace, monospace;
  font-size: 12.5px;
  min-width: 42px;
}
.cm-preset {
  padding: 3px 10px;
  border-radius: 999px;
  border: 1px solid rgba(127, 127, 127, 0.35);
  background: transparent;
  color: inherit;
  font-size: 11.5px;
  cursor: pointer;
}
.cm-preset.on {
  border-color: rgba(106, 141, 240, 0.8);
  background: rgba(106, 141, 240, 0.16);
  font-weight: 600;
}

.cm-hint {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.65;
  opacity: 0.68;
}

.cm-preview {
  margin-top: 4px;
  border: 1px solid rgba(127, 127, 127, 0.22);
  border-radius: 12px;
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.cm-preview-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
  font-size: 12.5px;
}
.cm-hit b { color: #396cd8; font-size: 14px; }
.cm-dim { opacity: 0.62; font-size: 11.5px; }
.cm-err { color: #e03131; font-size: 11.5px; }
.cm-samples {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(78px, 1fr));
  gap: 6px;
}
.cm-sample {
  margin: 0;
  position: relative;
  aspect-ratio: 1;
  border-radius: 8px;
  overflow: hidden;
  background: rgba(127, 127, 127, 0.15);
}
.cm-sample img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.cm-sample-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0.5;
}
.cm-sample figcaption {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  font-size: 10px;
  color: #fff;
  padding: 10px 4px 3px;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.65));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.cm-actions {
  margin-top: auto;
  display: flex;
  align-items: center;
  gap: 8px;
  padding-top: 10px;
  border-top: 1px solid rgba(127, 127, 127, 0.2);
}
.cm-flex { flex: 1; }
.cm-btn {
  padding: 7px 14px;
  border-radius: 9px;
  border: 1px solid rgba(127, 127, 127, 0.4);
  background: transparent;
  color: inherit;
  font-size: 12.5px;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
}
.cm-btn:hover:not(:disabled) {
  border-color: rgba(106, 141, 240, 0.75);
  background: rgba(106, 141, 240, 0.08);
}
.cm-btn:disabled { opacity: 0.55; cursor: wait; }
.cm-btn.primary {
  background: #396cd8;
  border-color: #396cd8;
  color: #fff;
}
.cm-btn.primary:hover:not(:disabled) { background: #2f5cc0; }
.cm-btn.danger { color: #e03131; border-color: rgba(224, 49, 49, 0.5); }
</style>
