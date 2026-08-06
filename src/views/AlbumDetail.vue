<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useAlbumStore } from "../stores/album";
import type { Album } from "../types/album";
import { formatSize } from "../types/album";
import { trace } from "../utils/trace";
import ConfirmDialog from "../components/ConfirmDialog.vue";

const route = useRoute();
const router = useRouter();
const store = useAlbumStore();

/** 将本地文件路径转为前端可访问的 URL（Tauri asset 协议） */
function fileUrl(path: string | null): string {
  return path ? convertFileSrc(path) : "";
}

const albumId = Number(route.params.id);
const loadError = ref(false);
const settingCover = ref(false);

/** 在系统文件管理器中打开相册文件夹内部 */
const openAlbumPath = trace("openAlbumPath", async (path: string) => {
  try {
    await invoke("open_folder", { path });
  } catch (e) {
    alert(`无法打开文件夹：${path}\n\n${e}`);
  }
});

/** 点击封面区域：选择图片作为封面（需求 §6.2 图片选择对话框） */
const chooseCover = trace("chooseCover", async () => {
  if (settingCover.value) return;
  try {
    const selected = await open({
      directory: false,
      multiple: false,
      title: "选择封面图片",
      defaultPath: store.currentAlbum?.path ?? undefined,
      filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp"] }],
    });
    if (typeof selected === "string") {
      settingCover.value = true;
      const updated = await invoke<Album>("set_cover", {
        id: albumId,
        imagePath: selected,
      });
      store.currentAlbum = updated;
      await store.fetchAlbums(); // 刷新列表，封面同步
    }
  } catch (e) {
    alert(`设置封面失败：${e}`);
  } finally {
    settingCover.value = false;
  }
});

const load = trace("load", async () => {
  try {
    console.log("[DETAIL] load: albumId=", albumId);
    await store.fetchAlbum(albumId);
    console.log("[DETAIL] load result: folder_id=", store.currentAlbum?.folder_id, "folder_path=", store.currentAlbum?.folder_path);
    loadError.value = false;
  } catch {
    loadError.value = true;
  }
});

function goBack() {
  router.push("/albums");
}

/** 跳转到所属父相册（手动排序分组） */
function jumpToParentFolder() {
  const fid = store.currentAlbum?.folder_id;
  if (fid == null) return;
  // 跳转相册列表页，带参数切换到手动排序并定位到该分组
  router.push({ path: "/albums", query: { sort: "manual", folder: String(fid) } });
}

/** 删除相册（仅删数据库记录，不删本地照片），删除后返回列表 */
const deleting = ref(false);
/** 自定义二次确认弹窗状态 */
const showDeleteConfirm = ref(false);
const deleteConfirmMessage = ref("");
const deleteAlbum = trace("deleteAlbum", async () => {
  if (deleting.value) return;
  const name = store.currentAlbum?.name ?? "";
  deleteConfirmMessage.value =
    `确定要删除相册「${name}」吗？\n\n此操作仅删除相册记录，不会删除本地照片文件。`;
  showDeleteConfirm.value = true;
});
/** 确认后真正执行删除 */
const doDelete = async () => {
  showDeleteConfirm.value = false;
  if (deleting.value) return;
  deleting.value = true;
  try {
    await store.deleteAlbum(albumId);
    alert("相册已删除");
    router.push("/albums");
  } catch (e) {
    alert(`删除失败：${e}`);
  } finally {
    deleting.value = false;
  }
};

/** 地点标签编辑 */
const editingLocation = ref(false);
const locationInput = ref("");
const savingLocation = ref(false);

function startEditLocation() {
  locationInput.value = store.currentAlbum?.location ?? "";
  editingLocation.value = true;
}

const saveLocation = trace("saveLocation", async () => {
  if (savingLocation.value) return;
  savingLocation.value = true;
  try {
    await store.updateAlbum({
      id: albumId,
      location: locationInput.value.trim(),
    });
    // 更新本地显示
    if (store.currentAlbum) {
      store.currentAlbum.location =
        locationInput.value.trim() || null;
    }
    editingLocation.value = false;
  } catch (e) {
    alert(`保存地点失败：${e}`);
  } finally {
    savingLocation.value = false;
  }
});

/** 相册标签编辑（最多 5 个） */
const editingTags = ref(false);
const tagInput = ref("");
const editTags = ref<string[]>([]);
const savingTags = ref(false);

function startEditTags() {
  editTags.value = [...(store.currentAlbum?.tags ?? [])];
  tagInput.value = "";
  editingTags.value = true;
}

function addTag() {
  const t = tagInput.value.trim();
  if (!t) return;
  if (editTags.value.length >= 5) {
    alert("最多只能添加 5 个标签");
    return;
  }
  if (!editTags.value.includes(t)) {
    editTags.value.push(t);
  }
  tagInput.value = "";
}

function removeEditTag(index: number) {
  editTags.value.splice(index, 1);
}

