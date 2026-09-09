<script setup lang="ts">
import { ref } from "vue";
import { useRouter } from "vue-router";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useAlbumStore } from "../stores/album";
import type { Album } from "../types/album";
import { formatSize } from "../types/album";
import { trace } from "../utils/trace";
import { useNotify } from "../composables/useNotify";

const props = defineProps<{ album: Album; settingCover: boolean }>();
const emit = defineEmits<{
  "update:album": [album: Album];
  delete: [];
}>();
const router = useRouter();
const store = useAlbumStore();
const notify = useNotify();

function fileUrl(path: string | null): string {
  return path ? convertFileSrc(path) : "";
}

/** 在系统文件管理器中打开相册文件夹 */
const openAlbumPath = trace("openAlbumPath", async (path: string) => {
  try {
    await invoke("open_folder", { path });
  } catch (e) {
    notify.error("无法打开文件夹", `${path}\n${e}`);
  }
});

/** 选择封面图片 */
const chooseCover = trace("chooseCover", async () => {
  if (props.settingCover) return;
  try {
    const selected = await open({
      directory: false,
      multiple: false,
      title: "选择封面图片",
      defaultPath: props.album.path ?? undefined,
      filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp"] }],
    });
    if (typeof selected === "string") {
      const updated = await invoke<Album>("set_cover", {
        id: props.album.id,
        imagePath: selected,
      });
      emit("update:album", updated);
      await store.fetchAlbums();
      notify.success("封面设置成功");
    }
  } catch (e) {
    notify.error("设置封面失败", String(e));
  }
});

/** 跳转到所属父相册 */
function jumpToParentFolder() {
  const fid = props.album.folder_id;
  if (fid == null) return;
  router.push({ path: "/albums", query: { sort: "manual", folder: String(fid) } });
}

// ---- 名称编辑 ----
const editingName = ref(false);
const nameInput = ref("");
const savingName = ref(false);

function startEditName() {
  nameInput.value = props.album.name ?? "";
  editingName.value = true;
}

const saveName = trace("saveName", async () => {
  if (savingName.value) return;
  const name = nameInput.value.trim();
  if (!name) { notify.warning("相册名称不能为空"); return; }
  if (name.length > 100) { notify.warning("相册名称不能超过 100 个字符"); return; }
  savingName.value = true;
  try {
    await store.renameAlbum(props.album.id, name, true);
    editingName.value = false;
    notify.success("名称保存成功");
  } catch (e) {
    notify.error("保存名称失败", String(e));
  } finally {
    savingName.value = false;
  }
});

// ---- 说明编辑 ----
const editingDesc = ref(false);
const descInput = ref("");
const savingDesc = ref(false);

function startEditDesc() {
  descInput.value = props.album.description ?? "";
  editingDesc.value = true;
}

const saveDesc = trace("saveDesc", async () => {
  if (savingDesc.value) return;
  savingDesc.value = true;
  try {
    await store.updateAlbum({ id: props.album.id, description: descInput.value.trim() });
    editingDesc.value = false;
    notify.success("说明保存成功");
  } catch (e) {
    notify.error("保存说明失败", String(e));
  } finally {
    savingDesc.value = false;
  }
});

// ---- 地点编辑 ----
const editingLocation = ref(false);
const locationInput = ref("");
const savingLocation = ref(false);
const detectingLocation = ref(false);

const autoDetectLocation = trace("autoDetectLocation", async () => {
  if (detectingLocation.value) return;
  detectingLocation.value = true;
  try {
    const r = await invoke<{ location: string; changed: boolean; lat: number; lon: number }>(
      "auto_detect_album_location",
      { albumId: props.album.id, force: false }
    );
    if (r.changed) {
      const updated = { ...props.album, location: r.location };
      emit("update:album", updated);
    }
    notify.success("自动识别地点成功", `${r.location}（${r.lat.toFixed(4)}, ${r.lon.toFixed(4)}）`);
  } catch (e) {
    notify.error("自动识别地点失败", String(e));
  } finally {
    detectingLocation.value = false;
  }
});

function startEditLocation() {
  locationInput.value = props.album.location ?? "";
  editingLocation.value = true;
}

