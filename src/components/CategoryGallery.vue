<script setup lang="ts">
/**
 * 语义分类画廊（v5）—— 智慧相册「内容分类」tab
 *
 * 数据源：`photo_categories` + `photo_category_hits`（分类命中已物化，浏览走 SQL 秒开）
 *   1. 分类 = 系统规则分类（人物/扫街/夜景/文档，由 YOLO 检测 / 影调 / OCR 产出）
 *            + 内置预设 + 用户自建（后两者由 Chinese-CLIP 关键词语义匹配产出）
 *   2. 卡片网格 → 点击进入照片网格；关键词 chips 按「命中的关键词」即时过滤（内存过滤零请求）
 *   3. 顶栏「⚙ 管理分类」→ CategoryManager（新建/改词/调阈值/实时预览/重建）
 *
 * 覆盖度提示：语义分类只能覆盖「已建向量索引」的照片，顶部常驻「已索引 N/M」，
 * 未建索引时给出去扫描中心建索引的入口，避免用户误以为功能坏了。
 *
 * 复用底层：list_categories / list_category_photos / rebuild_categories 命令、
 * PhotoLightbox 大图、ContextMenu、theme 暗色适配、骨架/空/错误三态。
 */
import { computed, onMounted, ref } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";
import { useContentStore } from "../stores/content";
import PhotoLightbox from "./PhotoLightbox.vue";
import ContextMenu, { type ContextMenuEntry } from "./ContextMenu.vue";
import CategoryManager from "./CategoryManager.vue";
import type { CategoryIndexStats, CategoryOverview, CategoryPhoto } from "../types/content";
import { categoryLabel, categoryTone } from "../utils/categoryLabel";
import { relToMatch } from "../utils/matchScore";

const theme = useThemeStore();
const notify = useNotify();
const contentStore = useContentStore();

/* -------------------- 数据 -------------------- */
const loading = ref(true);
const error = ref("");
/** 分类总览（含计数与封面） */
const cats = ref<CategoryOverview[]>([]);
/** 索引覆盖统计 */
const stats = ref<CategoryIndexStats | null>(null);
/** path → 缩略图缓存路径（卡片封面与照片网格共用） */
const thumbMap = ref<Record<string, string>>({});
/** 分类管理对话框 */
const managerOpen = ref(false);
const rebuilding = ref(false);
/** 首次进入自动匹配过一次（避免反复触发） */
let autoBuilt = false;
/** 自动匹配进行中（工具栏提示） */
const building = ref(false);

/* -------------------- 视图 1：分类卡片 -------------------- */
interface CategoryCard {
  id: number;
  name: string;
  icon: string;
  source: string;
  slug: string;
  keywords: string[];
  threshold: number;
  count: number;
  cover: string | null;
  coverAlbumId: number | null;
}

/** 卡片排序：系统规则分类固定在前，其余按命中数降序 */
const cards = computed<CategoryCard[]>(() =>
  cats.value
    .map((c) => ({
      id: c.id,
      name: c.name,
      icon: c.icon,
      source: c.source,
      slug: c.slug,
      keywords: c.keywords,
      threshold: c.threshold,
      count: c.count,
      cover: c.cover_path,
      coverAlbumId: c.cover_album_id,
    }))
    .sort((a, b) => {
      const ab = a.source === "builtin" ? 0 : 1;
      const bb = b.source === "builtin" ? 0 : 1;
      if (ab !== bb) return ab - bb;
      return b.count - a.count;
    }),
);

const totalHits = computed(() => cards.value.reduce((n, c) => n + c.count, 0));

/** 卡片无封面时的类目色底（复用 categoryTone 色卡） */
function toneBg(c: CategoryCard): string {
  return categoryTone(c.slug || "other").bg;
}

/** 语义索引是否为空（分类极可能空 → 优先提示建索引） */
const indexEmpty = computed(() => (stats.value?.indexed ?? 0) === 0);
/** 换过语义档位（旧档向量还在 → 提示重建索引） */
const indexStale = computed(() => (stats.value?.stale ?? 0) > 0);
/** 覆盖度文案 */
const indexText = computed(() => {
  const s = stats.value;
  if (!s) return "";
  return `语义索引 ${s.indexed} / 已入库 ${s.known} 张`;
});

