<script setup lang="ts">
/**
 * 人物画廊 —— 智慧相册「人物」分类的展示组件
 *
 * 数据源：persons.db 直读（list_persons，按脸数降序）；头像本地裁剪缓存。
 * 支持：行内重命名 / 合并到其他人物（二次确认）。
 * 人物注册表/人物浏览的唯一入口（相册页扫描面板已不再展示注册表）。
 */
import { computed, onBeforeUnmount, onMounted, ref, type Directive } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { PersonInfo, PersonPhotoItem } from "../types/photo";
import ConfirmDialog from "./ConfirmDialog.vue";
import ContextMenu, { type ContextMenuEntry } from "./ContextMenu.vue";
import PhotoLightbox from "./PhotoLightbox.vue";
import PhotoDeleteDialog, { type DeleteMode } from "./PhotoDeleteDialog.vue";
import PhotoActionMenu from "./PhotoActionMenu.vue";
import PhotoZoomBar from "./PhotoZoomBar.vue";
import SelectAllCheckbox from "./SelectAllCheckbox.vue";
import ResizableDialog from "./ResizableDialog.vue";
import { useAlbumStore } from "../stores/album";
import { useThemeStore } from "../stores/theme";
import { usePhotoActions } from "../composables/usePhotoActions";
import type { PhotoMarkState } from "../types/photo";

const theme = useThemeStore();
/** 用于调用 prewarmThumbs 等相册级缩略图命令 */
const albumStore = useAlbumStore();

/** FEAT-046：外部跳转定位的人物 id（回忆页近期人物 → /smart?tab=face&person={pid}）
 *  加载完成后自动打开该人物的照片弹窗；无跳转场景不传，行为不变 */
const props = defineProps<{ focusPid?: string | null }>();
/** 卡片/弹窗底色：跟随主题浅深模式，保证文字始终可读 */
const surfaceStyle = computed(() => theme.cardStyle);
const persons = ref<PersonInfo[]>([]);
const loading = ref(true);
const loadError = ref("");
const actionMsg = ref("");
/** pid → 头像 URL（获取失败无键，回退首字占位） */
const avatarMap = ref<Record<string, string>>({});
/** FEAT-047：头像资源 URL 时间戳（自选头像覆盖同路径文件后破 webview 图片缓存） */
const avatarTs = ref(Date.now());

/* ── FEAT-124：人物卡改成「照片裁切 + 文字嵌入」────────────────────────
   卡片封面用 4:5 的人脸裁切（后端 `get_person_covers_bulk`），不再是圆头像；
   姓名/张数用底部渐变压在照片上（与相册墙 `.tile-seal` 同一套语言）。
   头像（96×96）仍然保留，但只用于合并弹窗等小圆片场景。 */
const coverMap = ref<Record<string, string>>({});
const coverTs = ref(Date.now());
let unlistenCoverRefresh: UnlistenFn | null = null;

function coverUrl(_pid: string, path: string): string {
  return withCacheBust(path, coverTs.value);
}

/** 批量取封面：命中的直接渲染，未命中的后端排队后台裁切并 emit 事件 */
async function loadCovers() {
  const pids = persons.value.map((p) => p.id);
  if (!pids.length) return;
  try {
    const paths = await invoke<(string | null)[]>("get_person_covers_bulk", { pids });
    const next: Record<string, string> = { ...coverMap.value };
    persons.value.forEach((p, i) => {
      const path = paths[i];
      if (path) next[p.id] = coverUrl(p.id, path);
    });
    coverMap.value = next;
  } catch {
    /* 封面不可用 → 首字占位，不影响列表 */
  }
}

/** 后台裁切完成 → 渐进替换（933 人现场裁一轮要几十秒，不能阻塞首屏） */
async function bindCoverRefresh() {
  unlistenCoverRefresh = await listen<{ pid: string; path: string }>(
    "person-cover-refreshed",
    (ev) => {
      const { pid, path } = ev.payload;
      coverTs.value = Date.now();
      coverMap.value = { ...coverMap.value, [pid]: coverUrl(pid, path) };
    },
  );
}

/** 资源 URL + 时间戳（同名覆盖后仍能刷新）；mock 环境是 data URL，追加 ?t= 会让它失效 */
function withCacheBust(path: string, ts: number): string {
  const url = convertFileSrc(path);
  return url.startsWith("data:") ? url : `${url}?t=${ts}`;
}

/** 头像 asset URL：附时间戳参数，同名覆盖后仍能刷新显示 */
function avatarUrl(_pid: string, path: string): string {
  return withCacheBust(path, avatarTs.value);
}

/* ---- 行内重命名 ---- */
const editingId = ref<string | null>(null);
const editingName = ref("");

function startRename(p: PersonInfo) {
  editingId.value = p.id;
  editingName.value = p.name && p.name !== p.id ? p.name : "";
}

async function saveRename(p: PersonInfo) {
  const name = editingName.value.trim();
  if (!name) return stopRename();
  try {
    await invoke("rename_person", { pid: p.id, name });
    p.name = name;
    flash(`已重命名为「${name}」`);
  } catch (e) {
    flash(`重命名失败：${String(e)}`);
  } finally {
    stopRename();
  }
}

function stopRename() {
  editingId.value = null;
  editingName.value = "";
}

/* ---- 合并：选中的源人物 → 弹窗挑目标 → 二次确认 ---- */
const mergingSource = ref<PersonInfo | null>(null);
/** 已选定待确认的目标 */
const pendingTarget = ref<PersonInfo | null>(null);
const mergingBusy = ref(false);

const mergeCandidates = () => persons.value.filter((p) => p.id !== mergingSource.value?.id);

function displayName(p: PersonInfo): string {
  return p.name && p.name !== p.id ? p.name : p.id;
}

async function doMerge() {
  if (!mergingSource.value || !pendingTarget.value || mergingBusy.value) return;
  const source = mergingSource.value;
  const target = pendingTarget.value;
  mergingBusy.value = true;
  try {
    await invoke("merge_persons", { target: target.id, source: source.id });
    // 目标头像的人脸集合已变化 → 强制刷新缓存
    const fresh = await invoke<string>("get_person_avatar", {
      pid: target.id,
      forceRefresh: true,
    });
    avatarMap.value = { ...avatarMap.value, [target.id]: avatarUrl(target.id, fresh) };
    flash(`已将 ${displayName(source)}（${source.face_count} 张脸）并入 ${displayName(target)}`);
    await load();
  } catch (e) {
    flash(`合并失败：${String(e)}`);
  } finally {
    mergingBusy.value = false;
    pendingTarget.value = null;
    mergingSource.value = null;
  }
}

