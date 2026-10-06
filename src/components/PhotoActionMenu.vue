<script setup lang="ts">
/**
 * FEAT-095（R8）：照片缩略图右键菜单 —— 全站唯一的实现
 *
 * 需求 R8：所有照片的缩略图右键都应支持原图预览下的功能（打分 / 标签 / 编辑 /
 * 删除 / 隐藏 / 归档）。此前四个画廊（PhotoGrid / PersonGallery / CategoryGallery /
 * LocationGallery）各写一份右键菜单，条目与行为都不一样 —— 例如 CategoryGallery
 * 和 LocationGallery 只有两个删除项，连打分 / 标签都没有。本组件把这些收敛到一处：
 *
 *   - 菜单条目与 PhotoLightbox 工具栏一一对应（同一功能的两个入口，行为必须一致）
 *   - 快速打分 / 快速标签 / 删除双模式弹窗都内聚在这里，调用方只提供回调
 *   - 归档 / 隐藏统一走 `usePhotoActions`，语义只有一份实现
 *
 * 调用方契约：只需给出坐标、目标路径与一个「变更后刷新」回调。
 */
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import ContextMenu, { type ContextMenuEntry } from "./ContextMenu.vue";
import PhotoDeleteDialog, { type DeleteMode } from "./PhotoDeleteDialog.vue";
import { useAlbumStore } from "../stores/album";
import { useNotify } from "../composables/useNotify";
import { usePhotoActions } from "../composables/usePhotoActions";
import type { PhotoMarkState, PhotoRating } from "../types/photo";

const props = defineProps<{
  /** null = 菜单关闭；否则为右键坐标 */
  target: { x: number; y: number; paths: string[] } | null;
  /** 变更后由调用方刷新自己的列表（删除 / 归档 / 隐藏都会走这里） */
  onRefresh?: () => void | Promise<void>;
  /** 当前所在相册的标记状态：归档 / 隐藏相册内传对应值，普通相册不传 */
  inAlbumMark?: PhotoMarkState | null;
  /** 额外的菜单条目（如「设为该人物的头像」），插在分隔线之后 */
  extraItems?: ContextMenuEntry[];
  /** 关闭菜单（调用方在回调后应同步清空 target，这里只保证 UI 收起） */
  onClose?: () => void;
}>();

const albumStore = useAlbumStore();
const notify = useNotify();
const photoActions = usePhotoActions();

/* ---------------- 状态 ---------------- */

/** 当前照片评分（path → rating） */
const ratings = ref<Record<string, number>>({});
/** 快速打分弹窗：path + 当前选中分 */
const ratingDlg = ref<{ path: string; value: number } | null>(null);
/** 快速标签弹窗 */
const tagDlg = ref<{ path: string; text: string } | null>(null);
/** 删除方式弹窗 */
const deleteDlg = ref<{ paths: string[] } | null>(null);
const deleting = ref(false);

/* ---------------- 数据 ---------------- */

/** 目标照片（右键的那一张） */
const targetPath = computed(() => props.target?.paths[0] ?? "");
/** 目标照片的全部路径（多选右键时可批量） */
const targetPaths = computed(() => props.target?.paths ?? []);

/** 目标照片当前标记状态（菜单文案与操作方向都依赖它） */
const targetMark = computed<PhotoMarkState | null>(
  () => photoActions.markStateOf(targetPath.value) ?? props.inAlbumMark ?? null,
);

/** 是否处于虚拟相册内（归档 / 隐藏相册：同一按钮 = 取消标记取回原相册） */
const inVirtual = computed(() => props.inAlbumMark != null);

watch(
  () => props.target?.paths,
  async (paths) => {
    if (!paths?.length) return;
    // 打开菜单即把评分与标记状态拉齐，否则菜单文案 / 当前星数会不准
    try {
      const rows = await albumStore.getPhotoRatings(paths);
      const map = { ...ratings.value };
      for (const [p, r] of rows as PhotoRating[]) map[p] = r;
      ratings.value = map;
    } catch {
      /* 评分读取失败按未评分处理 */
    }
    await photoActions.loadMarkStates(paths);
  },
  { immediate: true },
);