async function load() {
  loading.value = true;
  error.value = "";
  try {
    const [list, st] = await Promise.allSettled([
      contentStore.listCategories(),
      contentStore.categoryIndexStats(),
    ]);
    if (list.status === "fulfilled") cats.value = list.value;
    else error.value = String(list.reason);
    stats.value = st.status === "fulfilled" ? st.value : null;
    // 首次进入（有索引但尚未算过任何语义命中）→ 自动重建一次，开箱即用
    if (!autoBuilt && stats.value && stats.value.indexed > 0 && needFirstBuild()) {
      autoBuilt = true;
      building.value = true;
      try {
        await contentStore.rebuildCategories();
        const fresh = await contentStore.listCategories();
        cats.value = fresh;
      } catch (e) {
        notify.warning("分类自动匹配未完成", `${e}（可点「🔄 重建分类」重试）`);
      } finally {
        building.value = false;
      }
    }
    await loadCoverThumbs();
  } finally {
    loading.value = false;
  }
}

/** 语义分类全部为 0 命中 → 视为尚未算过（builtin 规则分类不计入） */
function needFirstBuild(): boolean {
  const sem = cats.value.filter((c) => c.source !== "builtin");
  return sem.length > 0 && sem.every((c) => c.count === 0);
}

/** 封面缩略图批量懒加载（按相册分组，复用真实相册缓存命名） */
async function loadCoverThumbs() {
  const byAlbum = new Map<number, string[]>();
  for (const t of cards.value) {
    if (!t.cover || thumbMap.value[t.cover]) continue;
    const aid = t.coverAlbumId ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    byAlbum.get(aid)!.push(t.cover);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, paths]) => {
      try {
        const pairs = await invoke<[string, string][]>("get_photo_thumbs", { albumId: aid, paths });
        for (const [path, thumb] of pairs) if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
      } catch {
        /* 封面缺图不阻塞：回退类目色底 */
      }
    }),
  );
}

/* -------------------- 视图 2：分类照片网格 + 关键词 chips -------------------- */
const activeCard = ref<CategoryCard | null>(null);
const activeKw = ref<string | null>(null);
const photos = ref<CategoryPhoto[]>([]);
const photosLoading = ref(false);

/** 关键词 chips（按「实际命中的关键词」统计，即时过滤零请求） */
const kwChips = computed(() => {
  if (!activeCard.value) return [];
  const map = new Map<string, number>();
  for (const p of photos.value) {
    const k = p.matched_keyword || "";
    if (!k) continue;
    map.set(k, (map.get(k) ?? 0) + 1);
  }
  if (map.size <= 1) return [];
  const chips = [...map.entries()].map(([key, count]) => ({
    key,
    label: kwLabel(activeCard.value!, key),
    count,
  }));
  chips.sort((a, b) => b.count - a.count);
  return [{ key: "", label: "全部", count: photos.value.length }, ...chips];
});

/** 关键词中文展示：规则分类的 matched_keyword 是类别 key，需要映射 */
function kwLabel(card: CategoryCard, key: string): string {
  if (card.source === "builtin") return categoryLabel(key);
  return key;
}

/** chips 过滤纯前端内存过滤 */
const filteredPhotos = computed(() => {
  if (!activeKw.value) return photos.value;
  return photos.value.filter((p) => (p.matched_keyword || "") === activeKw.value);
});

/** 命中强度展示：与分类设置里的「AI 匹配度」同一刻度（换算见 utils/matchScore） */
function strengthLabel(score: number): string {
  return String(relToMatch(score));
}

async function openCategory(t: CategoryCard) {
  activeCard.value = t;
  activeKw.value = null;
  photosLoading.value = true;
  try {
    photos.value = await invoke<CategoryPhoto[]>("list_category_photos", { id: t.id, limit: null });
    await loadGridThumbs();
  } catch (e) {
    notify.error("加载分类照片失败", String(e));
    photos.value = [];
  } finally {
    photosLoading.value = false;
  }
}

async function loadGridThumbs() {
  const byAlbum = new Map<number, string[]>();
  for (const r of photos.value) {
    if (thumbMap.value[r.path]) continue;
    const aid = r.album_id ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    byAlbum.get(aid)!.push(r.path);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, paths]) => {
      try {
        const pairs = await invoke<[string, string][]>("get_photo_thumbs", { albumId: aid, paths });
        for (const [path, thumb] of pairs) if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
      } catch {
        /* 单相册失败不阻塞：缺图卡片回退占位 */
      }
    }),
  );
}