/* ---- 加载 ---- */
async function load() {
  loading.value = true;
  loadError.value = "";
  try {
    persons.value = await invoke<PersonInfo[]>("list_persons");
    // BUG-2026-0910-002 根治（原实现逐个 await → 900 次 IPC/每次画廊，峰值 3600 行日志/秒）：
    // ① 缓存命中（~98%）单次批量带回（1 次往返替代 N 次，后端只写 1 行日志）；
    // ② 未命中（需现场裁剪的少数）仅占位，点击该人物时再按需裁剪（ensureAvatar）。
    const paths = await invoke<(string | null)[]>("get_person_avatars_bulk", {
      pids: persons.value.map((p) => p.id),
    });
    const next: Record<string, string> = { ...avatarMap.value };
    persons.value.forEach((p, i) => {
      const path = paths[i];
      if (path) next[p.id] = avatarUrl(p.id, path);
    });
    avatarMap.value = next;
    // FEAT-124：人物卡封面（4:5 照片裁切）
    await loadCovers();
    // FEAT-046：携带 focusPid → 自动打开该人物的照片弹窗
    if (props.focusPid) {
      const target = persons.value.find((p) => p.id === props.focusPid);
      if (target) await openPhotos(target);
    }
  } catch (e) {
    loadError.value = String(e);
  } finally {
    loading.value = false;
  }
}

function flash(msg: string) {
  actionMsg.value = msg;
  setTimeout(() => (actionMsg.value = ""), 4000);
}

onMounted(() => {
  void load();
  void bindAvatarRefresh();
  void bindCoverRefresh();
});

onBeforeUnmount(() => {
  unlistenAvatarRefresh?.();
  unlistenCoverRefresh?.();
});

/* ---- 查看该人物的照片（复用预计算缩略图 + 大图看图器）---- */
const viewingPerson = ref<PersonInfo | null>(null);
const viewingPhotos = ref<PersonPhotoItem[]>([]);
const viewingLoading = ref(false);
const viewingError = ref("");
const lightboxOpen = ref(false);
const lightboxIndex = ref(0);

/** 缩略图优先用已算好的缓存；未缓存时展示占位（不退回原图，避免 4K 大图拉慢） */
function photoThumbSrc(p: PersonPhotoItem): string {
  return p.thumb ? convertFileSrc(p.thumb) : "";
}

/** 供看图器使用的人物照片列表（原图路径 + 所属相册 ID，便于大图查看器触发自动扫描） */
const lightboxPhotos = computed(() =>
  viewingPhotos.value.map((it) => ({ path: it.path, albumId: it.album_id })),
);

async function openPhotos(p: PersonInfo) {
  viewingPerson.value = p;
  viewingPhotos.value = [];
  viewingError.value = "";
  viewingLoading.value = true;
  exitSelectMode();
  // BUG-2026-0910-002：未缓存头像按需裁剪——画廊加载不再为缺缓存的少数人物现场解码，
  // 用户点开该人物时才裁剪并补上（代价转移到真正看它的那一刻）
  if (!avatarMap.value[p.id]) {
    invoke<string>("get_person_avatar", { pid: p.id, forceRefresh: false })
      .then((cachePath) => {
        avatarMap.value = { ...avatarMap.value, [p.id]: avatarUrl(p.id, cachePath) };
      })
      .catch(() => {
        /* 原图缺失等：保持占位 */
      });
  }
  try {
    // 第一次拉取：后端已自动补齐缩略图（缺图则 ensure_grid_thumb 生成后落盘）
    let items = await invoke<PersonPhotoItem[]>("get_person_photos", { pid: p.id });
    // 兜底：若仍有 thumb=null，按相册分组显式预热一次（覆盖"原图丢失导致补齐失败"的边角场景）
    const missing = items.filter((it) => !it.thumb && it.album_id != null);
    if (missing.length) {
      const byAlbum = new Map<number, string[]>();
      for (const it of missing) {
        const aid = it.album_id!;
        if (!byAlbum.has(aid)) byAlbum.set(aid, []);
        byAlbum.get(aid)!.push(it.path);
      }
      // 预热各相册（失败也不阻塞）
      await Promise.all(
        [...byAlbum.entries()].map(([aid, paths]) =>
          albumStore.prewarmThumbs(aid, paths).catch(() => null),
        ),
      );
      // 再拉一次：此时缓存应已就绪，thumb 字段会带路径
      items = await invoke<PersonPhotoItem[]>("get_person_photos", { pid: p.id });
    }
    viewingPhotos.value = items;
    if (!items.length) viewingError.value = "该人物暂无登记照片。";
    // FEAT-095：批量读评分 + 标记状态（右键打分菜单 / 归档按钮文案都依赖它）
    void loadRatings(items.map((i) => i.path));
    void photoActions.loadMarkStates(items.map((i) => i.path));
  } catch (e) {
    viewingError.value = String(e);
  } finally {
    viewingLoading.value = false;
  }
}

function closePhotos() {
  viewingPerson.value = null;
  viewingPhotos.value = [];
  lightboxOpen.value = false;
  avatarPickMode.value = false;
  exitSelectMode();
}

/* ---- FEAT-095（R1）：矩阵预览框缩放 ----
 * 缩略图缓存固定 256px，放大只做 CSS 拉伸（不触发重新生成），
 * 故上限取 320px —— 再大只是糊图。上限集中在 PhotoZoomBar 内维护。 */
const cellSize = ref(140);

function openPhoto(i: number) {
  lightboxIndex.value = i;
  lightboxOpen.value = true;
}

/* ---- FEAT-050：多选删除（查看弹窗右上角「☑ 多选」→ 勾选 → 删除按钮 → 两种选择） ---- */
const selectMode = ref(false);
const selected = ref<Set<string>>(new Set());
/** FEAT-095：待删除路径（交由 PhotoDeleteDialog 二次确认 + 选模式） */
const modeDialogPaths = ref<string[] | null>(null);
/** FEAT-095：删除执行中（禁用弹窗按钮，防重复提交） */
const deleting = ref(false);
/** FEAT-095：归档 / 隐藏动作（与全站共用同一套语义） */
const photoActions = usePhotoActions();