/* ---------------- 动作 ---------------- */

/** 关闭菜单 + 通知调用方同步 target */
function close() {
  props.onClose?.();
}

/** 变更后统一刷新（内部先关菜单，避免刷新后菜单还挂着） */
async function afterChange() {
  close();
  await props.onRefresh?.();
}

/** 归档 / 隐藏：虚拟相册内为「取消」，普通相册内为「设置」 */
async function toggleMark(kind: PhotoMarkState, path: string) {
  const marked = !(targetMark.value === kind);
  const ok = await photoActions.apply(kind, [path], marked);
  if (ok) await afterChange();
}

async function onQuickRating(n: number) {
  const dlg = ratingDlg.value;
  ratingDlg.value = null;
  if (!dlg) return;
  try {
    await albumStore.setPhotoRatings([dlg.path], n);
    ratings.value = { ...ratings.value, [dlg.path]: n };
    notify.success(n ? `已评 ${n} 星` : "已清除评分");
    close();
  } catch (e) {
    notify.error("保存评分失败", String(e));
  }
}

async function onQuickTag() {
  const dlg = tagDlg.value;
  tagDlg.value = null;
  if (!dlg) return;
  const t = dlg.text.trim();
  if (!t) return;
  try {
    const cur = await invoke<string[]>("get_photo_tags", { path: dlg.path });
    if (cur.includes(t)) {
      notify.info("该标签已存在", `"${t}" 已在照片标签中`);
      return;
    }
    if (cur.length >= 20) {
      notify.warning("标签数量已达上限", "每张照片最多 20 个标签");
      return;
    }
    await invoke<string[]>("set_photo_tags", { path: dlg.path, tags: [...cur, t] });
    notify.success("标签已添加", `「${t}」（共 ${cur.length + 1} 个）`);
    close();
  } catch (e) {
    notify.error("保存标签失败", String(e));
  }
}

/** FEAT-095（R7 / D1）：删除二次确认，两种模式 */
function openDeleteDialog() {
  if (!targetPaths.value.length) return;
  deleteDlg.value = { paths: [...targetPaths.value] };
}

async function pickDeleteMode(mode: DeleteMode) {
  const dlg = deleteDlg.value;
  deleteDlg.value = null;
  if (!dlg?.paths.length) return;
  deleting.value = true;
  const cmd = mode === "records" ? "delete_photo_records_by_paths" : "delete_photos_to_trash";
  try {
    const outcome = await invoke<{
      requested: number;
      deleted: number;
      failed: number;
      failed_paths: string[];
    }>(cmd, { paths: dlg.paths });
    if (outcome.failed > 0) {
      notify.warning(
        `已删除 ${outcome.deleted} / ${outcome.requested} 张`,
        `失败：${outcome.failed_paths.slice(0, 3).join("、")}`,
      );
    } else {
      notify.success(
        `已删除 ${outcome.deleted} 张`,
        mode === "trash" ? "文件已移入系统回收站，可找回" : "本地文件保留，仅清除记录",
      );
    }
    await afterChange();
  } catch (e) {
    notify.error("删除失败", String(e));
  } finally {
    deleting.value = false;
  }
}

/* ---------------- 菜单条目 ---------------- */

const items = computed<ContextMenuEntry[]>(() => {
  const path = targetPath.value;
  if (!path) return [];
  const isArchived = targetMark.value === "archived";
  const isHidden = targetMark.value === "hidden";
  const entries: ContextMenuEntry[] = [
    {
      label: "打分…",
      icon: "★",
      onClick: () => {
        ratingDlg.value = { path, value: ratings.value[path] || 5 };
      },
    },
    {
      label: "添加 / 编辑标签…",
      icon: "🏷",
      onClick: () => {
        tagDlg.value = { path, text: "" };
      },
    },
    {
      label: "在创意工坊编辑",
      icon: "✏️",
      onClick: () => photoActions.openInWorkshop(path),
    },
  ];
  // 调用方自定义条目（插在分隔线之后，便于与通用项区分）
  if (props.extraItems?.length) {
    entries.push({ divider: true }, ...props.extraItems);
  }
  entries.push(
    { divider: true },
    {
      label: inVirtual.value
        ? isHidden
          ? "取消隐藏"
          : "取消归档"
        : "归档",
      icon: "📦",
      onClick: () => toggleMark("archived", path),
    },
    {
      label: inVirtual.value
        ? isArchived
          ? "取消归档"
          : "取消隐藏"
        : "隐藏",
      icon: "🔒",
      onClick: () => toggleMark("hidden", path),
    },
    {
      label: "删除…",
      icon: "🗑",
      danger: true,
      onClick: openDeleteDialog,
    },
  );
  return entries;
});
</script>