function backToCards() {
  activeCard.value = null;
  activeKw.value = null;
  photos.value = [];
}

function selectKw(key: string) {
  activeKw.value = key || null;
}

/* -------------------- 重建 / 管理 -------------------- */
async function rebuildAll() {
  rebuilding.value = true;
  try {
    const rep = await contentStore.rebuildCategories();
    if (rep.empty_index) {
      notify.warning(
        "还没有语义索引",
        "请先到「扫描中心」对相册执行一次含「语义向量」的扫描，分类才能匹配出照片",
      );
    } else {
      notify.success(
        "分类已重建",
        `${rep.categories} 个分类 · 命中 ${rep.hits} 张 · 耗时 ${rep.ms} ms`,
      );
    }
    await load();
  } catch (e) {
    notify.error("重建失败", String(e));
  } finally {
    rebuilding.value = false;
  }
}

/** 分类管理对话框保存/删除后回调：刷新卡片与统计 */
async function onManagerChanged() {
  await load();
}

/* -------------------- 大图看图器 -------------------- */
const lightboxOpen = ref(false);
const lightboxIndex = ref(0);
const lightboxPhotos = computed(() =>
  filteredPhotos.value.map((p) => ({ path: p.path, albumId: p.album_id })),
);
function openLightbox(p: CategoryPhoto) {
  const idx = filteredPhotos.value.findIndex((x) => x.photo_hash === p.photo_hash);
  if (idx < 0) return;
  lightboxIndex.value = idx;
  lightboxOpen.value = true;
}

/* -------------------- FEAT-050：删除（记录删除 / 回收站磁盘删除） -------------------- */
type DeleteMode = "records" | "trash";

/** 批量管理模式 */
const selectMode = ref(false);
const selected = ref<Set<string>>(new Set());
/** 删除方式选择弹窗（两种选择即最终确认，点击方式立即执行） */
const modeDialogPaths = ref<string[] | null>(null);

/** 右键菜单 */
const ctxVisible = ref(false);
const ctxX = ref(0);
const ctxY = ref(0);
const ctxPath = ref("");
const ctxItems = computed<ContextMenuEntry[]>(() => [
  {
    label: "本地记录删除（保留文件）",
    icon: "📄",
    danger: true,
    onClick: () => executeDelete([ctxPath.value], "records"),
  },
  {
    label: "磁盘删除（移入回收站）",
    icon: "🗑",
    danger: true,
    onClick: () => executeDelete([ctxPath.value], "trash"),
  },
]);

function onCellContextMenu(e: MouseEvent, path: string) {
  if (selectMode.value) return;
  ctxX.value = e.clientX;
  ctxY.value = e.clientY;
  ctxPath.value = path;
  ctxVisible.value = true;
}

function toggleSelectMode() {
  selectMode.value = !selectMode.value;
  if (!selectMode.value) selected.value = new Set();
}
function toggleSelect(path: string) {
  const next = new Set(selected.value);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  selected.value = next;
}
function selectAll() {
  selected.value = new Set(filteredPhotos.value.map((p) => p.path));
}
function clearSelection() {
  selected.value = new Set();
}

/** 预览删除 / 批量删除：先选方式 */
function openModeDialog(paths: string[]) {
  if (!paths.length) return;
  modeDialogPaths.value = paths;
}
function pickMode(mode: DeleteMode) {
  const paths = modeDialogPaths.value ?? [];
  modeDialogPaths.value = null;
  void executeDelete(paths, mode);
}