function enterSelectMode() {
  selectMode.value = true;
  selected.value = new Set();
}
function exitSelectMode() {
  selectMode.value = false;
  selected.value = new Set();
}
function toggleSelect(path: string) {
  const next = new Set(selected.value);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  selected.value = next;
}
/**
 * 全选 / 取消全选（FEAT-096：改三态复选框后，参数为「本次点击意图」而非内部自行判断）
 *
 * @param selectAll `true` = 全选；`false` = 取消全选
 */
function selectAllPhotos(selectAll = true) {
  selected.value = selectAll
    ? new Set(viewingPhotos.value.map((it) => it.path))
    : new Set();
}
function openDeleteDialog() {
  if (!selected.value.size) return;
  modeDialogPaths.value = [...selected.value];
}
/** 预览大图内的删除按钮：对当前照片弹删除方式 */
function askDeleteCurrent() {
  const it = viewingPhotos.value[lightboxIndex.value];
  if (it) modeDialogPaths.value = [it.path];
}

/**
 * FEAT-095：删除方式二选一（相册记录删除 / 本地删除移入回收站）
 *
 * 两种模式都由后端按路径处理（不依赖相册上下文），所以人物画廊这类
 * 跨相册来源也能正确执行。执行后重拉当前人物照片列表。
 */
async function pickMode(mode: DeleteMode) {
  const paths = modeDialogPaths.value ?? [];
  if (!paths.length) return;
  deleting.value = true;
  const cmd = mode === "records" ? "delete_photo_records_by_paths" : "delete_photos_to_trash";
  try {
    const outcome = await invoke<{
      requested: number;
      deleted: number;
      failed: number;
      failed_paths: string[];
    }>(cmd, { paths });
    modeDialogPaths.value = null;
    // 重新拉取当前人物照片（缓存已清，剩余照片即时补齐）
    const person = viewingPerson.value;
    if (person) await openPhotos(person);
    exitSelectMode();
    if (lightboxOpen.value) {
      if (!viewingPhotos.value.length) lightboxOpen.value = false;
      else lightboxIndex.value = Math.min(lightboxIndex.value, viewingPhotos.value.length - 1);
    }
    if (outcome.failed > 0) {
      flash(`已删除 ${outcome.deleted} / ${outcome.requested} 张（失败 ${outcome.failed}）`);
    } else {
      flash(`已删除 ${outcome.deleted} 张（${mode === "trash" ? "已移入回收站" : "仅清除记录，本地文件保留"}）`);
    }
  } catch (e) {
    flash(`删除失败：${String(e)}`);
  } finally {
    deleting.value = false;
    selected.value = new Set();
  }
}

/* ---- FEAT-095（R4 / R8）：归档 / 隐藏 ---- */

/** 从当前列表移除已处理照片（列表已在内存，直接过滤即可，无需重拉） */
function dropFromView(paths: string[]) {
  const removed = new Set(paths);
  viewingPhotos.value = viewingPhotos.value.filter((it) => !removed.has(it.path));
  const nextSelected = new Set(selected.value);
  for (const p of paths) nextSelected.delete(p);
  selected.value = nextSelected;
  if (lightboxOpen.value) {
    if (!viewingPhotos.value.length) lightboxOpen.value = false;
    else lightboxIndex.value = Math.min(lightboxIndex.value, viewingPhotos.value.length - 1);
  }
}

/** 归档选中 / 当前照片 */
async function doArchive(paths: string[], marked: boolean) {
  const ok = await photoActions.apply("archived", paths, marked);
  if (ok) dropFromView(paths);
}

/** 隐藏选中 / 当前照片 */
async function doHide(paths: string[], marked: boolean) {
  const ok = await photoActions.apply("hidden", paths, marked);
  if (ok) dropFromView(paths);
}

/** 当前大图照片的标记状态（用于按钮文案切换） */
function currentMarkState(path: string): PhotoMarkState | null {
  return photoActions.markStateOf(path);
}

/* ---- FEAT-095（R8）：缩略图右键菜单 ---- */

/**
 * 右键菜单状态（FEAT-095：条目与快速打分 / 标签 / 删除弹窗都复用 `PhotoActionMenu`）
 *
 * 人物画廊比通用菜单多一项「设为该人物的头像」（FEAT-047 的入口），
 * 通过 `extraItems` 传入，其余条目由共享组件统一维护。
 */
const cellCtx = ref<{ x: number; y: number; paths: string[] } | null>(null);

function onCellContextMenu(e: MouseEvent, path: string) {
  if (selectMode.value || avatarPickMode.value) return;
  e.preventDefault();
  e.stopPropagation();
  cellCtx.value = { x: e.clientX, y: e.clientY, paths: [path] };
}

/** 人物画廊专属条目：设为该人物的头像 */
const cellCtxExtra = computed<ContextMenuEntry[]>(() => [
  {
    label: "设为该人物的头像",
    icon: "⭐",
    onClick: () => {
      const path = cellCtx.value?.paths[0];
      if (path) onSetAvatar(path);
    },
  },
]);

/** 右键动作完成后的刷新：归档 / 隐藏 / 删除都会改变当前人物的照片列表 */
async function onCellCtxRefresh() {
  const person = viewingPerson.value;
  if (person) await openPhotos(person);
}

/** 人物画廊内的评分（后端 photo_ratings 与全站同一体系） */
const ratings = ref<Record<string, number>>({});

function ratingOf(path: string): number {
  return ratings.value[path] ?? 0;
}

/** 打开照片时把评分一并读出来（一次批量，右键打分才能显示当前分） */
async function loadRatings(paths: string[]) {
  if (!paths.length) return;
  try {
    const rows = await albumStore.getPhotoRatings(paths);
    const map: Record<string, number> = { ...ratings.value };
    for (const [p, r] of rows) map[p] = r;
    ratings.value = map;
  } catch {
    /* 评分读取失败按未评分处理 */
  }
}

/* ---- FEAT-047：自选头像（照片弹窗内指定一张照片作为头像封面） ---- */
/** 正在设置中的照片路径（防重复点击）；null = 空闲 */
const settingAvatar = ref<string | null>(null);
/** 头像挑选模式：由头像右键菜单「更换头像…」进入，弹窗顶部显示提示条 */
const avatarPickMode = ref(false);

