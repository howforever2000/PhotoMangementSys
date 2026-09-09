<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAlbumStore } from "../stores/album";
import { useThemeStore } from "../stores/theme";
import { trace } from "../utils/trace";
import ConfirmDialog from "../components/ConfirmDialog.vue";
import AlbumMeta from "../components/AlbumMeta.vue";
import CollapseSection from "../components/CollapseSection.vue";
import ContentSearch from "../components/ContentSearch.vue";
import BreadcrumbNav from "../components/BreadcrumbNav.vue";
import ScanPanel from "../components/ScanPanel.vue";
import PhotoGrid from "../components/PhotoGrid.vue";
import { useNotify } from "../composables/useNotify";

const route = useRoute();
const router = useRouter();
const store = useAlbumStore();
const theme = useThemeStore();
const notify = useNotify();

/** 页面级主题变量：本页按钮/文本原先写死浅色（#fff / #ddd / #2c3e50），
 *  深色模式下会变成"深底 + 白按钮 + 深色字"的刺眼组合。
 *  这里统一以 `--detail-*` 下发，样式块消费变量，浅色模式保持原外观。 */
const detailVars = computed(() => {
  const dark = theme.isDark;
  return {
    "--detail-text": dark ? "#f5f7ff" : "#2c3e50",
    "--detail-muted": dark ? "rgba(214,221,240,.62)" : "#667085",
    "--detail-btn-bg": dark ? "rgba(255,255,255,.06)" : "#fff",
    "--detail-btn-border": dark ? "rgba(255,255,255,.18)" : "#ddd",
    "--detail-btn-hover": dark ? "rgba(255,255,255,.13)" : "#f2f4f7",
  };
});

const albumId = Number(route.params.id);

/** FEAT-034-B：来自路由 query.focus 的需要高亮定位的照片路径。
 *  - Timeline 右下角按钮跳转携带该 query。
 *  - 路径含特殊字符时由 encodeURIComponent/decodeURIComponent 处理。
 *  - reactive：路由 query 变化（如同页中切换）也会响应。 */
const focusPath = computed(() => {
  const v = route.query.focus;
  if (typeof v !== "string" || !v) return undefined;
  try {
    return decodeURIComponent(v);
  } catch {
    return v;
  }
});
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

/** FEAT-E：返回上一级
 *  - 优先 router.back()（从 Memores/Timeline 跳转过来时回到上一页）
 *  - 若无历史（直接进入），回退到相册列表
 *  - 主页按钮始终可用（提供"随时回到主页"的兜底）
 */
function goBack() {
  if (window.history.length > 1) {
    router.back();
  } else {
    router.push("/albums");
  }
}

function goHome() {
  router.push("/home");
}

/** 面包屑导航：根据当前相册状态构建路径 */
const breadcrumbs = computed((): { label: string; to?: string }[] => {
  const crumbs: { label: string; to?: string }[] = [{ label: "相册列表", to: "/albums" }];
  if (store.currentAlbum) {
    // 有父分组时显示父分组
    if (store.currentAlbum.folder_path) {
      const parts = store.currentAlbum.folder_path.split("/").filter(Boolean);
      for (const part of parts) {
        crumbs.push({ label: part });
      }
    }
    crumbs.push({ label: store.currentAlbum.name });
  }
  return crumbs;
});

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
    notify.success("相册已删除");
    router.push("/albums");
  } catch (e) {
    notify.error("删除失败", String(e));
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
  <div class="detail-page" :style="detailVars">
    <!-- 顶部导航栏 -->
    <nav class="detail-nav">
      <div class="nav-left">
        <button class="btn" @click="goBack" title="返回上一级">← 返回</button>
        <button class="btn btn-home" @click="goHome" title="回到主页">🏠 主页</button>
        <!-- P1 面包屑导航 -->
        <BreadcrumbNav :crumbs="breadcrumbs" />
      </div>
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
        <!-- FEAT-034-B：接收路由 query.focus（时间线跳转定位目标照片路径） -->
        <PhotoGrid :album-id="albumId" :focus-path="focusPath" />
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
  color: var(--detail-text);
}

.detail-nav {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 24px;
}
.nav-left {
  display: flex;
  gap: 8px;
}
.btn-home {
  /* 主页按钮与返回按钮区分：紫色调 */
  background: linear-gradient(135deg, #6a8df0 0%, #a764ec 100%);
  color: #fff;
  border-color: transparent;
}
.btn-home:hover {
  background: linear-gradient(135deg, #5a7de0 0%, #9754dc 100%);
  color: #fff;
  border-color: transparent;
}

.nav-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.btn {
  padding: 8px 16px;
  border-radius: 8px;
  border: 1px solid var(--detail-btn-border);
  background: var(--detail-btn-bg);
  color: var(--detail-text);
  cursor: pointer;
  font-size: 14px;
  transition: all 0.2s;
}

/* 悬停态只作用于「普通按钮」：若直接写 `.btn:hover`，其选择符权重与
   `.btn-home:hover` 相同、且位于其后，会覆盖主页按钮的紫色背景与白色文字。 */
.btn:hover:not(.btn-home):not(.btn-primary):not(.btn-danger) {
  border-color: #396cd8;
  color: #396cd8;
  background: var(--detail-btn-hover);
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
  color: var(--detail-muted);
}

.not-found p {
  font-size: 16px;
  margin-bottom: 16px;
}
</style>