async function executeDelete(paths: string[], mode: DeleteMode) {
  if (!paths.length) return;
  const cmd = mode === "records" ? "delete_photo_records_by_paths" : "delete_photos_to_trash";
  try {
    const outcome = await invoke<{
      requested: number;
      deleted: number;
      failed: number;
      failed_paths: string[];
    }>(cmd, { paths });
    const removed = new Set(paths.filter((p) => !outcome.failed_paths.includes(p)));
    // 同步本地列表与缩略图缓存
    photos.value = photos.value.filter((p) => !removed.has(p.path));
    const tm = { ...thumbMap.value };
    for (const p of removed) delete tm[p];
    thumbMap.value = tm;
    // 预览器内删除：切到下一张（空则关闭）
    if (lightboxOpen.value) {
      if (!filteredPhotos.value.length) lightboxOpen.value = false;
      else if (lightboxIndex.value >= filteredPhotos.value.length)
        lightboxIndex.value = filteredPhotos.value.length - 1;
    }
    // 刷新聚合计数（卡片视图张数同步）
    await refreshCards();
    if (activeCard.value && !cards.value.some((t) => t.id === activeCard.value!.id)) {
      backToCards();
    }
    if (outcome.failed > 0) {
      notify.warning(
        `已删除 ${outcome.deleted} / ${outcome.requested} 张`,
        `失败：${outcome.failed_paths.slice(0, 3).join("、")}${outcome.failed_paths.length > 3 ? "…" : ""}`,
      );
    } else {
      notify.success(
        `已删除 ${outcome.deleted} 张`,
        mode === "trash" ? "文件已移入系统回收站" : "本地文件保留，仅清除记录",
      );
    }
  } catch (e) {
    notify.error("删除失败", String(e));
  } finally {
    selected.value = new Set();
  }
}

/** 聚合计数刷新（删除后调用；失败不阻塞） */
async function refreshCards() {
  try {
    cats.value = await contentStore.listCategories();
    await loadCoverThumbs();
  } catch {
    /* 计数刷新失败不阻塞 */
  }
}

function fileUrl(p: string): string {
  return p ? convertFileSrc(p) : "";
}

onMounted(load);
</script>