/** 命令返回：头像与卡片封面两条缓存路径（后端一次写入） */
interface PersonPortrait {
  avatar: string;
  cover: string;
}

/** 头像 + 封面一起刷新（人物卡画的是 cover，只刷 avatar 等于没变，BUG-2026-1028-013） */
function applyPortrait(pid: string, p: PersonPortrait): void {
  avatarTs.value = Date.now();
  coverTs.value = Date.now();
  avatarMap.value = { ...avatarMap.value, [pid]: avatarUrl(pid, p.avatar) };
  coverMap.value = { ...coverMap.value, [pid]: coverUrl(pid, p.cover) };
}

async function setAvatar(p: PersonInfo, photoPath: string) {
  if (settingAvatar.value) return;
  settingAvatar.value = photoPath;
  try {
    const portrait = await invoke<PersonPortrait>("set_person_avatar_from_photo", {
      pid: p.id,
      photoPath,
    });
    applyPortrait(p.id, portrait);
    if (avatarPickMode.value) {
      // 从「更换头像」入口来的：选完即收工，回到画廊看效果
      closePhotos();
    }
    flash(`已将所选照片设为 ${displayName(p)} 的头像与卡片封面`);
  } catch (e) {
    flash(`设置头像失败：${String(e)}`);
  } finally {
    settingAvatar.value = null;
  }
}

/* ---- 头像右键菜单：更换头像 / 恢复自动头像 / 合并 ---- */
const ctxMenu = ref<{ x: number; y: number; person: PersonInfo } | null>(null);

function onAvatarContextMenu(e: MouseEvent, p: PersonInfo) {
  ctxMenu.value = { x: e.clientX, y: e.clientY, person: p };
}

const ctxItems = computed<ContextMenuEntry[]>(() => {
  const p = ctxMenu.value?.person;
  if (!p) return [];
  return [
    {
      label: "更换头像 / 封面…",
      icon: "🖼",
      onClick: () => {
        avatarPickMode.value = true;
        void openPhotos(p);
      },
    },
    {
      label: "恢复自动头像",
      icon: "🔄",
      onClick: () => void restoreAvatar(p),
    },
    { divider: true },
    {
      label: "合并到其他人物…",
      icon: "🔗",
      onClick: () => (mergingSource.value = p),
    },
  ];
});

/** 恢复自动头像：抹掉自选标记 + 按当前代表脸重裁（后端命令） */
async function restoreAvatar(p: PersonInfo) {
  if (restoringAvatar.value) return;
  restoringAvatar.value = p.id;
  try {
    const portrait = await invoke<PersonPortrait>("restore_person_avatar", { pid: p.id });
    applyPortrait(p.id, portrait);
    flash(`已恢复 ${displayName(p)} 的自动头像与封面`);
  } catch (e) {
    flash(`恢复自动头像失败：${String(e)}`);
  } finally {
    restoringAvatar.value = null;
  }
}

/* ---- 重建人物库（换识别模型后必须走一次） ---- */
const rebuilding = ref(false);
const rebuildConfirm = ref(false);

async function doRebuild() {
  rebuildConfirm.value = false;
  if (rebuilding.value) return;
  rebuilding.value = true;
  try {
    const r = await invoke<{
      backup: string;
      persons: number;
      faces: number;
      avatars_cleared: number;
    }>("rebuild_person_registry");
    avatarMap.value = {};
    persons.value = [];
    flash(
      `已重建人物库（清空 ${r.persons} 位人物 / ${r.faces} 张脸 / ${r.avatars_cleared} 个头像缓存，备份：${r.backup}）。请到扫描面板对相册执行「全量扫描」重新识别人脸。`,
    );
    await load();
  } catch (e) {
    flash(`重建人物库失败：${String(e)}`);
  } finally {
    rebuilding.value = false;
  }
}

/* ---- 后台重裁完成事件：头像校验失配时后端逐个重裁并上报 ---- */
const restoringAvatar = ref<string | null>(null);
let unlistenAvatarRefresh: UnlistenFn | null = null;

async function bindAvatarRefresh() {
  try {
    unlistenAvatarRefresh = await listen<{ pid: string; path: string }>(
      "person-avatar-refreshed",
      (e) => {
        const { pid, path } = e.payload;
        if (!pid || !path) return;
        avatarTs.value = Date.now();
        avatarMap.value = { ...avatarMap.value, [pid]: avatarUrl(pid, path) };
      },
    );
  } catch {
    /* 事件订阅失败不影响画廊主体 */
  }
}

/** 模板入口：从查看弹窗安全取当前人物（v-if 作用域内必然存在） */
function onSetAvatar(photoPath: string) {
  const p = viewingPerson.value;
  if (!p) return;
  void setAvatar(p, photoPath);
}

/** 本地指令：进入重命名时自动聚焦 */
const vFocus: Directive<HTMLElement> = {
  mounted: (el) => el.focus(),
};
</script>