const saveLocation = trace("saveLocation", async () => {
  if (savingLocation.value) return;
  savingLocation.value = true;
  try {
    await store.updateAlbum({ id: props.album.id, location: locationInput.value.trim() });
    const updated = { ...props.album, location: locationInput.value.trim() || null };
    emit("update:album", updated);
    editingLocation.value = false;
    notify.success("地点保存成功");
  } catch (e) {
    notify.error("保存地点失败", String(e));
  } finally {
    savingLocation.value = false;
  }
});

// ---- 标签编辑 ----
const editingTags = ref(false);
const tagInput = ref("");
const editTags = ref<string[]>([]);
const savingTags = ref(false);

function startEditTags() {
  editTags.value = [...(props.album.tags ?? [])];
  tagInput.value = "";
  editingTags.value = true;
}

function addTag() {
  const t = tagInput.value.trim();
  if (!t) return;
  if (editTags.value.length >= 5) { notify.warning("最多只能添加 5 个标签"); return; }
  if (!editTags.value.includes(t)) editTags.value.push(t);
  tagInput.value = "";
}

function removeEditTag(index: number) {
  editTags.value.splice(index, 1);
}

const saveTags = trace("saveTags", async () => {
  if (savingTags.value) return;
  savingTags.value = true;
  try {
    await invoke("update_album_tags", { albumId: props.album.id, tags: editTags.value });
    const updated = { ...props.album, tags: [...editTags.value] };
    emit("update:album", updated);
    editingTags.value = false;
    notify.success("标签保存成功");
  } catch (e) {
    notify.error("保存标签失败", String(e));
  } finally {
    savingTags.value = false;
  }
});
</script>