const saveTags = trace("saveTags", async () => {
  if (savingTags.value) return;
  savingTags.value = true;
  try {
    await invoke("update_album_tags", { albumId, tags: editTags.value });
    if (store.currentAlbum) {
      store.currentAlbum.tags = [...editTags.value];
    }
    editingTags.value = false;
  } catch (e) {
    alert(`保存标签失败：${e}`);
  } finally {
    savingTags.value = false;
  }
});

onMounted(load);
</script>

<template>
  <div class="detail-page">
    <!-- 顶部导航栏（需求 §5.3） -->
    <nav class="detail-nav">
      <button class="btn" @click="goBack">← 返回相册列表</button>
      <div class="nav-actions">
        <button class="btn btn-danger" :disabled="deleting" @click="deleteAlbum">
          {{ deleting ? "删除中…" : "删除相册" }}
        </button>
      </div>
    </nav>

    <!-- 加载失败提示 -->
    <div v-if="loadError" class="not-found">
      <p>相册不存在或已删除</p>
      <button class="btn btn-primary" @click="goBack">返回列表</button>
    </div>

    <template v-else-if="store.currentAlbum">
      <!-- 信息头区域 -->
      <div class="detail-header">
        <div class="cover-large cover-clickable" @click="chooseCover">
          <img v-if="store.currentAlbum.cover_path" :src="fileUrl(store.currentAlbum.cover_path)" alt="封面" />
          <div v-else class="cover-placeholder">📷</div>
          <div class="cover-overlay">{{ settingCover ? "设置中…" : "点击更换封面" }}</div>
        </div>
        <div class="detail-info">
          <h1 class="detail-name">{{ store.currentAlbum.name }}</h1>
          <p class="detail-path">
            <span
              class="path-link"
              title="在文件资源管理器中打开"
              @click="openAlbumPath(store.currentAlbum.path)"
            >
              📁 {{ store.currentAlbum.path }}
            </span>
          </p>
          <p class="detail-desc">{{ store.currentAlbum.description || "暂无说明" }}</p>

          <!-- 父相册（所属分组）归属，点击跳转到手动排序对应分组 -->
          <p v-if="store.currentAlbum.folder_id != null" class="detail-parent">
            <span class="parent-label">所属相册：</span>
            <span class="parent-path" :title="`跳转到父相册 ${store.currentAlbum.folder_path}`" @click="jumpToParentFolder">
              📁 {{ store.currentAlbum.folder_path || "父相册" }}
            </span>
          </p>

          <!-- 统计属性 -->
          <div class="detail-stats">
            <!-- 地点标签（可点击编辑） -->
            <div v-if="!editingLocation" class="stat-block stat-clickable" @click="startEditLocation" title="点击设置地点">
              <span class="stat-label">地点 📍</span>
              <span class="stat-value">{{ store.currentAlbum.location || "点击设置" }}</span>
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
              <span class="stat-value">{{ store.currentAlbum.photo_count }} 张</span>
            </div>
            <div class="stat-block">
              <span class="stat-label">相册大小</span>
              <span class="stat-value">{{ formatSize(store.currentAlbum.size_bytes) }}</span>
            </div>
            <div class="stat-block">
              <span class="stat-label">拍摄时间</span>
              <span class="stat-value">{{ store.currentAlbum.shoot_time || "未知" }}</span>
            </div>
            <div class="stat-block">
              <span class="stat-label">创建时间</span>
              <span class="stat-value">{{ new Date(store.currentAlbum.created_at * 1000).toLocaleDateString() }}</span>
            </div>
            <div class="stat-block">
              <span class="stat-label">更新时间</span>
              <span class="stat-value">{{ new Date(store.currentAlbum.updated_at * 1000).toLocaleDateString() }}</span>
            </div>
          </div>

          <!-- 相册标签（最多 5 个） -->
          <div class="album-tags">
            <div class="album-tags-head">
              <span class="album-tags-label">相册标签</span>
              <button v-if="!editingTags" class="btn btn-sm" @click="startEditTags">
                {{ store.currentAlbum.tags.length > 0 ? "编辑标签" : "添加标签" }}
              </button>
            </div>
            <!-- 显示模式 -->
            <div v-if="!editingTags" class="album-tags-display">
              <span v-for="t in store.currentAlbum.tags" :key="t" class="tag-chip">{{ t }}</span>
              <span v-if="store.currentAlbum.tags.length === 0" class="tag-empty">暂无标签</span>
            </div>
            <!-- 编辑模式 -->
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

      <!-- 图片浏览占位（需求 §2.3 P1） -->
      <section class="photo-area">
        <p class="photo-hint">图片扫描功能即将上线，敬请期待</p>
      </section>
    </template>

    <!-- 删除相册二次确认 -->
    <ConfirmDialog
      :visible="showDeleteConfirm"
      title="删除相册"
      :message="deleteConfirmMessage"
      @confirm="doDelete"
      @cancel="showDeleteConfirm = false"
    />
  </div>