<template>
  <div class="pg-wrap" :style="{ color: theme.textColor }">
    <div class="pg-toolbar">
      <span class="pg-summary" v-if="!loading && !loadError">共 {{ persons.length }} 位人物（按出现次数排序）</span>
      <button
        class="mini-btn danger pg-rebuild-btn"
        :disabled="rebuilding"
        @click="rebuildConfirm = true"
        title="更换人脸识别模型后使用：备份并清空人物库与头像缓存，再对相册重新扫描以重建人物"
      >
        {{ rebuilding ? "重建中…" : "重建人物库" }}
      </button>
      <button class="btn pg-refresh-btn" @click="load">刷新</button>
    </div>
    <div v-if="actionMsg" class="pg-action-msg">{{ actionMsg }}</div>

    <!-- 加载中 -->
    <div v-if="loading" class="pg-state">正在读取人物注册表…</div>

    <!-- 加载失败 -->
    <div v-else-if="loadError" class="pg-state pg-error">
      读取失败：{{ loadError }}
      <p class="pg-hint">请确认项目目录下存在 python/data/persons.db（执行过人脸识别扫描）。</p>
    </div>

    <!-- 空状态 -->
    <div v-else-if="!persons.length" class="pg-state">
      <p>暂无已识别的人物。</p>
      <p class="pg-hint">进入任意相册 → 「综合扫描」（勾选人脸识别）后，识别到的人物会自动登记在此。</p>
    </div>

    <!-- 人物卡片网格（FEAT-124：照片裁切封面 + 文字嵌入） -->
    <div v-else class="person-grid">
      <article
        v-for="p in persons"
        :key="p.id"
        class="person-card"
        :title="`${displayName(p)}（${p.id}）· ${p.face_count} 张照片`"
      >
        <div
          class="pc-cover"
          @click.stop="openPhotos(p)"
          @contextmenu.prevent="onAvatarContextMenu($event, p)"
          title="点击查看该人物的照片；右键可更换头像"
        >
          <img v-if="coverMap[p.id]" :src="coverMap[p.id]" class="pc-img" loading="lazy" alt="" />
          <span v-else class="pc-ph">{{ displayName(p).slice(0, 1) }}</span>

          <!-- 文字嵌入：底部渐变上压姓名 + 张数 -->
          <div class="pc-cap">
            <template v-if="editingId === p.id">
              <input
                v-model="editingName"
                class="person-rename-input pc-rename"
                maxlength="50"
                placeholder="输入新名称"
                @click.stop
                @keyup.enter="saveRename(p)"
                @keyup.esc="stopRename"
                v-focus
              />
              <div class="pc-edit-actions" @click.stop>
                <button class="mini-btn ok" @click="saveRename(p)">保存</button>
                <button class="mini-btn" @click="stopRename">取消</button>
              </div>
            </template>
            <template v-else>
              <div class="pc-name">{{ p.name && p.name !== p.id ? p.name : "未命名" }}</div>
              <div class="pc-meta">
                <span class="mono pc-id">{{ p.id }}</span>
                <span class="pc-dot">·</span>
                {{ p.face_count }} 张照片
              </div>
            </template>
          </div>

          <!-- hover 才浮出的工具：不抢封面的视觉 -->
          <div v-if="editingId !== p.id" class="pc-tools" @click.stop>
            <button class="mini-btn" title="重命名" @click="startRename(p)">✎</button>
            <button
              class="mini-btn"
              title="合并到其他人物"
              @click="mergingSource = p"
            >
              ⇄
            </button>
          </div>
        </div>
      </article>
    </div>

    <!-- 合并：选择目标人物弹窗 -->
    <Teleport to="body">
      <div v-if="mergingSource && !pendingTarget" class="merge-mask" @click.self="mergingSource = null">
        <div class="merge-dialog glass-card" :style="surfaceStyle">
          <h4>将 {{ displayName(mergingSource) }}（{{ mergingSource.face_count }} 张脸）合并到…</h4>
          <p class="merge-tip">选择保留的目标人物；被并者的人脸与计数将全部转移。</p>
          <div class="merge-list">
            <button
              v-for="c in mergeCandidates()"
              :key="c.id"
              class="merge-item"
              @click="pendingTarget = c"
            >
              <img v-if="avatarMap[c.id]" :src="avatarMap[c.id]" class="merge-avatar" alt="" />
              <span v-else class="merge-avatar merge-avatar-fallback">{{ displayName(c).slice(0, 1) }}</span>
              <span class="merge-name">{{ displayName(c) }}</span>
              <span class="mono merge-id">{{ c.id }}</span>
              <span class="merge-count">{{ c.face_count }} 张脸</span>
            </button>
          </div>
          <div class="merge-actions">
            <button class="btn" @click="mergingSource = null">取消</button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- 合并二次确认 -->
    <ConfirmDialog
      :visible="!!pendingTarget && !!mergingSource"
      title="合并人物"
      :message="
        pendingTarget && mergingSource
          ? `确定将 ${displayName(mergingSource)}（${mergingSource.face_count} 张脸）并入 ${displayName(pendingTarget)}（${pendingTarget.face_count} 张脸）吗？合并后不可自动拆分。`
          : ''
      "
      confirm-text="确认合并"
      :danger="false"
      @confirm="doMerge"
      @cancel="pendingTarget = null"
    />

    <!-- 查看该人物的照片（缩略图网格）—— FEAT-096（R3）：窗口式可拖边缩放 + 拖标题栏移动 + 全屏 -->
    <Teleport to="body">
      <div v-if="viewingPerson" class="viewer-mask" @click.self="closePhotos">
        <ResizableDialog
          class="viewer-dialog"
          :style="surfaceStyle"
          :initial-width="760"
          :initial-height="580"
          :min-width="440"
          :min-height="320"
        >
          <template #head="{ fullscreen, toggleFullscreen }">
            <div class="viewer-head">
              <span class="viewer-title">{{ displayName(viewingPerson) }} 的照片</span>
              <span class="viewer-count">{{ viewingPhotos.length }} 张</span>
              <!-- FEAT-095（R1）：矩阵格子缩放 -->
              <PhotoZoomBar v-model="cellSize" :disabled="!viewingPhotos.length" />
              <span class="viewer-spacer"></span>
              <template v-if="selectMode">
                <!-- FEAT-096：三态全选复选框（空圈 → 打勾；全选后再点即取消全选） -->
                <SelectAllCheckbox
                  :selected-count="selected.size"
                  :total-count="viewingPhotos.length"
                  @toggle="selectAllPhotos"
                />
                <!-- FEAT-095：多选批量归档 / 隐藏（针对照片，非相册整体） -->
                <button class="mini-btn" :disabled="!selected.size || photoActions.busy.value" @click="doArchive([...selected], true)">
                  📦 归档（{{ selected.size }}）
                </button>
                <button class="mini-btn" :disabled="!selected.size || photoActions.busy.value" @click="doHide([...selected], true)">
                  🔒 隐藏（{{ selected.size }}）
                </button>
                <button class="mini-btn danger" :disabled="!selected.size || deleting" @click="openDeleteDialog">
                  🗑 删除选中（{{ selected.size }}）
                </button>
                <button class="mini-btn" @click="exitSelectMode">退出</button>
              </template>
              <button v-else class="mini-btn" @click="enterSelectMode">☑ 多选</button>
              <!-- FEAT-096（R3）：全屏 / 还原（双击标题栏同效） -->
              <button class="mini-btn" :title="fullscreen ? '还原窗口 (F11)' : '全屏查看 (F11)'" @click="toggleFullscreen">
                {{ fullscreen ? "🗗 还原" : "⛶ 全屏" }}
              </button>
              <button class="btn viewer-close" @click="closePhotos">✕</button>
            </div>
          </template>
          <!-- 头像挑选模式提示（由头像右键菜单「更换头像…」进入） -->
          <div v-if="avatarPickMode && !selectMode" class="viewer-pick-hint">
            正在更换封面：鼠标移到照片上点「设为头像」即可（选完自动关闭）
          </div>
          <div v-if="viewingLoading" class="viewer-state">正在读取缩略图…</div>
          <div v-else-if="viewingError" class="viewer-state viewer-error">{{ viewingError }}</div>
          <div v-else class="viewer-grid" :style="{ gridTemplateColumns: `repeat(auto-fill, minmax(${cellSize}px, 1fr))` }">
            <div
              v-for="(it, i) in viewingPhotos"
              :key="it.path"
              class="viewer-cell"
              :class="{ checked: selectMode && selected.has(it.path) }"
              @click="selectMode ? toggleSelect(it.path) : openPhoto(i)"
              @contextmenu.prevent.stop="onCellContextMenu($event, it.path)"
            >
              <img
                v-if="it.thumb"
                :src="photoThumbSrc(it)"
                class="viewer-photo"
                :title="`${it.path}\n右键可打分 / 标签 / 编辑 / 归档 / 隐藏 / 删除`"
                loading="lazy"
                alt=""
                draggable="false"
                @click.stop="selectMode ? toggleSelect(it.path) : openPhoto(i)"
              />
              <div v-else class="viewer-photo viewer-photo-missing" :title="`缩略图生成中或原图不可用：${it.path}`">🖼</div>
              <!-- FEAT-095：评分星标角标（与 PhotoGrid 一致，右键打分后即时反映） -->
              <span v-if="ratingOf(it.path) > 0" class="cell-rating" :title="`打分 ${ratingOf(it.path)} 星`">
                {{ "★".repeat(ratingOf(it.path)) }}
              </span>
              <!-- FEAT-095：归档 / 隐藏状态角标 -->
              <span v-if="photoActions.markStateOf(it.path)" class="cell-mark" :title="photoActions.markStateOf(it.path) === 'archived' ? '已归档' : '已隐藏'">
                {{ photoActions.markStateOf(it.path) === "archived" ? "📦" : "🔒" }}
              </span>
              <!-- FEAT-047：自选头像入口（多选模式下隐藏） -->
              <button
                v-if="!selectMode"
                class="viewer-set-avatar"
                :class="{ 'always-on': avatarPickMode }"
                title="设为该人物的头像"
                :disabled="settingAvatar === it.path"
                @click.stop="onSetAvatar(it.path)"
              >
                {{ settingAvatar === it.path ? "设置中…" : "设为头像" }}
              </button>
              <span v-if="selectMode" class="cell-check" :class="{ on: selected.has(it.path) }">
                {{ selected.has(it.path) ? "✓" : "" }}
              </span>
            </div>
          </div>
          <!-- 原图看图器（启用删除 / 归档 / 隐藏按钮） -->
          <PhotoLightbox
            v-if="lightboxOpen"
            :photos="lightboxPhotos"
            :index="lightboxIndex"
            deletable
            markable
            :archived="currentMarkState(lightboxPhotos[lightboxIndex]?.path ?? '') === 'archived'"
            :hidden="currentMarkState(lightboxPhotos[lightboxIndex]?.path ?? '') === 'hidden'"
            @close="lightboxOpen = false"
            @delete="askDeleteCurrent"
            @archive="(p, m) => doArchive([p], m)"
            @hide="(p, m) => doHide([p], m)"
          />
        </ResizableDialog>
      </div>
    </Teleport>

    <!-- FEAT-095：删除方式选择（相册记录删除 / 本地删除移入回收站） -->
    <PhotoDeleteDialog
      :paths="modeDialogPaths"
      :busy="deleting"
      @pick="pickMode"
      @cancel="modeDialogPaths = null"
    />

    <!-- FEAT-095（R8）：缩略图右键菜单（复用 PhotoActionMenu，人物画廊追加「设为头像」） -->
    <PhotoActionMenu
      :target="cellCtx"
      :extra-items="cellCtxExtra"
      :on-refresh="onCellCtxRefresh"
      :on-close="() => (cellCtx = null)"
    />

    <!-- 头像右键菜单：更换头像 / 恢复自动头像 / 合并 -->
    <ContextMenu
      v-if="ctxMenu"
      :items="ctxItems"
      :x="ctxMenu.x"
      :y="ctxMenu.y"
      @close="ctxMenu = null"
    />

    <!-- 重建人物库二次确认（危险操作：清空现有 P 编号与合并结果） -->
    <ConfirmDialog
      :visible="rebuildConfirm"
      title="重建人物库"
      :message="`将备份并清空人物库（现有 ${persons.length} 位人物的编号、改名与合并结果都会重置），同时清除全部人物头像缓存。\n换人脸识别模型后必须执行；完成后请到扫描面板对相册执行「全量扫描」重新识别人脸。\n备份文件与 persons.db 同目录（persons.db.bak-...），确认继续吗？`"
      confirm-text="确认重建"
      :danger="true"
      @confirm="doRebuild"
      @cancel="rebuildConfirm = false"
    />
  </div>
