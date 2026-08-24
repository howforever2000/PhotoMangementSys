<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAlbumStore } from "../stores/album";
import { trace } from "../utils/trace";
import ConfirmDialog from "../components/ConfirmDialog.vue";
import AlbumMeta from "../components/AlbumMeta.vue";
import CollapseSection from "../components/CollapseSection.vue";
import ContentSearch from "../components/ContentSearch.vue";
import ScanPanel from "../components/ScanPanel.vue";
import PhotoGrid from "../components/PhotoGrid.vue";

const route = useRoute();
const router = useRouter();
const store = useAlbumStore();

const albumId = Number(route.params.id);
const loadError = ref(false);
const settingCover = ref(false);
const deleting = ref(false);
const showDeleteConfirm = ref(false);
const deleteConfirmMessage = ref("");

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

const deleteAlbum = trace("deleteAlbum", async () => {
  if (deleting.value) return;
  const name = store.currentAlbum?.name ?? "";
  deleteConfirmMessage.value =
    `确定要删除相册「${name}」吗？\n\n此操作仅删除相册记录，不会删除本地照片文件。`;
  showDeleteConfirm.value = true;
});

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

function handleAlbumUpdate(updated: any) {
  store.currentAlbum = updated;
}

onMounted(load);
</script>

<template>
  <div class="detail-page">
    <!-- 顶部导航栏 -->
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
      <!-- 相册信息头 -->
      <AlbumMeta
        :album="store.currentAlbum"
        :setting-cover="settingCover"
        @update:album="handleAlbumUpdate"
        @delete="deleteAlbum"
      />

      <!-- 三个功能区统一用 CollapseSection 折叠：搜索 / 扫描在前，缩略图浏览在最后 -->
      <CollapseSection title="🔍 照片搜索" storage-key="detail-search">
        <ContentSearch :album-id="albumId" />
      </CollapseSection>

      <!-- 扫描面板：组合扫描（EXIF/影调/AI 统一入口，后台执行）+ 人物 -->
      <CollapseSection
        title="🧩 组合扫描"
        subtitle="EXIF / 影调 / AI 统一入口 · 后台执行 · 退出页面不中断"
        storage-key="detail-scan"
      >
        <ScanPanel
          :album-id="albumId"
          :album-path="store.currentAlbum.path"
        />
      </CollapseSection>

      <!-- 照片网格浏览：相册内照片一览 + 大图查看（放在最后，作为浏览区） -->
      <CollapseSection title="🖼️ 缩略图浏览" storage-key="detail-photos">
        <PhotoGrid :album-id="albumId" />
      </CollapseSection>
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

.not-found {
  text-align: center;
  padding: 60px 20px;
  color: #667085;
}

.not-found p {
  font-size: 16px;
  margin-bottom: 16px;
}
</style>