<template>
  <div class="detail-header">
    <div class="cover-large cover-clickable" @click="chooseCover">
      <img v-if="album.cover_path" :src="fileUrl(album.cover_path)" alt="封面" />
      <div v-else class="cover-placeholder">📷</div>
      <div class="cover-overlay">{{ settingCover ? "设置中…" : "点击更换封面" }}</div>
    </div>
    <div class="detail-info">
      <!-- 名称 -->
      <div v-if="!editingName" class="detail-name-wrap" @click="startEditName" title="点击编辑名称">
        <h1 class="detail-name">{{ album.name }}</h1>
        <span class="name-edit-hint">✏️</span>
      </div>
      <div v-else class="name-edit">
        <input
          v-model="nameInput"
          class="name-input"
          maxlength="100"
          placeholder="相册名称"
          @keydown.enter="saveName"
          @keydown.esc="editingName = false"
        />
        <div class="name-edit-actions">
          <button class="btn btn-sm btn-primary" :disabled="savingName" @click="saveName">
            {{ savingName ? "保存中…" : "保存" }}
          </button>
          <button class="btn btn-sm" @click="editingName = false">取消</button>
        </div>
      </div>
      <p class="detail-path">
        <span class="path-link" title="在文件资源管理器中打开" @click="openAlbumPath(album.path)">
          📁 {{ album.path }}
        </span>
      </p>

      <!-- 说明 -->
      <div v-if="!editingDesc" class="detail-desc-wrap" @click="startEditDesc" title="点击编辑说明">
        <p class="detail-desc">{{ album.description || "暂无说明" }}</p>
        <span class="desc-edit-hint">✏️ 点击编辑</span>
      </div>
      <div v-else class="desc-edit">
        <textarea
          v-model="descInput"
          class="desc-textarea"
          rows="3"
          maxlength="500"
          placeholder="介绍一下这个相册的内容…"
        ></textarea>
        <div class="desc-edit-actions">
          <button class="btn btn-sm btn-primary" :disabled="savingDesc" @click="saveDesc">
            {{ savingDesc ? "保存中…" : "保存" }}
          </button>
          <button class="btn btn-sm" @click="editingDesc = false">取消</button>
        </div>
      </div>

      <!-- 父相册归属 -->
      <p v-if="album.folder_id != null" class="detail-parent">
        <span class="parent-label">所属相册：</span>
        <span class="parent-path" :title="`跳转到父相册 ${album.folder_path}`" @click="jumpToParentFolder">
          📁 {{ album.folder_path || "父相册" }}
        </span>
      </p>

      <!-- 统计属性 -->
      <div class="detail-stats">
        <div v-if="!editingLocation" class="stat-block stat-clickable" @click="startEditLocation" title="点击设置地点">
          <span class="stat-label">地点 📍</span>
          <span class="stat-value">{{ album.location || "点击设置" }}</span>
          <button class="btn btn-sm btn-ghost loc-auto-btn" :disabled="detectingLocation" @click.stop="autoDetectLocation">
            {{ detectingLocation ? "识别中…" : "自动识别" }}
          </button>
        </div>
        <div v-else class="stat-block stat-edit">
          <span class="stat-label">地点</span>
          <div class="loc-edit-row">
            <input v-model="locationInput" class="input-sm" placeholder="如：北京 / 巴黎" maxlength="50" />
            <button class="btn btn-sm btn-primary" :disabled="savingLocation" @click="saveLocation">保存</button>
            <button class="btn btn-sm" @click="editingLocation = false">取消</button>
          </div>
        </div>
        <div class="stat-block">
          <span class="stat-label">照片数量</span>
          <span class="stat-value">{{ album.photo_count }} 张</span>
        </div>
        <div class="stat-block">
          <span class="stat-label">相册大小</span>
          <span class="stat-value">{{ formatSize(album.size_bytes) }}</span>
        </div>
        <div class="stat-block">
          <span class="stat-label">拍摄时间</span>
          <span class="stat-value">{{ album.shoot_time || "未知" }}</span>
        </div>
        <div class="stat-block">
          <span class="stat-label">创建时间</span>
          <span class="stat-value">{{ new Date(album.created_at * 1000).toLocaleDateString() }}</span>
        </div>
        <div class="stat-block">
          <span class="stat-label">更新时间</span>
          <span class="stat-value">{{ new Date(album.updated_at * 1000).toLocaleDateString() }}</span>
        </div>
      </div>

      <!-- 相册标签 -->
      <div class="album-tags">
        <div class="album-tags-head">
          <span class="album-tags-label">相册标签</span>
          <button v-if="!editingTags" class="btn btn-sm" @click="startEditTags">
            {{ album.tags.length > 0 ? "编辑标签" : "添加标签" }}
          </button>
        </div>
        <div v-if="!editingTags" class="album-tags-display">
          <span v-for="t in album.tags" :key="t" class="tag-chip">{{ t }}</span>
          <span v-if="album.tags.length === 0" class="tag-empty">暂无标签</span>
        </div>
        <div v-else class="album-tags-edit">
          <div class="tag-edit-row">
            <input v-model="tagInput" class="input-sm" placeholder="输入标签后回车/点添加" maxlength="20"
                   @keyup.enter="addTag" />
            <button class="btn btn-sm" @click="addTag">添加</button>
          </div>
          <div class="tag-list">
            <span v-for="(t, i) in editTags" :key="i" class="tag-chip editable">
              {{ t }} <button class="tag-del" @click="removeEditTag(i)">×</button>
            </span>
            <span v-if="editTags.length === 0" class="tag-empty">暂无标签</span>
          </div>
          <div class="tag-actions">
            <button class="btn btn-sm btn-primary" :disabled="savingTags" @click="saveTags">保存</button>
            <button class="btn btn-sm" @click="editingTags = false">取消</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.detail-header {
  display: flex;
  gap: 20px;
  margin-bottom: 24px;
}

.cover-large {
  flex-shrink: 0;
  width: 200px;
  height: 150px;
  border-radius: 8px;
  overflow: hidden;
  position: relative;
  background: #f0f0f0;
}

.cover-large img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.cover-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 40px;
}

.cover-clickable {
  cursor: pointer;
}

.cover-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0,0,0,0.45);
  color: #fff;
  font-size: 14px;
  opacity: 0;
  transition: opacity 0.2s;
}

.cover-clickable:hover .cover-overlay {
  opacity: 1;
}

.cover-actions {
  margin-top: 10px;
  display: flex;
  gap: 8px;
}

.detail-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.detail-name-wrap {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
}

.detail-name {
  font-size: 24px;
  font-weight: 700;
  margin: 0;
  color: var(--color-text);
}

.name-edit-hint {
  font-size: 16px;
  opacity: 0;
  transition: opacity 0.15s;
}

.detail-name-wrap:hover .name-edit-hint {
  opacity: 0.6;
}