</template>
<style scoped>
.pg-wrap { min-width: 0; }

.pg-toolbar {
  display: flex;
  align-items: center;
  margin-bottom: 12px;
}
.pg-summary {
  opacity: 0.75;
  font-size: 13px;
}
.pg-refresh-btn { margin-left: auto; padding: 4px 12px; font-size: 13px; }
/* 重建人物库入口：次要危险操作，紧贴刷新按钮左侧 */
.pg-rebuild-btn { margin-left: auto; margin-right: 8px; padding: 4px 10px; font-size: 12px; }
.pg-rebuild-btn + .pg-refresh-btn { margin-left: 0; }

.pg-action-msg {
  background: var(--color-primary-soft, #eef5ff);
  border: 1px solid color-mix(in srgb, var(--color-primary) 30%, transparent);
  color: var(--color-primary-text);
  border-radius: 8px;
  padding: 8px 12px;
  font-size: 13px;
  margin-bottom: 12px;
}
body.theme-dark .pg-action-msg {
  color: var(--color-link);
}

.pg-state {
  text-align: center;
  padding: 48px 20px;
  opacity: 0.8;
  color: inherit;
}
.pg-error { color: var(--color-danger); }
.pg-hint {
  opacity: 0.65;
  font-size: 13px;
  margin-top: 8px;
}

/* ── 人物卡：竖排大卡（FEAT-113 ⑦）─────────────────────────────────
   【为什么改】旧版是 176px 宽的**横排**卡：头像 64 + 间距 14 + 内边距 32
   = 110px 被占掉，名字只剩 66px 宽。而名字后面还挤着一个「✎」按钮，
   于是「妈妈」「爸爸」勉强能显示，一碰到「外婆（成都老宅）」这种就只剩
   “外婆（成…” —— 这就是“名字显示不全”的真正原因，**不是省略号写错了**。
   横排卡里再加宽也没用：名字区域是剩余空间，永远会被头像吃掉大半。

   改竖排后名字拿到整张卡的宽度，并允许折两行：名字是这张卡的主信息，
   宁可两行也不截断。列宽 176 → 196：一行少排一两张，换来名字可读。 */
.person-grid {
  display: grid;
  /* FEAT-124：卡片变成 4:5 照片封面，列宽抬到 176px（再小就看不出是谁） */
  grid-template-columns: repeat(auto-fill, minmax(176px, 1fr));
  grid-auto-flow: dense;
  gap: 18px 14px;
}

/* FEAT-124：人物卡 = 一张 4:5 的人脸裁切照片 + 文字嵌在照片底部。
   旧版是「圆头像 + 下方三行元信息」，头像只有 86px 且圆形裁掉了合影上下文；
   用户要的是「显示照片或照片的裁切」，所以封面改回照片，文字压上去。 */
.person-card {
  background: transparent;
  padding: 0;
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: 0;
  text-align: left;
}

/* 封面：4:5 竖版，圆角由卡片自己给（与相册墙瓦片同一套观感） */
.pc-cover {
  position: relative;
  aspect-ratio: 4 / 5;
  border-radius: 10px;
  overflow: hidden;
  cursor: pointer;
  background: color-mix(in srgb, var(--c-fg) 10%, transparent);
  transition: transform var(--dur-1) ease, box-shadow var(--dur-2) ease;
}
.pc-cover:hover {
  transform: translateY(-2px);
  box-shadow: 0 8px 22px rgba(0, 0, 0, 0.28);
}
.pc-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
/* 无封面（还没裁出来 / 人物无脸）→ 首字大号占位，不用灰块 */
.pc-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 44px;
  font-weight: 650;
  color: color-mix(in srgb, var(--c-fg) 45%, transparent);
  background: color-mix(in srgb, var(--color-primary) 14%, transparent);
}

