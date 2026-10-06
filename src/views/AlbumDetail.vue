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
import { useVaultStore } from "../stores/vault";

const route = useRoute();
const router = useRouter();
const store = useAlbumStore();
const theme = useThemeStore();
const notify = useNotify();
/** FEAT-095：隐藏相册解锁态（未解锁时禁止直达详情页） */
const vault = useVaultStore();

/** 页面级主题变量：本页按钮/文本直接落在页面背景上 ——
 *  颜色跟随实际背景的对比度（theme.onBg*），按钮底/边框按 onBgDark 二分；
 *  旧写法按 mode.isDark 二分，浅色预设下白字融进浅背景。 */
const detailVars = computed(() => {
  const onDark = theme.onBgDark; // true = 浅背景配深字
  return {
    "--detail-text": theme.onBgColor,
    "--detail-muted": theme.onBgSubColor,
    "--detail-btn-bg": onDark ? "rgba(0,0,0,.05)" : "rgba(255,255,255,.06)",
    "--detail-btn-border": onDark ? "rgba(0,0,0,.16)" : "rgba(255,255,255,.18)",
    "--detail-btn-hover": onDark ? "rgba(0,0,0,.1)" : "rgba(255,255,255,.13)",
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
  // FEAT-095：归档 / 隐藏是系统内置虚拟相册，删掉会导致标记数据无处安放
  if (isVirtual.value) {
    notify.warning("系统内置相册不可删除", "「归档」与「隐藏」相册由系统维护，请通过照片的归档 / 隐藏操作来管理内容。");
    return;
  }
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

/* ---- FEAT-095：虚拟相册（归档 / 隐藏）差异化处理 ---- */

/** 当前相册是否虚拟（归档 / 隐藏） */
const isVirtual = computed(() => {
  const k = store.currentAlbum?.kind;
  return k === "archived" || k === "hidden";
});

/**
 * 隐藏相册在未解锁时不允许直达详情页（绕过相册管理的密码门禁）
 *
 * 直接访问 `/album/:id` 也拦一道：隐藏相册的门禁不能只做在列表过滤上，
 * 否则把 id 写进地址栏就等于绕过密码。
 */
const vaultDenied = computed(
  () => store.currentAlbum?.kind === "hidden" && !vault.hiddenUnlocked,
);
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
        <!-- FEAT-095：虚拟相册不提供「删除相册」入口（系统内置，不可删） -->
        <button v-if="!isVirtual" class="btn btn-danger" :disabled="deleting" @click="deleteAlbum">
          {{ deleting ? "删除中…" : "删除相册" }}
        </button>
      </div>
    </nav>

    <!-- 加载失败提示 -->
    <div v-if="loadError" class="not-found">
      <p>相册不存在或已删除</p>
      <button class="btn btn-primary" @click="goBack">返回列表</button>
    </div>

    <!-- FEAT-095：隐藏相册未解锁（地址栏直达时的兜底拦截） -->
    <div v-else-if="vaultDenied" class="not-found">
      <p>🔒 该相册已隐藏，需在「个人信息」中输入密码解锁后才能查看。</p>
      <button class="btn btn-primary" @click="goBack">返回列表</button>
    </div>

    <template v-else-if="store.currentAlbum">
      <!-- FEAT-095：虚拟相册横幅（说明这是归档 / 隐藏聚合视图，且不可删除相册本身） -->
      <div v-if="isVirtual" class="virtual-banner">
        <span v-if="store.currentAlbum.kind === 'archived'">📦</span>
        <span v-else>🔒</span>
        <b>{{ store.currentAlbum.name }}</b>
        <span class="virtual-desc">{{ store.currentAlbum.description }}</span>
        <span class="virtual-tip">照片仍在原位置，可在此批量打分 / 标签 / 归档 / 删除，或取回原相册。</span>
      </div>

      <!-- 相册信息头（虚拟相册无封面 / 简介可编辑能力，跳过） -->
      <AlbumMeta
        v-if="!isVirtual"
        :album="store.currentAlbum"
        :setting-cover="settingCover"
        @update:album="handleAlbumUpdate"
        @delete="deleteAlbum"
      />

      <!-- 三个功能区统一用 CollapseSection 折叠：搜索 / 扫描在前，缩略图浏览在最后 -->
      <CollapseSection v-if="!isVirtual" title="🔍 照片搜索" storage-key="detail-search">
        <ContentSearch :album-id="albumId" />
      </CollapseSection>

      <!-- 扫描面板：组合扫描（EXIF/影调/AI 统一入口，后台执行）。人物注册表已移至智慧相册 -->
      <CollapseSection
        v-if="!isVirtual"
        title="🧩 组合扫描"
        subtitle="EXIF / 影调 / AI 统一入口 · 后台执行 · 退出页面不中断"
        storage-key="detail-scan"
      >
        <ScanPanel :album-id="albumId" />
      </CollapseSection>

      <!-- 照片网格浏览：相册内照片一览 + 大图查看（放在最后，作为浏览区） -->
      <CollapseSection title="🖼️ 缩略图浏览" storage-key="detail-photos">
        <!-- FEAT-034-B：接收路由 query.focus（时间线跳转定位目标照片路径）
             FEAT-095：albumKind 传给 PhotoGrid，决定右键菜单是「归档」还是「取消归档」 -->
        <PhotoGrid
          :album-id="albumId"
          :focus-path="focusPath"
          :album-kind="store.currentAlbum.kind"
        />
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
  /* 次按钮：幽灵/描边样式（每屏只留一个实心主按钮） */
  background: transparent;
  color: var(--color-link);
  border: 1px solid var(--color-border);
}
.btn-home:hover {
  background: var(--color-soft-accent);
  color: var(--color-primary-hover);
  border-color: var(--color-link);
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
  border-color: var(--color-primary);
  color: var(--color-primary-text);
  background: var(--detail-btn-hover);
}

.btn-danger {
  background: var(--color-danger);
  color: #fff;
  border-color: var(--color-danger);
}

.btn-danger:hover {
  background: var(--color-danger-hover);
  color: #fff;
}

.btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.btn-primary {
  background: var(--color-primary);
  color: var(--color-on-primary);
  border-color: var(--color-primary);
}

.btn-primary:hover {
  background: var(--color-primary-hover);
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

/* FEAT-095：归档 / 隐藏虚拟相册横幅 */
.virtual-banner {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  padding: 12px 16px;
  margin-bottom: 20px;
  border-radius: 12px;
  font-size: 13px;
  border: 1px solid color-mix(in srgb, var(--color-primary) 32%, transparent);
  background: color-mix(in srgb, var(--color-primary) 10%, transparent);
  color: var(--detail-text);
}
.virtual-banner b { font-size: 15px; }
.virtual-desc { color: var(--detail-muted); }
.virtual-tip { color: var(--detail-muted); font-size: 12px; }
</style>