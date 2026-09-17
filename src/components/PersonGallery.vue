<script setup lang="ts">
/**
 * 人物画廊 —— 智慧相册「人物」分类的展示组件
 *
 * 数据源：persons.db 直读（list_persons，按脸数降序）；头像本地裁剪缓存。
 * 支持：行内重命名 / 合并到其他人物（二次确认）。
 * 与 ScanPanel 内 PersonPanel 的差异：本组件面向浏览场景，完全离线可用。
 */
import { computed, onMounted, ref, type Directive } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { PersonInfo, PersonPhotoItem } from "../types/photo";
import ConfirmDialog from "./ConfirmDialog.vue";
import PhotoLightbox from "./PhotoLightbox.vue";
import { useAlbumStore } from "../stores/album";
import { useThemeStore } from "../stores/theme";

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

/** 头像 asset URL：附时间戳参数，同名覆盖后仍能刷新显示 */
function avatarUrl(_pid: string, path: string): string {
  return `${convertFileSrc(path)}?t=${avatarTs.value}`;
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

onMounted(load);

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
  exitSelectMode();
}

function openPhoto(i: number) {
  lightboxIndex.value = i;
  lightboxOpen.value = true;
}

/* ---- FEAT-050：多选删除（查看弹窗右上角「☑ 多选」→ 勾选 → 删除按钮 → 两种选择） ---- */
const selectMode = ref(false);
const selected = ref<Set<string>>(new Set());
const modeDialogPaths = ref<string[] | null>(null);

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
function selectAllPhotos() {
  selected.value = new Set(viewingPhotos.value.map((it) => it.path));
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

async function pickMode(mode: "records" | "trash") {
  const paths = modeDialogPaths.value ?? [];
  modeDialogPaths.value = null;
  if (!paths.length) return;
  const cmd = mode === "records" ? "delete_photo_records_by_paths" : "delete_photos_to_trash";
  try {
    const outcome = await invoke<{
      requested: number;
      deleted: number;
      failed: number;
      failed_paths: string[];
    }>(cmd, { paths });
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
    selected.value = new Set();
  }
}

/* ---- FEAT-047：自选头像（照片弹窗内指定一张照片作为头像封面） ---- */
/** 正在设置中的照片路径（防重复点击）；null = 空闲 */
const settingAvatar = ref<string | null>(null);

async function setAvatar(p: PersonInfo, photoPath: string) {
  if (settingAvatar.value) return;
  settingAvatar.value = photoPath;
  try {
    const cachePath = await invoke<string>("set_person_avatar_from_photo", {
      pid: p.id,
      photoPath,
    });
    avatarTs.value = Date.now();
    avatarMap.value = { ...avatarMap.value, [p.id]: avatarUrl(p.id, cachePath) };
    flash(`已将所选照片设为 ${displayName(p)} 的头像`);
  } catch (e) {
    flash(`设置头像失败：${String(e)}`);
  } finally {
    settingAvatar.value = null;
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

    <!-- 人物卡片网格 -->
    <div v-else class="person-grid">
      <article v-for="p in persons" :key="p.id" class="person-card" :style="surfaceStyle" :title="`${displayName(p)}（${p.id}）`">
        <div class="person-avatar-wrap" @click.stop="openPhotos(p)" title="点击查看该人物的照片">
          <img v-if="avatarMap[p.id]" :src="avatarMap[p.id]" class="person-avatar" alt="" />
          <span v-else class="person-avatar person-avatar-fallback">{{ displayName(p).slice(0, 1) }}</span>
          <span class="person-face-count">{{ p.face_count }} 张脸</span>
        </div>
        <div class="person-info">
          <!-- 行内重命名 -->
          <template v-if="editingId === p.id">
            <input
              v-model="editingName"
              class="person-rename-input"
              maxlength="50"
              placeholder="输入新名称"
              @keyup.enter="saveRename(p)"
              @keyup.esc="stopRename"
              v-focus
            />
            <div class="person-edit-actions">
              <button class="mini-btn ok" @click="saveRename(p)">保存</button>
              <button class="mini-btn" @click="stopRename">取消</button>
            </div>
          </template>
          <template v-else>
            <div class="person-name-row">
              <span class="person-name">{{ displayName(p) }}</span>
              <button class="mini-btn" title="重命名" @click="startRename(p)">✎</button>
            </div>
            <div class="person-id mono">{{ p.id }}</div>
            <div class="person-date">登记于 {{ p.created_at }}</div>
            <button class="mini-btn merge-btn" @click="mergingSource = p">合并到其他人物…</button>
          </template>
        </div>
      </article>
    </div>

    <!-- 合并：选择目标人物弹窗 -->
    <Teleport to="body">
      <div v-if="mergingSource && !pendingTarget" class="merge-mask" @click.self="mergingSource = null">
        <div class="merge-dialog" :style="surfaceStyle">
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

    <!-- 查看该人物的照片（缩略图网格） -->
    <Teleport to="body">
      <div v-if="viewingPerson" class="viewer-mask" @click.self="closePhotos">
        <div class="viewer-dialog" :style="surfaceStyle">
          <div class="viewer-head">
            <span class="viewer-title">{{ displayName(viewingPerson) }} 的照片</span>
            <span class="viewer-count">{{ viewingPhotos.length }} 张</span>
            <span class="viewer-spacer"></span>
            <template v-if="selectMode">
              <button class="mini-btn" @click="selectAllPhotos">全选</button>
              <button class="mini-btn danger" :disabled="!selected.size" @click="openDeleteDialog">
                🗑 删除选中（{{ selected.size }}）
              </button>
              <button class="mini-btn" @click="exitSelectMode">退出</button>
            </template>
            <button v-else class="mini-btn" @click="enterSelectMode">☑ 多选</button>
            <button class="btn viewer-close" @click="closePhotos">✕</button>
          </div>
          <div v-if="viewingLoading" class="viewer-state">正在读取缩略图…</div>
          <div v-else-if="viewingError" class="viewer-state viewer-error">{{ viewingError }}</div>
          <div v-else class="viewer-grid">
            <div
              v-for="(it, i) in viewingPhotos"
              :key="it.path"
              class="viewer-cell"
              :class="{ checked: selectMode && selected.has(it.path) }"
              @click="selectMode ? toggleSelect(it.path) : openPhoto(i)"
            >
              <img
                v-if="it.thumb"
                :src="photoThumbSrc(it)"
                class="viewer-photo"
                :title="it.path"
                loading="lazy"
                alt=""
                @click.stop="selectMode ? toggleSelect(it.path) : openPhoto(i)"
              />
              <div v-else class="viewer-photo viewer-photo-missing" :title="`缩略图生成中或原图不可用：${it.path}`">🖼</div>
              <!-- FEAT-047：自选头像入口（多选模式下隐藏） -->
              <button
                v-if="!selectMode"
                class="viewer-set-avatar"
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
          <!-- 原图看图器（启用删除按钮） -->
          <PhotoLightbox
            v-if="lightboxOpen"
            :photos="lightboxPhotos"
            :index="lightboxIndex"
            deletable
            @close="lightboxOpen = false"
            @delete="askDeleteCurrent"
          />
        </div>
      </div>
    </Teleport>

    <!-- FEAT-050：删除方式选择（两种选择即最终确认） -->
    <Teleport to="body">
      <div v-if="modeDialogPaths" class="del-mask" @click.self="modeDialogPaths = null">
        <div class="del-dialog" :style="surfaceStyle">
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

.pg-action-msg {
  background: var(--color-primary-soft, #eef5ff);
  border: 1px solid rgba(57, 108, 216, 0.3);
  color: #2f5bc0;
  border-radius: 8px;
  padding: 8px 12px;
  font-size: 13px;
  margin-bottom: 12px;
}
body.theme-dark .pg-action-msg {
  color: #93b4f5;
}

.pg-state {
  text-align: center;
  padding: 48px 20px;
  opacity: 0.8;
  color: inherit;
}
.pg-error { color: #e5484d; }
.pg-hint {
  opacity: 0.65;
  font-size: 13px;
  margin-top: 8px;
}

.person-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  gap: 16px;
}

.person-card {
  background: transparent; /* 底色由 surfaceStyle(主题) 提供 */
  border-radius: 12px;
  padding: 16px;
  display: flex;
  align-items: center;
  gap: 14px;
  transition: transform 0.15s, box-shadow 0.15s;
}
.person-card:hover {
  transform: translateY(-2px);
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.1);
}

.person-avatar-wrap { position: relative; flex-shrink: 0; }

.person-avatar {
  width: 64px;
  height: 64px;
  border-radius: 50%;
  object-fit: cover;
  display: block;
  border: 2px solid rgba(57, 108, 216, 0.35);
}
.person-avatar-fallback {
  background: #396cd8;
  color: #fff;
  font-size: 26px;
  line-height: 60px;
  text-align: center;
  display: inline-block;
}
.person-face-count {
  position: absolute;
  bottom: -4px;
  left: 50%;
  transform: translateX(-50%);
  background: #396cd8;
  color: #fff;
  font-size: 11px;
  padding: 1px 8px;
  border-radius: 999px;
  white-space: nowrap;
}

.person-info { min-width: 0; flex: 1; }

.person-name-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}
.person-name {
  font-size: 15px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mono { font-family: Consolas, Monaco, monospace; }
.person-id { font-size: 12px; color: #396cd8; margin-top: 2px; }
.person-date { font-size: 11px; opacity: 0.6; margin-top: 4px; }

.mini-btn {
  border: 1px solid currentColor;
  background: transparent;
  border-radius: 6px;
  font-size: 11px;
  padding: 2px 7px;
  cursor: pointer;
  color: inherit;
  transition: all 0.15s;
  flex-shrink: 0;
}
.mini-btn:hover { border-color: #396cd8; color: #396cd8; }
.mini-btn.ok { background: #396cd8; border-color: #396cd8; color: #fff; }
.merge-btn { margin-top: 6px; }

.person-rename-input {
  width: 100%;
  font-size: 13px;
  padding: 3px 8px;
  border: 1px solid #396cd8;
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
  transition: all 0.15s;
}
.merge-item:hover { border-color: #396cd8; background: rgba(57, 108, 216, 0.06); }
.merge-avatar {
  width: 36px;
  height: 36px;
  border-radius: 50%;
  object-fit: cover;
}
.merge-avatar-fallback {
  background: #396cd8;
  color: #fff;
  line-height: 34px;
  text-align: center;
  font-size: 15px;
}
.merge-name { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.merge-id { font-size: 11px; color: #396cd8; }
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
  display: flex;
  align-items: center;
  justify-content: center;
}
.viewer-dialog {
  background: transparent; /* 底色由 surfaceStyle(主题) 提供 */
  border-radius: 16px;
  padding: 20px 22px;
  width: min(720px, 94vw);
  max-height: 86vh;
  display: flex;
  flex-direction: column;
}
.viewer-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 12px;
}
.viewer-title { font-size: 17px; font-weight: 700; }
.viewer-count { font-size: 13px; opacity: 0.7; }
.viewer-close { margin-left: auto; padding: 4px 12px; font-size: 14px; }
.viewer-state { text-align: center; padding: 40px 20px; opacity: 0.8; }
.viewer-error { color: #e5484d; }
.viewer-grid {
  overflow-y: auto;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
  gap: 8px;
}
.viewer-cell { position: relative; }

/* FEAT-050：多选删除 */
.viewer-spacer {
  flex: 1;
}
.mini-btn.danger {
  color: #e03131;
  border-color: rgba(224, 49, 49, 0.5);
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
.del-mask {
  position: fixed;
  inset: 0;
  z-index: 1250;
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
  transition: opacity 0.15s;
}
.viewer-cell:hover .viewer-set-avatar,
.viewer-set-avatar:focus-visible {
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
  transition: transform 0.15s;
  display: block;
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