/* 文字嵌入：底部渐变让白字在任何照片上都读得出来 */
.pc-cap {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 26px 10px 9px;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.72));
  color: #fff;
  pointer-events: none;
}
.pc-cap > * {
  pointer-events: auto;
}
.pc-name {
  font-size: 13.5px;
  font-weight: 650;
  letter-spacing: -0.01em;
  line-height: 1.25;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.5);
}
.pc-meta {
  margin-top: 2px;
  font-size: 11px;
  opacity: 0.82;
  display: flex;
  align-items: center;
  gap: 5px;
  font-variant-numeric: tabular-nums;
}
.pc-id {
  font-size: 10.5px;
  opacity: 0.9;
}
.pc-dot {
  opacity: 0.5;
}

/* hover 才浮出的工具条（不抢封面视觉） */
.pc-tools {
  position: absolute;
  top: 6px;
  right: 6px;
  display: flex;
  gap: 4px;
  opacity: 0;
  transition: opacity var(--dur-1) ease;
}
.pc-cover:hover .pc-tools,
.pc-cover:focus-within .pc-tools {
  opacity: 1;
}
.pc-tools .mini-btn {
  background: rgba(0, 0, 0, 0.45);
  color: #fff;
  border: 1px solid rgba(255, 255, 255, 0.25);
  border-radius: 999px;
  width: 24px;
  height: 24px;
  font-size: 12px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
}
.pc-tools .mini-btn:hover {
  background: rgba(0, 0, 0, 0.68);
}

/* 封面上的行内重命名 */
.pc-rename {
  width: 100%;
  box-sizing: border-box;
  font-size: 13px;
  padding: 3px 6px;
  border-radius: 6px;
}
.pc-edit-actions {
  display: flex;
  gap: 4px;
  margin-top: 5px;
}

.person-info { min-width: 0; width: 100%; }

.person-name-row {
  display: flex;
  align-items: flex-start;
  justify-content: center;
  gap: 4px;
  min-width: 0;
}
.person-name {
  font-size: 14px;
  font-weight: 600;
  /* 名字最多两行，**不用省略号**：宁可折行也不截断。
     overflow-wrap 处理超长无空格串（纯英文/ID），避免直接溢出卡片。 */
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  overflow-wrap: anywhere;
  line-height: 1.32;
  max-width: 100%;
}
/* 重命名按钮：命名是低频动作，不该和名字争视线。
   默认隐藏（占位保留，避免悬停时名字跳行），悬停/聚焦才出现。 */
.person-name-row .mini-btn {
  margin-top: 1px;
  flex-shrink: 0;
  opacity: 0;
  transition: opacity var(--dur-1) ease;
  pointer-events: none;
}
.person-card:hover .person-name-row .mini-btn,
.person-card:focus-within .person-name-row .mini-btn {
  opacity: 1;
  pointer-events: auto;
}
.mono { font-family: Consolas, Monaco, monospace; }
.person-id { font-size: 11px; color: var(--color-link); margin-top: 3px; opacity: 0.85; }
.person-date { font-size: 10.5px; opacity: 0.55; margin-top: 3px; font-variant-numeric: tabular-nums; }

.mini-btn {
  border: 1px solid currentColor;
  background: transparent;
  border-radius: 6px;
  font-size: 11px;
  padding: 2px 7px;
  cursor: pointer;
  color: inherit;
  transition: all var(--dur-2);
  flex-shrink: 0;
}
.mini-btn:hover { border-color: var(--color-link); color: var(--color-link); }
.mini-btn.ok { background: var(--color-primary); border-color: var(--color-primary); color: var(--color-on-primary); }
/* 合并按钮：竖排后占整行宽，也更好点 */
.merge-btn { margin-top: 4px; border: none; padding: 2px 4px; font-size: 10.5px; color: var(--fg-3); }