<template>
  <div class="cg-wrap" :style="{ color: theme.textColor }">
    <!-- ============ 视图 1：分类卡片 ============ -->
    <template v-if="!activeCard">
      <!-- 工具条：索引覆盖 + 重建 + 管理 -->
      <div class="cg-toolbar">
        <span class="cg-index" :class="{ warn: indexEmpty }">
          {{ indexEmpty ? "尚未建立语义索引（语义分类暂无法匹配）" : indexText }}
        </span>
        <span v-if="indexStale" class="cg-index warn">
          · 有 {{ stats?.stale }} 张向量属于其他模型档位，需重建索引
        </span>
        <span class="cg-spacer"></span>
        <span v-if="building" class="cg-index">首次匹配中…（正在用关键词检索全库）</span>
        <span class="cg-total">共 {{ totalHits }} 张次</span>
        <button class="btn" :disabled="rebuilding" @click="rebuildAll">
          {{ rebuilding ? "重建中…" : "🔄 重建分类" }}
        </button>
        <button class="btn" @click="managerOpen = true">⚙ 管理分类</button>
      </div>

      <!-- 换档 / 建索引的入口指引（语义分类的两个前置：模型档位 + 向量索引） -->
      <p v-if="indexEmpty" class="cg-tip">
        语义分类需要先建立「语义向量索引」：进入 <b>扫描中心 → 相册扫描</b>，勾选「语义向量」执行一次；
        模型档位（B/16 ↔ L/14-336）在 <b>扫描中心 → ⚙ 性能设置 → 语义模型</b> 里切换，换档后需重建索引。
      </p>

      <!-- 加载骨架 -->
      <div v-if="loading" class="cg-state">
        <div class="sk-grid">
          <div v-for="i in 6" :key="i" class="sk-card"></div>
        </div>
      </div>

      <!-- 错误态 -->
      <div v-else-if="error" class="cg-state">
        <div class="cg-state-icon">⚠️</div>
        <p>加载失败：{{ error }}</p>
        <button class="btn" @click="load">重试</button>
      </div>

      <!-- 分类卡片网格 -->
      <div v-else class="cat-grid">
        <article
          v-for="t in cards"
          :key="t.id"
          class="cat-card"
          :style="theme.cardStyle"
          :title="`${t.name} · ${t.count} 张`"
          @click="openCategory(t)"
        >
          <div class="cat-cover" :style="{ background: toneBg(t) }">
            <img v-if="t.cover && thumbMap[t.cover]" :src="fileUrl(thumbMap[t.cover])" loading="lazy" alt="" />
            <span v-else class="cat-cover-ph">{{ t.icon || t.name.slice(0, 1) }}</span>
            <span class="cat-count">{{ t.count }} 张</span>
            <span v-if="t.source === 'builtin'" class="cat-tag">规则</span>
          </div>
          <div class="cat-body">
            <h3 class="cat-name">{{ t.icon }} {{ t.name }}</h3>
            <span v-if="t.keywords.length" class="cat-sub-hint">
              {{ t.keywords.length }} 词 · 匹配度 {{ relToMatch(t.threshold) }}
            </span>
          </div>
        </article>
      </div>

      <!-- 空态：仍展示管理入口（用户可先建分类） -->
      <div v-if="!loading && !error && !cards.length" class="cg-state">
        <div class="cg-state-icon">🏷️</div>
        <p class="cg-state-title">还没有分类</p>
        <p class="cg-state-text">点击「⚙ 管理分类」新建分类并写好关键词，匹配到的照片会自动归入。</p>
      </div>
    </template>

    <!-- ============ 视图 2：分类照片网格 ============ -->
    <template v-else>
      <div class="cg-detail-head">
        <button class="btn" @click="backToCards">← 返回分类</button>
        <h2 class="cg-detail-title">{{ activeCard.icon }} {{ activeCard.name }}</h2>
        <span class="cg-detail-count">{{ activeCard.count }} 张</span>
        <span v-if="activeCard.source === 'builtin'" class="cg-detail-count">· 规则分类（无需语义索引）</span>
        <span class="cg-spacer"></span>
        <button class="btn" :class="{ active: selectMode }" @click="toggleSelectMode">
          {{ selectMode ? "退出批量" : "☑ 批量管理" }}
        </button>
        <template v-if="selectMode">
          <button class="btn" @click="selectAll">全选</button>
          <button class="btn" @click="clearSelection">取消全选</button>
          <button class="btn btn-danger" :disabled="!selected.size" @click="openModeDialog([...selected])">
            🗑 删除选中（{{ selected.size }}）
          </button>
        </template>
      </div>

      <!-- 关键词 chips 即时过滤 -->
      <div v-if="kwChips.length" class="chip-row">
        <button
          v-for="c in kwChips"
          :key="c.key"
          class="chip"
          :class="{ on: (activeKw ?? '') === c.key }"
          @click="selectKw(c.key)"
        >
          {{ c.label }} <span class="chip-n">{{ c.count }}</span>
        </button>
      </div>

      <div v-if="photosLoading" class="cg-state">
        <div class="sk-grid">
          <div v-for="i in 8" :key="i" class="sk-card sk-photo"></div>
        </div>
      </div>
      <div v-else-if="!filteredPhotos.length" class="cg-state">
        <div class="cg-state-icon">🗂</div>
        <p>该分类下暂无照片</p>
        <p class="cg-state-text">
          语义分类只覆盖已建向量索引的照片；可在「扫描中心」执行含「语义向量」的扫描后点「🔄 重建分类」。
        </p>
      </div>
      <div v-else class="photo-grid">
        <figure
          v-for="p in filteredPhotos"
          :key="p.photo_hash"
          class="photo-cell"
          :class="{ selectable: selectMode, checked: selectMode && selected.has(p.path) }"
          :title="[p.matched_keyword, p.shoot_time, p.location].filter(Boolean).join(' · ')"
          @click="selectMode ? toggleSelect(p.path) : openLightbox(p)"
          @contextmenu.prevent="onCellContextMenu($event, p.path)"
        >
          <img v-if="thumbMap[p.path]" :src="fileUrl(thumbMap[p.path])" loading="lazy" alt="" />
          <div v-else class="photo-ph">🖼</div>
          <span v-if="p.score > 0" class="photo-score" :title="`AI 匹配度 ${strengthLabel(p.score)}（越高越像；阈值可在 ⚙ 管理分类 里调）`">
            ✨ {{ strengthLabel(p.score) }}
          </span>
          <figcaption v-if="p.matched_keyword" class="photo-cap">
            {{ kwLabel(activeCard, p.matched_keyword) }}
          </figcaption>
          <span v-if="selectMode" class="cell-check" :class="{ on: selected.has(p.path) }">
            {{ selected.has(p.path) ? "✓" : "" }}
          </span>
        </figure>
      </div>
    </template>

    <!-- 大图看图器（复用；启用删除按钮） -->
    <PhotoLightbox
      v-if="lightboxOpen"
      :photos="lightboxPhotos"
      :index="lightboxIndex"
      deletable
      @close="lightboxOpen = false"
      @delete="openModeDialog([$event])"
    />

    <!-- 右键菜单（复用 FEAT-043 组件；v-if 用完即卸载，避免全屏透明遮罩常驻拦截点击） -->
    <ContextMenu
      v-if="ctxVisible"
      :items="ctxItems"
      :x="ctxX"
      :y="ctxY"
      @close="ctxVisible = false"
    />

    <!-- 分类管理（原子组件：列表 + 编辑 + 实时预览 + 重建） -->
    <CategoryManager
      v-if="managerOpen"
      :categories="cats"
      @close="managerOpen = false"
      @changed="onManagerChanged"
    />

    <!-- 删除方式选择（批量 / 预览删除） -->
    <Teleport to="body">
      <div v-if="modeDialogPaths" class="del-mask" @click.self="modeDialogPaths = null">
        <div class="del-dialog" :style="theme.cardStyle">
          <h4>选择删除方式（{{ modeDialogPaths.length }} 张）</h4>
          <button class="del-opt" @click="pickMode('records')">
            <b>📄 本地记录删除</b>
            <span>清除扫描 / AI 记录与缩略图缓存，本地文件保留</span>
          </button>
          <button class="del-opt" @click="pickMode('trash')">
            <b>🗑 磁盘删除（移入回收站）</b>
            <span>照片移入系统回收站，可找回；记录与缓存同步清除</span>
          </button>
          <button class="btn del-cancel" @click="modeDialogPaths = null">取消</button>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.cg-wrap {
  min-width: 0;
}