</template>

<style scoped>
.detail-page {
  max-width: 1200px;
  margin: 0 auto;
  padding: 24px;
  min-height: 100vh;
}

.detail-nav {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 24px;
}

.nav-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.btn {
  padding: 8px 16px;
  border-radius: 8px;
  border: 1px solid #ddd;
  background: #fff;
  cursor: pointer;
  font-size: 14px;
  transition: all 0.2s;
}

.btn:hover {
  border-color: #396cd8;
  color: #396cd8;
}

.btn-danger {
  background: #e5484d;
  color: #fff;
  border-color: #e5484d;
}

.btn-danger:hover {
  background: #d13438;
  color: #fff;
}

.btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
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

.detail-header {
  display: flex;
  gap: 24px;
  align-items: flex-start;
  margin-bottom: 32px;
}

.cover-large {
  position: relative;
  width: 320px;
  height: 220px;
  background: #f0f0f0;
  border-radius: 12px;
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.cover-large img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.cover-clickable {
  cursor: pointer;
}

/* 悬停遮罩提示 */
.cover-overlay {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  color: #fff;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 14px;
  opacity: 0;
  transition: opacity 0.2s;
  pointer-events: none;
}

.cover-clickable:hover .cover-overlay {
  opacity: 1;
}

.cover-placeholder {
  font-size: 48px;
  opacity: 0.4;
}

.detail-info {
  flex: 1;
}

.detail-name {
  margin: 0 0 12px;
  font-size: 28px;
}

.detail-path {
  margin: 0 0 8px;
  color: #555;
  word-break: break-all;
}

.path-link {
  color: #396cd8;
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 2px;
}

.path-link:hover {
  color: #2f5cc2;
}

.detail-desc {
  margin: 0 0 8px;
  color: #666;
  font-size: 15px;
  line-height: 1.6;
}

/* 父相册归属 */
.detail-parent {
  margin: 0 0 8px;
  display: flex;
  align-items: center;
  gap: 6px;
}

.parent-label {
  font-size: 14px;
  color: #888;
}

.parent-path {
  font-size: 14px;
  color: #396cd8;
  cursor: pointer;
  background: #eef3ff;
  padding: 2px 10px;
  border-radius: 10px;
  transition: all 0.2s;
}

.parent-path:hover {
  background: #396cd8;
  color: #fff;
}

.detail-time {
  margin: 0;
  color: #999;
  font-size: 13px;
}

/* 统计属性块 */
.detail-stats {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  margin-top: 16px;
}

.stat-block {
  background: #f5f7fa;
  border-radius: 8px;
  padding: 10px 14px;
  min-width: 120px;
}

.stat-label {
  display: block;
  font-size: 12px;
  color: #999;
  margin-bottom: 4px;
}

.stat-value {
  font-size: 16px;
  font-weight: 600;
  color: #2c3e50;
}

/* 地点标签可点击 */
.stat-clickable {
  cursor: pointer;
  transition: background 0.2s;
}

.stat-clickable:hover {
  background: #eef3ff;
}

.stat-edit {
  background: #eef3ff;
}

.loc-edit-row {
  display: flex;
  align-items: center;
  gap: 6px;
}

.input-sm {
  width: 120px;
  padding: 5px 8px;
  border: 1px solid #ddd;
  border-radius: 6px;
  font-size: 13px;
  outline: none;
}

.input-sm:focus {
  border-color: #396cd8;
}

.btn-sm {
  padding: 5px 10px;
  font-size: 12px;
}

.btn-sm.btn-primary {
  background: #396cd8;
  color: #fff;
  border-color: #396cd8;
}

/* 相册标签区 */
.album-tags {
  margin-top: 16px;
}

.album-tags-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
}

.album-tags-label {
  font-size: 14px;
  color: #555;
  font-weight: 500;
}

.album-tags-display {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}

.tag-chip {
  background: #eef3ff;
  color: #396cd8;
  font-size: 12px;
  padding: 2px 10px;
  border-radius: 10px;
}

.tag-chip.editable {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.tag-del {
  border: none;
  background: none;
  color: #999;
  cursor: pointer;
  font-size: 12px;
  padding: 0;
}

.tag-empty {
  color: #999;
  font-size: 13px;
}

.tag-edit-row {
  display: flex;
  gap: 8px;
  margin-bottom: 8px;
}

.tag-edit-row .input-sm {
  width: 160px;
}

.tag-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  min-height: 28px;
  margin-bottom: 8px;
  align-items: center;
}

.tag-actions {
  display: flex;
  gap: 8px;
}

.photo-area {
  border: 1px dashed #ccc;
  border-radius: 12px;
  padding: 60px 0;
  text-align: center;
  color: #999;
}

.photo-hint {
  margin: 0;
}

.not-found {
  text-align: center;
  padding: 80px 0;
  color: #888;
}
</style>