.person-rename-input {
  width: 100%;
  font-size: 13px;
  padding: 3px 8px;
  border: 1px solid var(--color-primary);
  border-radius: 6px;
  outline: none;
}
.person-edit-actions { display: flex; gap: 6px; margin-top: 6px; }

/* 合并选择弹窗 */
.merge-mask {
  position: fixed;
  inset: 0;
  z-index: 1100;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
}
.merge-dialog {
  background: transparent; /* 底色由 surfaceStyle(主题) 提供 */
  border-radius: 14px;
  padding: 20px 22px;
  width: min(420px, 92vw);
  max-height: 76vh;
  display: flex;
  flex-direction: column;
}
.merge-dialog h4 { margin: 0 0 6px; }
.merge-tip { font-size: 12px; opacity: 0.7; margin: 0 0 12px; }
.merge-list { overflow-y: auto; display: flex; flex-direction: column; gap: 8px; }
.merge-item {
  display: flex;
  align-items: center;
  gap: 10px;
  box-shadow: inset 0 0 0 1px rgba(128,138,158,.4);
  background: transparent;
  border-radius: 10px;
  padding: 8px 12px;
  cursor: pointer;
  text-align: left;
  transition: all var(--dur-2);
}
.merge-item:hover { border-color: var(--color-primary); background: color-mix(in srgb, var(--color-primary) 6%, transparent); }
.merge-avatar {
  width: 36px;
  height: 36px;
  border-radius: 50%;
  object-fit: cover;
}
.merge-avatar-fallback {
  background: var(--color-primary);
  color: var(--color-on-primary);
  line-height: 34px;
  text-align: center;
  font-size: 15px;
}
.merge-name { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.merge-id { font-size: 11px; color: var(--color-link); }
.merge-count { margin-left: auto; font-size: 12px; opacity: 0.7; }
.merge-actions { margin-top: 14px; display: flex; justify-content: flex-end; }

/* 人物头像：可点击进入照片查看 */
.person-avatar-wrap { cursor: pointer; }

/* 查看某人物照片弹窗 */
.viewer-mask {
  position: fixed;
  inset: 0;
  z-index: 1200;
  background: rgba(0, 0, 0, 0.55);
}
/* FEAT-096（R3）：窗口尺寸由 ResizableDialog 的拖边把手接管，
   这里不能再写死 width / max-height（否则会和拖出来的实际尺寸打架） */
.viewer-dialog {
  padding: 16px 18px 18px;
  display: flex;
  flex-direction: column;
}
.viewer-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 12px;
  padding: 0 2px;
}
.viewer-title { font-size: 17px; font-weight: 700; }
.viewer-count { font-size: 13px; opacity: 0.7; }
.viewer-close { margin-left: auto; padding: 4px 12px; font-size: 14px; }
.viewer-state { text-align: center; padding: 40px 20px; opacity: 0.8; }
.viewer-error { color: var(--color-danger); }
.viewer-grid {
  overflow-y: auto;
  /* FEAT-096（R3）：窗口可缩放后网格必须自己撑满剩余高度并在内部滚动，
     否则窗口拉高时网格仍只占原高度，底部留一大片空白 */
  flex: 1;
  min-height: 0;
  display: grid;
  /* 格子尺寸由 PhotoZoomBar 的 v-model 驱动（style 绑定在模板上），此处只给兜底值 */
  grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
  gap: 8px;
  align-content: start;
}
.viewer-cell { position: relative; }

/* FEAT-095：评分星标 / 归档隐藏角标 */
.cell-rating {
  position: absolute;
  left: 4px;
  top: 4px;
  font-size: 11px;
  color: #ffce3a;
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.8);
  pointer-events: none;
  z-index: 1;
}
.cell-mark {
  position: absolute;
  right: 4px;
  top: 4px;
  font-size: 12px;
  background: rgba(0, 0, 0, 0.45);
  border-radius: 5px;
  padding: 0 4px;
  pointer-events: none;
  z-index: 1;
}

/* FEAT-050：多选删除 */
.viewer-spacer {
  flex: 1;
}
/* 头像挑选模式提示条（由头像右键菜单「更换头像…」进入） */
.viewer-pick-hint {
  margin: -4px 0 10px;
  padding: 6px 10px;
  border-radius: 8px;
  font-size: 12px;
  background: color-mix(in srgb, var(--color-primary) 12%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-primary) 35%, transparent);
  color: var(--color-primary-text);
}
body.theme-dark .viewer-pick-hint { color: var(--color-link); }
.mini-btn.danger {
  color: var(--color-danger-text);
  border-color: color-mix(in srgb, var(--color-danger-text) 50%, transparent);
}
.mini-btn.danger:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.viewer-cell.checked .viewer-photo {
  opacity: 0.55;
}
.viewer-cell.checked {
  outline: 3px solid rgba(76, 141, 255, 0.85);
  outline-offset: -3px;
  border-radius: 8px;
}
.cell-check {
  position: absolute;
  left: 6px;
  top: 6px;
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
  z-index: 1;
}
.cell-check.on {
  background: #4c8dff;
  border-color: #4c8dff;
}
/* FEAT-047：自选头像悬浮按钮 */
.viewer-set-avatar {
  position: absolute;
  left: 6px;
  right: 6px;
  bottom: 6px;
  padding: 4px 0;
  font-size: 11px;
  color: #fff;
  background: rgba(0, 0, 0, 0.55);
  border: 1px solid rgba(255, 255, 255, 0.35);
  border-radius: 6px;
  cursor: pointer;
  opacity: 0;
  transition: opacity var(--dur-2);
}
.viewer-cell:hover .viewer-set-avatar,
.viewer-set-avatar:focus-visible,
.viewer-set-avatar.always-on {
  opacity: 1;
}
.viewer-set-avatar:disabled {
  cursor: wait;
  opacity: 1;
}
.viewer-photo {
  width: 100%;
  aspect-ratio: 1;
  object-fit: cover;
  border-radius: 8px;
  cursor: zoom-in;
  transition: transform var(--dur-2);
  display: block;
  /* FEAT-095：格子可放大到 320px（> 缓存 256px）时不再放大，避免二次糊化 */
  image-rendering: auto;
}
.viewer-photo:hover { transform: scale(1.04); }
.viewer-photo-missing {
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 30px;
  opacity: 0.45;
  background: rgba(127, 127, 127, 0.1);
  cursor: default;
}
.viewer-photo-missing:hover { transform: none; }

</style>