<template>
  <!-- 右键菜单 -->
  <ContextMenu
    v-if="target"
    :items="items"
    :x="target.x"
    :y="target.y"
    @close="close"
  />

  <!-- 快速打分 -->
  <Teleport to="body">
    <div v-if="ratingDlg" class="pam-mask" @click.self="ratingDlg = null">
      <div class="pam-dialog" role="dialog" aria-modal="true">
        <h4>给这张照片打分</h4>
        <div class="pam-stars">
          <button
            v-for="n in 5"
            :key="n"
            class="pam-star"
            :class="{ on: n <= ratingDlg.value }"
            :title="`${n} 星`"
            @click="onQuickRating(n)"
          >★</button>
        </div>
        <div class="pam-actions">
          <button class="btn" @click="ratingDlg = null">取消</button>
          <button class="btn" @click="onQuickRating(0)">清除评分</button>
        </div>
      </div>
    </div>
  </Teleport>

  <!-- 快速标签 -->
  <Teleport to="body">
    <div v-if="tagDlg" class="pam-mask" @click.self="tagDlg = null">
      <div class="pam-dialog" role="dialog" aria-modal="true">
        <h4>添加标签</h4>
        <input
          v-model="tagDlg.text"
          class="pam-input"
          placeholder="输入标签后回车（每张照片最多 20 个）"
          maxlength="30"
          @keydown.enter.prevent="onQuickTag"
          @keydown.esc="tagDlg = null"
        />
        <div class="pam-actions">
          <button class="btn" @click="tagDlg = null">取消</button>
          <button class="btn btn-primary" @click="onQuickTag">保存</button>
        </div>
      </div>
    </div>
  </Teleport>

  <!-- FEAT-095（R7 / D1）：删除二次确认（相册记录删除 / 本地删除移入回收站） -->
  <PhotoDeleteDialog
    :paths="deleteDlg?.paths ?? null"
    :in-virtual-album="inVirtual"
    :busy="deleting"
    @pick="pickDeleteMode"
    @cancel="deleteDlg = null"
  />
</template>

<style scoped>
.pam-mask {
  position: fixed;
  inset: 0;
  z-index: 1250;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
}
.pam-dialog {
  width: min(400px, 92vw);
  border-radius: 14px;
  padding: 18px;
  background: var(--color-surface, #fff);
  color: var(--color-text, #2c3e50);
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.28);
}
.pam-dialog h4 {
  margin: 0 0 12px;
  font-size: 15px;
}
.pam-stars {
  display: flex;
  gap: 6px;
  justify-content: center;
  padding: 4px 0 10px;
}
.pam-star {
  background: none;
  border: none;
  color: rgba(127, 127, 127, 0.4);
  font-size: 32px;
  line-height: 1;
  cursor: pointer;
  transition: color 0.12s, transform 0.1s;
}
.pam-star.on { color: #ffce3a; }
.pam-star:hover { transform: scale(1.12); }
.pam-input {
  width: 100%;
  font-size: 13px;
  padding: 7px 11px;
  border: 1px solid var(--color-primary);
  border-radius: 8px;
  outline: none;
  background: var(--color-surface, #fff);
  color: var(--color-text, #2c3e50);
}
.pam-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  margin-top: 14px;
}
.pam-actions .btn {
  padding: 7px 16px;
  font-size: 13px;
  border-radius: 8px;
  border: 1px solid var(--color-border, #ddd);
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.pam-actions .btn-primary {
  background: var(--color-primary);
  color: var(--color-on-primary);
  border-color: var(--color-primary);
}
</style>