/* ---- 工具条 ---- */
.cg-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 12px;
}
.cg-index {
  font-size: 12.5px;
  opacity: 0.7;
}
.cg-index.warn {
  color: #b45309;
  opacity: 1;
}
.cg-total {
  font-size: 12.5px;
  opacity: 0.65;
}
.cg-tip {
  margin: 0 0 12px;
  padding: 8px 12px;
  border-radius: 10px;
  font-size: 12px;
  line-height: 1.7;
  background: rgba(180, 83, 9, 0.1);
  border: 1px solid rgba(180, 83, 9, 0.28);
}
.cat-tag {
  position: absolute;
  left: 8px;
  top: 8px;
  padding: 2px 8px;
  font-size: 10.5px;
  color: #fff;
  background: rgba(60, 90, 200, 0.75);
  border-radius: 999px;
}
.photo-score {
  position: absolute;
  right: 6px;
  top: 6px;
  padding: 1px 7px;
  font-size: 10.5px;
  color: #fff;
  background: rgba(0, 0, 0, 0.55);
  border-radius: 999px;
  backdrop-filter: blur(3px);
}

/* ---- 三态 ---- */
.cg-state {
  padding: 30px 10px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}
.cg-state-icon {
  font-size: 44px;
}
.cg-state-title {
  font-size: 16px;
  font-weight: 600;
  margin: 0;
}
.cg-state-text {
  opacity: 0.7;
  font-size: 13px;
  max-width: 420px;
  line-height: 1.6;
  margin: 0;
}

/* 骨架 */
.sk-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 14px;
  width: 100%;
}
.sk-card {
  height: 190px;
  border-radius: 14px;
  background: rgba(127, 127, 127, 0.16);
  animation: cgpulse 1.2s infinite;
}
.sk-photo {
  height: 130px;
}
@keyframes cgpulse {
  50% { opacity: 0.4; }
}