.name-edit {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.name-input {
  font-size: 16px;
  padding: 8px 12px;
  border: 1px solid #396cd8;
  border-radius: 6px;
  outline: none;
  max-width: 400px;
}

.name-edit-actions {
  display: flex;
  gap: 6px;
}

.detail-path {
  font-size: 12px;
  color: var(--color-text-2);
  margin: 0;
}

.path-link {
  cursor: pointer;
  text-decoration: underline dotted;
}

.path-link:hover {
  color: #396cd8;
}

.detail-desc-wrap {
  cursor: pointer;
  position: relative;
}

.detail-desc {
  margin: 0;
  font-size: 13px;
  color: var(--color-text-2);
  line-height: 1.5;
}

.desc-edit-hint {
  font-size: 11px;
  color: #396cd8;
  opacity: 0;
  transition: opacity 0.15s;
}

.detail-desc-wrap:hover .desc-edit-hint {
  opacity: 1;
}

.desc-edit {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.desc-textarea {
  font-size: 13px;
  padding: 8px 12px;
  border: 1px solid #396cd8;
  border-radius: 6px;
  outline: none;
  resize: vertical;
  max-width: 500px;
}

.desc-edit-actions {
  display: flex;
  gap: 6px;
}

.detail-parent {
  font-size: 12px;
  color: var(--color-text-2);
  margin: 0;
}

.parent-label {
  color: var(--color-text-2);
}

.parent-path {
  cursor: pointer;
  text-decoration: underline dotted;
}

.parent-path:hover {
  color: #396cd8;
}

.detail-stats {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.stat-block {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  background: rgba(120, 130, 150, 0.08);
  border-radius: 6px;
  font-size: 12px;
}

.stat-label {
  color: var(--color-text-2);
  font-weight: 500;
}

.stat-value {
  color: var(--color-text);
  font-weight: 600;
}

.stat-clickable {
  cursor: pointer;
  transition: background 0.12s;
}

.stat-clickable:hover {
  background: #eef3fb;
}

.stat-edit {
  display: flex;
  gap: 6px;
  align-items: center;
  padding: 4px 8px;
}

.loc-edit-row {
  display: flex;
  gap: 4px;
  align-items: center;
}

.loc-auto-btn {
  margin-left: 4px;
  font-size: 11px;
}

.input-sm {
  padding: 4px 8px;
  border: 1px solid var(--color-border);
  border-radius: 4px;
  font-size: 12px;
  outline: none;
  max-width: 160px;
  background: var(--color-surface);
  color: var(--color-text);
}

.input-sm:focus {
  border-color: #396cd8;
}

/* 标签 */
.album-tags {
  margin-top: 4px;
}

.album-tags-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 6px;
}

.album-tags-label {
  font-size: 11px;
  font-weight: 600;
  color: var(--color-text-2);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.album-tags-display {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.tag-chip {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 4px;
  background: #eef3fb;
  color: #396cd8;
  font-size: 12px;
  font-weight: 500;
}

.tag-chip.editable {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.tag-del {
  background: transparent;
  border: none;
  cursor: pointer;
  font-size: 14px;
  color: var(--color-text-2);
  padding: 0;
  line-height: 1;
}

.tag-del:hover {
  color: #e5484d;
}

.tag-empty {
  font-size: 12px;
  color: var(--color-text-2);
}

.album-tags-edit {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.tag-edit-row {
  display: flex;
  gap: 4px;
  align-items: center;
}

.tag-list {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.tag-actions {
  display: flex;
  gap: 6px;
}

/* 按钮复用 */
.btn {
  padding: 8px 16px;
  border-radius: 8px;
  border: 1px solid var(--color-border);
  background: var(--color-surface);
  cursor: pointer;
  font-size: 14px;
  transition: all 0.2s;
  color: var(--color-text);
}

.btn:hover {
  border-color: #396cd8;
  color: #396cd8;
}

.btn-sm {
  padding: 4px 10px;
  font-size: 12px;
  border-radius: 6px;
}

.btn-primary {
  background: #396cd8;
  color: #fff;
  border-color: #396cd8;
}

.btn-primary:hover {
  background: #2f5cc2;
  color: #fff;
}

.btn-ghost {
  background: transparent;
  color: #396cd8;
  border-color: #396cd8;
}

.btn-ghost:hover {
  background: rgba(57, 108, 216, 0.08);
  color: #2f5cc2;
}

.btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
</style>