/* ---- 分类卡片网格 ---- */
.cat-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 16px;
}
.cat-card {
  border-radius: 14px;
  overflow: hidden;
  cursor: pointer;
  transition: transform 0.16s ease, box-shadow 0.16s ease;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.1);
}
.cat-card:hover {
  transform: translateY(-3px);
  box-shadow: 0 10px 24px rgba(0, 0, 0, 0.18);
}
.cat-cover {
  position: relative;
  aspect-ratio: 4 / 3;
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
}
.cat-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.cat-cover-ph {
  font-size: 40px;
  font-weight: 700;
  opacity: 0.75;
}
.cat-count {
  position: absolute;
  right: 8px;
  top: 8px;
  padding: 2px 9px;
  font-size: 11.5px;
  color: #fff;
  background: rgba(0, 0, 0, 0.55);
  border-radius: 999px;
  backdrop-filter: blur(3px);
}
.cat-body {
  padding: 10px 12px 12px;
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
}
.cat-name {
  margin: 0;
  font-size: 15px;
  font-weight: 700;
}
.cat-sub-hint {
  font-size: 11.5px;
  opacity: 0.6;
  white-space: nowrap;
}

/* ---- 二级头部 + chips ---- */
.cg-detail-head {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
  flex-wrap: wrap;
}
.cg-detail-title {
  margin: 0;
  font-size: 19px;
  font-weight: 700;
}
.cg-detail-count {
  font-size: 12.5px;
  opacity: 0.65;
}
.chip-row {
  display: flex;
  gap: 8px;
  overflow-x: auto;
  padding: 2px 2px 10px;
}
.chip {
  flex: 0 0 auto;
  padding: 5px 12px;
  font-size: 12.5px;
  border-radius: 999px;
  border: 1px solid rgba(127, 127, 127, 0.35);
  background: transparent;
  color: inherit;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s;
}
.chip:hover {
  border-color: rgba(106, 141, 240, 0.7);
}
.chip.on {
  background: rgba(106, 141, 240, 0.16);
  border-color: rgba(106, 141, 240, 0.8);
  font-weight: 600;
}
.chip-n {
  opacity: 0.65;
  font-size: 11px;
  margin-left: 2px;
}

/* ---- 照片网格 ---- */
.photo-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 10px;
}
.photo-cell {
  margin: 0;
  position: relative;
  aspect-ratio: 1;
  border-radius: 10px;
  overflow: hidden;
  cursor: zoom-in;
  background: rgba(127, 127, 127, 0.12);
  transition: transform 0.12s ease;
}
.photo-cell:hover {
  transform: translateY(-2px);
}
.photo-cell img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.photo-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 28px;
  opacity: 0.6;
}
.photo-cap {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 14px 8px 6px;
  font-size: 11px;
  color: #fff;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.6));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

@media (max-width: 640px) {
  .cat-grid { grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 10px; }
  .photo-grid { grid-template-columns: repeat(auto-fill, minmax(110px, 1fr)); }
}

/* ---- FEAT-050：批量选择 / 删除 ---- */
.cg-spacer {
  flex: 1;
}
.btn.active {
  border-color: rgba(106, 141, 240, 0.8);
  color: #6a8df0;
}
.btn-danger {
  color: #e03131;
  border-color: rgba(224, 49, 49, 0.5);
}
.btn-danger:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.photo-cell.selectable {
  cursor: pointer;
}
.photo-cell.checked img {
  opacity: 0.55;
}
.photo-cell.checked {
  outline: 3px solid rgba(106, 141, 240, 0.85);
  outline-offset: -3px;
}
.cell-check {
  position: absolute;
  left: 7px;
  top: 7px;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 2px solid rgba(255, 255, 255, 0.9);
  background: rgba(0, 0, 0, 0.35);
  color: #fff;
  font-size: 13px;
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: none;
}
.cell-check.on {
  background: #4c8dff;
  border-color: #4c8dff;
}
.del-mask {
  position: fixed;
  inset: 0;
  z-index: 1100;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
}
.del-dialog {
  width: min(420px, 92vw);
  border-radius: 14px;
  padding: 18px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.28);
}
.del-dialog h4 {
  margin: 0 0 4px;
  font-size: 16px;
}
.del-opt {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  padding: 12px 14px;
  border-radius: 10px;
  border: 1px solid rgba(127, 127, 127, 0.32);
  background: transparent;
  color: inherit;
  cursor: pointer;
  text-align: left;
  transition: border-color 0.15s, background 0.15s;
}
.del-opt:hover {
  border-color: rgba(106, 141, 240, 0.75);
  background: rgba(106, 141, 240, 0.08);
}
.del-opt span {
  font-size: 12px;
  opacity: 0.65;
  line-height: 1.5;
}
.del-cancel {
  align-self: flex-end;
}
</style>
