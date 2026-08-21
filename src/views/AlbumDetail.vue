<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { listen } from "@tauri-apps/api/event";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import type { Album } from "../types/album";
import { formatSize } from "../types/album";
import type { ContentSearchHit, VcrGpuStatus } from "../types/content";
import type {
  ClassifyProgress,
  PersonInfo,
  PhotoExif,
  PhotoTone,
  VisionResult,
} from "../types/photo";
import { trace } from "../utils/trace";
import ConfirmDialog from "../components/ConfirmDialog.vue";

const route = useRoute();
const router = useRouter();
const store = useAlbumStore();
const contentStore = useContentStore();

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

/** 名称编辑（创建后也可改名） */
const editingName = ref(false);
const nameInput = ref("");
const savingName = ref(false);

function startEditName() {
  nameInput.value = store.currentAlbum?.name ?? "";
  editingName.value = true;
}

const saveName = trace("saveName", async () => {
  if (savingName.value) return;
  const name = nameInput.value.trim();
  if (!name) {
    alert("相册名称不能为空");
    return;
  }
  if (name.length > 100) {
    alert("相册名称不能超过 100 个字符");
    return;
  }
  savingName.value = true;
  try {
    await store.renameAlbum(albumId, name, true);
    editingName.value = false;
  } catch (e) {
    alert(`保存名称失败：${e}`);
  } finally {
    savingName.value = false;
  }
});

/** 说明编辑（导入后也可修改） */
const editingDesc = ref(false);
const descInput = ref("");
const savingDesc = ref(false);

function startEditDesc() {
  descInput.value = store.currentAlbum?.description ?? "";
  editingDesc.value = true;
}

const saveDesc = trace("saveDesc", async () => {
  if (savingDesc.value) return;
  savingDesc.value = true;
  try {
    await store.updateAlbum({
      id: albumId,
      description: descInput.value.trim(),
    });
    editingDesc.value = false;
  } catch (e) {
    alert(`保存说明失败：${e}`);
  } finally {
    savingDesc.value = false;
  }
});

/** 地点标签编辑 */
const editingLocation = ref(false);
const locationInput = ref("");
const savingLocation = ref(false);
const detectingLocation = ref(false);

/** 地点自动识别：扫描相册照片 GPS → 反向地理编码 → 落库（不动手动标签） */
const autoDetectLocation = trace("autoDetectLocation", async () => {
  if (detectingLocation.value || !store.currentAlbum) return;
  detectingLocation.value = true;
  try {
    const r = await invoke<{ location: string; changed: boolean; lat: number; lon: number }>(
      "auto_detect_album_location",
      { albumId: store.currentAlbum.id, force: false }
    );
    if (r.changed) {
      store.currentAlbum.location = r.location;
    }
    alert(`自动识别地点：${r.location}（${r.lat.toFixed(4)}, ${r.lon.toFixed(4)}）`);
  } catch (e) {
    alert(`自动识别地点失败：${e}`);
  } finally {
    detectingLocation.value = false;
  }
});

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

/** 图片 EXIF 扫描（测试功能）：扫描相册目录内所有图片的拍摄参数 */
const scanning = ref(false);
const placeScanning = ref(false);
const localScanning = ref(false);
const scanError = ref("");
const scanResult = ref<PhotoExif[] | null>(null);

const scanPhotos = trace("scanPhotos", async () => {
  if (scanning.value || !store.currentAlbum) return;
  scanning.value = true;
  scanError.value = "";
  try {
    scanResult.value = await invoke<PhotoExif[]>("scan_album_photos", {
      path: store.currentAlbum.path,
    });
  } catch (e) {
    scanError.value = `扫描失败：${e}`;
  } finally {
    scanning.value = false;
  }
});

/** EXIF + 本地省/市（离线）：用内嵌行政区划边界做点面判断，秒回，零网络 */
const scanPhotosLocalPlace = trace("scanPhotosLocalPlace", async () => {
  if (localScanning.value || !store.currentAlbum) return;
  localScanning.value = true;
  scanError.value = "";
  try {
    scanResult.value = await invoke<PhotoExif[]>("scan_album_photos_local_place", {
      path: store.currentAlbum.path,
    });
  } catch (e) {
    scanError.value = `本地地名查询失败：${e}`;
  } finally {
    localScanning.value = false;
  }
});

/** EXIF + 反向地理编码（联网）：在扫描基础上为有 GPS 坐标的照片补中文地名 */
const scanPhotosWithPlace = trace("scanPhotosWithPlace", async () => {
  if (placeScanning.value || !store.currentAlbum) return;
  placeScanning.value = true;
  scanError.value = "";
  try {
    scanResult.value = await invoke<PhotoExif[]>("scan_album_photos_with_place", {
      path: store.currentAlbum.path,
    });
  } catch (e) {
    scanError.value = `地名查询失败：${e}`;
  } finally {
    placeScanning.value = false;
  }
});

/** 坐标格式化：31.92128°N, 107.63749°E */
function fmtCoord(lat: number, lon: number): string {
  const la = `${Math.abs(lat).toFixed(4)}°${lat >= 0 ? "N" : "S"}`;
  const lo = `${Math.abs(lon).toFixed(4)}°${lon >= 0 ? "E" : "W"}`;
  return `${la}, ${lo}`;
}

/** 图片影调分析（测试功能）：扫描相册目录内所有图片的灰度直方图 + 影调类型 */
const toneScanning = ref(false);
const toneError = ref("");
const toneResult = ref<PhotoTone[] | null>(null);

const scanTones = trace("scanTones", async () => {
  if (toneScanning.value || !store.currentAlbum) return;
  toneScanning.value = true;
  toneError.value = "";
  try {
    toneResult.value = await invoke<PhotoTone[]>("scan_album_tones", {
      path: store.currentAlbum.path,
    });
  } catch (e) {
    toneError.value = `影调分析失败：${e}`;
  } finally {
    toneScanning.value = false;
  }
});

/** 影调类型标签文案 */
function toneLabel(t: PhotoTone["tone_type"]): string {
  switch (t) {
    case "low-key":
      return "低调";
    case "mid-key":
      return "中间调";
    case "high-key":
      return "高调";
    default:
      return "—";
  }
}

/** 直方图 256 bin 合并为 128 条绘制数据（降低 DOM 节点量），并归一化到 0..1 */
function histBars(h: number[]): { k: number; v: number }[] {
  const bins = 128;
  const merged = new Array(bins).fill(0) as number[];
  for (let i = 0; i < h.length; i++) {
    merged[Math.floor((i * bins) / h.length)] += h[i];
  }
  const max = Math.max(...merged, 1);
  return merged.map((v, k) => ({ k, v: v / max }));
}

/** 内容识别（YOLOv8n-cls，测试功能）：调用 Python 微服务批量识别图片内容 */
const visionScanning = ref(false);
const visionError = ref("");
const visionScanMessage = ref("");
const visionResult = ref<VisionResult[] | null>(null);
const visionProgress = ref<ClassifyProgress | null>(null);
let unlistenProgress: (() => void) | null = null;

// ---------- 批次与 GPU（R3） ----------
/** 推理批次（8/16/32，透传给 Python 服务） */
const scanBatch = ref(8);
const BATCH_OPTIONS = [8, 16, 32];
/** GPU 加速可行性状态 */
const gpuStatus = ref<VcrGpuStatus | null>(null);
const gpuLoading = ref(false);

/** 查询 GPU 可行性（会确保识别服务就绪） */
async function fetchGpuStatus() {
  gpuLoading.value = true;
  try {
    gpuStatus.value = await contentStore.fetchGpuStatus();
  } catch (e) {
    gpuStatus.value = null;
    visionError.value = `GPU 检测失败：${e}`;
  } finally {
    gpuLoading.value = false;
  }
}

/** GPU 状态展示文案 */
function gpuStatusText(): string {
  const g = gpuStatus.value;
  if (!g) return "未检测";
  if (!g.running) return "服务未运行";
  if (g.use_gpu) return `GPU 加速已启用（${g.provider}）`;
  return `CPU 推理（未检测到可用 GPU${g.gpu.length ? "；已安装 GPU 提供方 " + g.gpu.join(",") : ""}）`;
}

// ---------- 内容识别结果分页（R1：10/20/50/100 条每页） ----------
const PAGE_SIZES = [10, 20, 50, 100];
const pageSize = ref(50);
const currentPage = ref(1);
/** 当前页结果（切片，供表格渲染） */
const pagedVision = computed(() => {
  if (!visionResult.value) return [];
  const start = (currentPage.value - 1) * pageSize.value;
  return visionResult.value.slice(start, start + pageSize.value);
});
/** 总页数 */
const visionPageCount = computed(() =>
  Math.max(1, Math.ceil((visionResult.value?.length ?? 0) / pageSize.value))
);
/** 全局序号偏移（第几行，跨页连续） */
const visionRowOffset = computed(() => (currentPage.value - 1) * pageSize.value);
/** 切换页大小 → 回到第 1 页 */
watch(pageSize, () => {
  currentPage.value = 1;
});
/** 结果/页大小变化后，当前页越界则收拢到末页 */
watch([visionResult, pageSize], () => {
  if (currentPage.value > visionPageCount.value) currentPage.value = visionPageCount.value;
});
function changeVisionPage(p: number) {
  const next = Math.min(Math.max(1, p), visionPageCount.value);
  currentPage.value = next;
}

/** 人物注册表 */
const persons = ref<PersonInfo[]>([]);
const loadPersons = trace("loadPersons", async () => {
  try {
    persons.value = await invoke<PersonInfo[]>("list_persons");
  } catch {
    persons.value = [];
  }
});
const renamePerson = trace("renamePerson", async (ps: PersonInfo, name: string) => {
  const n = name.trim();
  if (!n) return;
  try {
    await invoke("rename_person", { pid: ps.id, name: n });
    ps.name = n;
  } catch (e) {
    alert(`重命名失败：${e}`);
  }
});
const askMerge = trace("askMerge", async (ps: PersonInfo) => {
  const target = prompt(`将 ${ps.id} 并入哪个标号？\n（输入目标标号，如 P001）`);
  if (!target?.trim()) return;
  try {
    await invoke("merge_persons", { target: target.trim(), source: ps.id });
    await loadPersons();
  } catch (e) {
    alert(`合并失败：${e}`);
  }
});
const removePerson = trace("removePerson", async (ps: PersonInfo) => {
  if (!confirm(`确定删除人物 ${ps.id}（${ps.name}）？其标号将从已识别照片中移除`)) return;
  try {
    await invoke("delete_person", { pid: ps.id });
    await loadPersons();
  } catch (e) {
    alert(`删除失败：${e}`);
  }
});

/** 相册大类 → 中文标签 + 样式 */
function categoryLabel(cat: string): string {
  const map: Record<string, string> = {
    animal: "动物",
    animal_pet: "动物",
    food: "食物",
    flower: "花朵",
    plant: "植物",
    plant_flower: "植物花卉",
    architecture: "建筑",
    cityscape: "城市风光",
    sports: "运动",
    landscape_nature: "自然风景",
    landscape: "自然风景",
    text: "文本截图",
    document: "文档",
    vehicle: "车辆",
    portrait: "人物特写",
    street: "扫街",
    night_scene: "夜景",
    other: "其他",
  };
  return map[cat] ?? cat;
}

/** 动物/植物 子类 → 中文 */
function animalSubLabel(sub: string): string {
  const map: Record<string, string> = { dog: "狗", cat: "猫", bird: "鸟", flower: "花", plant: "植物" };
  return map[sub] ?? sub ?? "";
}

/** 点击图片名：用系统默认看图程序打开实际图片 */
const openImage = trace("openImage", async (path: string) => {
  try {
    await openPath(path);
  } catch (e) {
    alert(`无法打开图片：${path}\n\n${e}`);
  }
});

const classifyAlbum = trace("classifyAlbum", async () => {
  if (visionScanning.value || !store.currentAlbum) return;
  visionScanning.value = true;
  visionError.value = "";
  visionProgress.value = null;
  currentPage.value = 1;
  try {
    // 监听进度事件（首次进入时注册一次，结束后不卸载，方便重复点击）
    if (!unlistenProgress) {
      unlistenProgress = await listen<ClassifyProgress>("classify-progress", (e) => {
        visionProgress.value = e.payload;
      });
    }
    // 内容扫描并落库（二次扫描按哈希覆盖更新；返回识别明细供表格展示）
    const outcome = await contentStore.scanAlbumContent(albumId, scanBatch.value);
    visionResult.value = outcome.results;
    const r = outcome.report;
    visionScanMessage.value = `✅ 已写入内容库 ${r.written}/${r.total} 张${r.failed ? `，失败 ${r.failed}` : ""}；识别结果已可用于照片内容搜索`;
    await loadPersons();
    // 扫描后刷新 GPU 可行性（识别服务此时已在运行）
    fetchGpuStatus();
  } catch (e) {
    visionError.value = `内容识别失败：${e}`;
  } finally {
    visionScanning.value = false;
  }
});

// ---------- 单相册内部内容搜索（智能搜索，范围 = 当前相册） ----------
const contentKeyword = ref("");
const contentHits = ref<ContentSearchHit[]>([]);
const contentSearching = ref(false);
let contentSearchTimer: ReturnType<typeof setTimeout> | null = null;

/** 单相册内容搜索输入（防抖） */
function onContentSearchInput() {
  if (contentSearchTimer) clearTimeout(contentSearchTimer);
  contentSearchTimer = setTimeout(async () => {
    const kw = contentKeyword.value.trim();
    if (!kw) {
      contentHits.value = [];
      return;
    }
    contentSearching.value = true;
    try {
      contentHits.value = await contentStore.searchPhotoContent(kw, albumId);
    } catch {
      contentHits.value = [];
    } finally {
      contentSearching.value = false;
    }
  }, 300);
}

/** 清空单相册内容搜索 */
function clearContentSearch() {
  contentKeyword.value = "";
  contentHits.value = [];
}

/** 点击单相册内容搜索命中：用系统默认看图程序打开原图 */
async function openContentHit(path: string) {
  try {
    await openPath(path);
  } catch (e) {
    alert(`无法打开图片：${path}\n\n${e}`);
  }
}

/** 单相册内容搜索命中显示名：label / category / 文件名 依次回退 */
function contentHitName(hit: ContentSearchHit): string {
  if (hit.label) return hit.label;
  if (hit.category) return hit.category;
  const i = Math.max(hit.path.lastIndexOf("/"), hit.path.lastIndexOf("\\"));
  return i >= 0 ? hit.path.slice(i + 1) : hit.path;
}
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
          <!-- 名称（可点击编辑，创建后也可改名） -->
          <div v-if="!editingName" class="detail-name-wrap" @click="startEditName" title="点击编辑名称">
            <h1 class="detail-name">{{ store.currentAlbum.name }}</h1>
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
            <span
              class="path-link"
              title="在文件资源管理器中打开"
              @click="openAlbumPath(store.currentAlbum.path)"
            >
              📁 {{ store.currentAlbum.path }}
            </span>
          </p>
          <!-- 说明（可点击编辑，导入后也可修改） -->
          <div v-if="!editingDesc" class="detail-desc-wrap" @click="startEditDesc" title="点击编辑说明">
            <p class="detail-desc">{{ store.currentAlbum.description || "暂无说明" }}</p>
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

      <!-- 图片 EXIF 扫描（测试功能） -->
      <section class="scan-area">
        <div class="scan-toolbar">
          <div>
            <h3 class="scan-title">图片 EXIF 扫描</h3>
            <p class="scan-sub">扫描相册目录内所有图片的拍摄参数（ISO / 焦段 / 光圈 / 快门速度 / 拍摄时间），点击照片名字可用默认看图程序打开原图</p>
          </div>
          <button class="btn btn-primary" :disabled="scanning" @click="scanPhotos">
            {{ scanning ? "扫描中…" : scanResult ? "重新扫描" : "开始扫描" }}
          </button>
        </div>

        <!-- 错误提示 -->
        <p v-if="scanError" class="scan-error">{{ scanError }}</p>

        <!-- 空状态 -->
        <div v-if="!scanning && !scanResult && !scanError" class="scan-empty">
          <p>点击「开始扫描」查看相册内照片的 EXIF 信息</p>
        </div>

        <!-- 扫描中状态 -->
        <div v-if="scanning" class="scan-empty">
          <p class="scan-loading">⏳ 正在扫描… 请稍候</p>
        </div>

        <!-- 结果表格 -->
        <div v-if="scanResult" class="scan-table-wrap">
          <div class="scan-toolbar" style="padding:10px 14px 0;border:none">
            <p class="scan-sub">地点列：默认显示坐标链接（点击打开地图定位）；「本地省/市」秒回离线反查，未命中（国外/公海）仍显示坐标链接；「精确地名」联网反查到区县级</p>
            <div class="scan-actions" style="display:flex;gap:8px;flex-wrap:wrap">
              <button class="btn btn-primary" :disabled="scanning || placeScanning || localScanning" @click="scanPhotos">重新扫描</button>
              <button class="btn btn-ghost" :disabled="placeScanning || localScanning" @click="scanPhotosLocalPlace">
                {{ localScanning ? "本地解析中…" : "本地省/市（秒回·离线）" }}
              </button>
              <button class="btn btn-ghost" :disabled="placeScanning || localScanning" @click="scanPhotosWithPlace">
                {{ placeScanning ? "地名查询中…" : "精确地名（联网）" }}
              </button>
            </div>
          </div>
          <table class="scan-table">
            <thead>
              <tr>
                <th class="col-idx">#</th>
                <th>照片名字</th>
                <th>ISO</th>
                <th>焦段</th>
                <th>光圈</th>
                <th>快门速度</th>
                <th>拍摄时间</th>
                <th>地点</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="(p, i) in scanResult" :key="p.path">
                <td class="col-idx">{{ i + 1 }}</td>
                <td class="col-name" :title="p.path">
                  <span class="img-name-link" @click="openImage(p.path)">{{ p.file_name }}</span>
                </td>
                <td>{{ p.iso ?? "—" }}</td>
                <td>{{ p.focal_length ?? "—" }}</td>
                <td>{{ p.aperture ?? "—" }}</td>
                <td>{{ p.shutter_speed ?? "—" }}</td>
                <td>{{ p.shoot_time ?? "—" }}</td>
                <td class="col-place">
                  <template v-if="p.place">{{ p.place }}</template>
                  <template v-else-if="p.lat !== null && p.lon !== null">
                    <a v-if="p.map_url" :href="p.map_url" target="_blank" rel="noopener" class="gps-link">
                      {{ fmtCoord(p.lat, p.lon) }}
                    </a>
                    <span v-else>{{ fmtCoord(p.lat, p.lon) }}</span>
                    <span v-if="p.alt_m !== null" class="gps-alt">{{ Math.round(p.alt_m) }}m</span>
                  </template>
                  <span v-else>—</span>
                </td>
              </tr>
            </tbody>
          </table>
          <p class="scan-count">共 {{ scanResult.length }} 张图片</p>
        </div>
      </section>

      <!-- 图片影调分析（测试功能） -->
      <section class="scan-area tone-area">
        <div class="scan-toolbar">
          <div>
            <h3 class="scan-title">图片影调分析</h3>
            <p class="scan-sub">下采样后统计灰度直方图，按平均亮度法判断影调（低调 &lt; 85 ≤ 中间调 ≤ 170 &lt; 高调），点击照片名字可用默认看图程序打开原图</p>
          </div>
          <button class="btn btn-primary" :disabled="toneScanning" @click="scanTones">
            {{ toneScanning ? "分析中…" : toneResult ? "重新分析" : "开始分析" }}
          </button>
        </div>

        <!-- 错误提示 -->
        <p v-if="toneError" class="scan-error">{{ toneError }}</p>

        <!-- 空状态 -->
        <div v-if="!toneScanning && !toneResult && !toneError" class="scan-empty">
          <p>点击「开始分析」查看相册内照片的灰度分布与影调类型</p>
        </div>

        <!-- 分析中状态 -->
        <div v-if="toneScanning" class="scan-empty">
          <p class="scan-loading">⏳ 正在解码分析… 请稍候</p>
        </div>

        <!-- 结果表格 -->
        <div v-if="toneResult" class="scan-table-wrap">
          <svg width="0" height="0" style="position:absolute">
            <defs>
              <linearGradient id="hist-grad" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stop-color="#396cd8" />
                <stop offset="100%" stop-color="#8fb3f0" />
              </linearGradient>
            </defs>
          </svg>
          <table class="scan-table tone-table">
            <thead>
              <tr>
                <th class="col-idx">#</th>
                <th>照片名字</th>
                <th>影调类型</th>
                <th>平均亮度 L̄</th>
                <th>灰度柱状图分布</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="(p, i) in toneResult" :key="p.path">
                <td class="col-idx">{{ i + 1 }}</td>
                <td class="col-name" :title="p.path">
                  <span class="img-name-link" @click="openImage(p.path)">{{ p.file_name }}</span>
                </td>
                <td>
                  <span class="tone-badge" :class="p.tone_type ? 'tone-' + p.tone_type : ''">
                    {{ toneLabel(p.tone_type) }}
                  </span>
                </td>
                <td class="col-luma">{{ p.avg_luma != null ? p.avg_luma.toFixed(1) : "—" }}</td>
                <td class="col-hist">
                  <svg
                    v-if="p.histogram.length > 0"
                    viewBox="0 0 256 36"
                    preserveAspectRatio="none"
                    class="mini-hist"
                  >
                    <rect
                      v-for="b in histBars(p.histogram)"
                      :key="b.k"
                      :x="b.k * 2"
                      :width="2"
                      :height="1 + b.v * 33"
                      :y="36 - (1 + b.v * 33)"
                      fill="url(#hist-grad)"
                    />
                  </svg>
                  <span v-else class="hist-empty">—</span>
                </td>
              </tr>
            </tbody>
          </table>
          <p class="scan-count">共 {{ toneResult.length }} 张图片（下采样至 256px 后统计）</p>
        </div>
      </section>

      <!-- 内容识别（YOLOv8n-cls，测试功能） -->
      <section class="scan-area vision-area">
        <div class="scan-toolbar">
          <div>
            <h3 class="scan-title">内容识别</h3>
            <p class="scan-sub">三路识别（YOLOv8s-cls 分类 + YOLOv8n 检测 + Places365 场景）融合为相册大类：人物特写/扫街/自然风景/建筑城市/动物/文本等；人物自动标号（同人同 P 编号），动物细分狗/猫/鸟。点击照片名字可用默认看图程序打开原图</p>
          </div>
          <div class="scan-tool-actions">
            <!-- 批次选择（R3） -->
            <label class="batch-select">
              批次
              <select v-model="scanBatch" title="推理批次：每批提交识别的图片数">
                <option v-for="b in BATCH_OPTIONS" :key="b" :value="b">{{ b }}</option>
              </select>
            </label>
            <!-- GPU 可行性 -->
            <button class="btn btn-mini" :disabled="gpuLoading" @click="fetchGpuStatus" title="检测 GPU 加速可行性">
              {{ gpuLoading ? "检测中…" : "检测 GPU" }}
            </button>
            <button class="btn btn-primary" :disabled="visionScanning" @click="classifyAlbum">
              {{ visionScanning ? "识别中…" : visionResult ? "重新识别" : "开始识别" }}
            </button>
          </div>
        </div>

        <!-- GPU 状态 -->
        <p v-if="gpuStatus" class="gpu-status" :class="{ 'gpu-on': gpuStatus.use_gpu }">
          🖥 {{ gpuStatusText() }}
        </p>

        <!-- 错误提示 -->
        <p v-if="visionError" class="scan-error">{{ visionError }}</p>

        <!-- 成功提示（已写入内容库） -->
        <p v-if="visionScanMessage && !visionError" class="scan-ok">{{ visionScanMessage }}</p>

        <!-- 空状态 -->
        <div v-if="!visionScanning && !visionResult && !visionError" class="scan-empty">
          <p>点击「开始识别」用 AI 识别相册图片内容（首次会启动识别服务，需等待约 10 秒）</p>
        </div>

        <!-- 进度条 -->
        <div v-if="visionScanning || (visionProgress && visionProgress.total > 0)" class="vision-progress">
          <div class="progress-track">
            <div
              class="progress-fill"
              :style="{ width: (visionProgress ? (visionProgress.current / visionProgress.total) * 100 : 0) + '%' }"
            ></div>
          </div>
          <p v-if="visionProgress" class="progress-text">
            {{ visionProgress.current }} / {{ visionProgress.total }} 张
            （成功 {{ visionProgress.done }} · 失败 {{ visionProgress.failed }}）
          </p>
        </div>

        <!-- 结果表格 -->
        <div v-if="visionResult" class="scan-table-wrap">
          <table class="scan-table vision-table">
            <thead>
              <tr>
                <th class="col-idx">#</th>
                <th>照片名字</th>
                <th>内容类别</th>
                <th>识别细类</th>
                <th>置信度</th>
                <th>Top3 候选</th>
                <th>耗时</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="(p, i) in pagedVision" :key="p.path">
                <td class="col-idx">{{ visionRowOffset + i + 1 }}</td>
                <td>
                  <span class="img-name-link" :title="p.path" @click="openImage(p.path)">
                    {{ p.file_name }}
                  </span>
                </td>
                <td>
                  <span v-if="p.error" class="vision-err">{{ p.error }}</span>
                  <template v-else>
                    <span class="vision-cat">{{ categoryLabel(p.category) }}</span>
                    <span v-if="p.sub_category" class="vision-sub">{{ animalSubLabel(p.sub_category) }}</span>
                    <span
                      v-for="pid in p.person_ids"
                      :key="pid"
                      class="person-chip"
                      :title="'同人标号：此人与其他 P 编号照片为同一人'"
                    >{{ pid }}</span>
                  </template>
                </td>
                <td class="col-label">
                  {{ p.label || "—" }}
                  <span v-if="p.person_count > 1" class="ppl-note">{{ p.person_count }} 人</span>
                </td>
                <td>
                  <span v-if="!p.error" class="conf-bar">
                    <span class="conf-fill" :style="{ width: Math.round(p.confidence * 100) + '%' }"></span>
                  </span>
                  <span v-if="!p.error" class="conf-val">{{ (p.confidence * 100).toFixed(1) }}%</span>
                </td>
                <td class="col-top3">
                  <template v-if="p.top3.length">
                    <span v-for="(t, ti) in p.top3" :key="ti" class="top3-chip">
                      {{ categoryLabel(t.category) }}·{{ t.label }}
                    </span>
                  </template>
                  <span v-else>—</span>
                </td>
                <td class="col-luma">{{ p.elapsed_ms ? p.elapsed_ms.toFixed(0) + "ms" : "—" }}</td>
              </tr>
            </tbody>
          </table>
          <!-- 分页控件（R1：10/20/50/100 条每页） -->
          <div class="vision-pager">
            <label class="pager-size">
              每页
              <select v-model="pageSize">
                <option v-for="s in PAGE_SIZES" :key="s" :value="s">{{ s }}</option>
              </select>
              条
            </label>
            <div class="pager-nav">
              <button class="btn-mini" :disabled="currentPage <= 1" @click="changeVisionPage(currentPage - 1)">上一页</button>
              <span class="pager-info">第 {{ currentPage }} / {{ visionPageCount }} 页</span>
              <button class="btn-mini" :disabled="currentPage >= visionPageCount" @click="changeVisionPage(currentPage + 1)">下一页</button>
            </div>
          </div>
          <p class="scan-count">共 {{ visionResult.length }} 张图片（识别服务常驻，重复识别更快）</p>
        </div>

        <!-- 人物管理 -->
        <div v-if="visionResult && persons.length" class="persons-panel">
          <div class="persons-head">
            <span class="persons-title">👤 人物注册表（同人同标号 · 可重命名/合并/删除）</span>
            <button class="persons-refresh" @click="loadPersons">刷新</button>
          </div>
          <div class="persons-grid">
            <div v-for="ps in persons" :key="ps.id" class="person-card">
              <div class="person-card-top">
                <span class="person-card-id mono">{{ ps.id }}</span>
                <span class="person-card-name">{{ ps.name || ps.id }}</span>
                <span class="person-card-count">{{ ps.face_count }} 张脸</span>
              </div>
              <div class="person-card-actions">
                <input
                  class="person-rename-input"
                  :placeholder="'重命名 ' + ps.id"
                  @keyup.enter="renamePerson(ps, ($event.target as HTMLInputElement).value)"
                />
                <button class="btn-mini" @click="askMerge(ps)">并入他</button>
                <button class="btn-mini danger" @click="removePerson(ps)">删除</button>
              </div>
            </div>
          </div>
        </div>

        <!-- 单相册内容搜索（智能搜索，范围 = 当前相册） -->
        <div class="content-search-area">
          <div class="content-search-input-wrap">
            <input
              v-model="contentKeyword"
              class="content-search-input"
              placeholder="在本相册内按内容搜索照片，如：狗 / 人物 / 建筑 / P001…"
              @input="onContentSearchInput"
            />
            <button v-if="contentKeyword" class="search-clear" @click="clearContentSearch">×</button>
          </div>
          <div v-if="contentKeyword.trim()" class="content-search-results">
            <div v-if="contentSearching" class="scan-empty">正在搜索照片内容…</div>
            <div v-else-if="contentHits.length === 0" class="scan-empty">
              未在本相册中找到匹配的照片（可先点击上方「内容识别」写入内容库）
            </div>
            <div v-else class="content-hit-list">
              <div
                v-for="hit in contentHits"
                :key="hit.id"
                class="content-hit-item"
                :title="hit.path"
                @click="openContentHit(hit.path)"
              >
                <span class="content-hit-name">{{ contentHitName(hit) }}</span>
                <span class="content-hit-tags">
                  <span v-if="hit.label" class="top3-chip">{{ hit.label }}</span>
                  <span v-for="pid in hit.person_ids" :key="pid" class="person-chip">{{ pid }}</span>
                  <span v-if="hit.location" class="top3-chip">{{ hit.location }}</span>
                  <span v-if="hit.shoot_time" class="top3-chip">{{ hit.shoot_time }}</span>
                  <span v-if="hit.iso" class="top3-chip">ISO {{ hit.iso }}</span>
                  <span v-if="hit.aperture" class="top3-chip">{{ hit.aperture }}</span>
                </span>
              </div>
            </div>
          </div>
        </div>
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

.btn-ghost {
  background: transparent;
  color: #396cd8;
  border-color: #396cd8;
}

.btn-ghost:hover {
  background: rgba(57, 108, 216, 0.08);
  color: #2f5cc2;
}

/* EXIF 地点列：坐标链接 + 海拔 */
.gps-link {
  color: #396cd8;
  text-decoration: none;
  border-bottom: 1px dashed #396cd8;
}

.gps-link:hover {
  color: #2f5cc2;
}

.gps-alt {
  margin-left: 6px;
  color: #888;
  font-size: 12px;
}

.col-place {
  max-width: 220px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 地点自动识别按钮 */
.loc-auto-btn {
  margin-left: 8px;
  padding: 2px 10px;
  font-size: 12px;
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
  margin: 0;
  font-size: 28px;
}

/* 名称编辑（点击进入） */
.detail-name-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
  border-radius: 8px;
  padding: 2px 8px;
  margin: 0 -8px 12px;
  transition: background 0.15s;
}

.detail-name-wrap:hover {
  background: #f5f7ff;
}

.name-edit-hint {
  font-size: 14px;
  opacity: 0;
  transition: opacity 0.15s;
}

.detail-name-wrap:hover .name-edit-hint {
  opacity: 1;
}

.name-edit {
  margin-bottom: 12px;
}

.name-input {
  width: 100%;
  box-sizing: border-box;
  border: 1px solid #396cd8;
  border-radius: 8px;
  padding: 8px 12px;
  font-size: 20px;
  font-weight: 600;
  outline: none;
}

.name-edit-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 8px;
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

/* 说明编辑（点击进入） */
.detail-desc-wrap {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  cursor: pointer;
  border-radius: 8px;
  padding: 2px 6px;
  margin: 0 -6px;
  transition: background 0.15s;
}

.detail-desc-wrap:hover {
  background: #f5f7ff;
}

.detail-desc-wrap:hover .desc-edit-hint {
  opacity: 1;
}

.desc-edit-hint {
  font-size: 11px;
  color: #396cd8;
  opacity: 0;
  white-space: nowrap;
  transition: opacity 0.15s;
  margin-top: 3px;
}

.desc-edit {
  margin-bottom: 8px;
}

.desc-textarea {
  width: 100%;
  box-sizing: border-box;
  border: 1px solid #ddd;
  border-radius: 8px;
  padding: 10px 12px;
  font-size: 14px;
  font-family: inherit;
  line-height: 1.6;
  resize: vertical;
  outline: none;
}

.desc-textarea:focus {
  border-color: #396cd8;
}

.desc-edit-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 8px;
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

/* 图片 EXIF 扫描区 */
.scan-area {
  border: 1px solid #e2e5ea;
  border-radius: 12px;
  padding: 20px 24px;
  background: #fff;
}

.scan-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 16px;
  flex-wrap: wrap;
}

.scan-title {
  margin: 0 0 4px;
  font-size: 16px;
  color: #2c3e50;
}

.scan-sub {
  margin: 0;
  font-size: 13px;
  color: #888;
}

.scan-empty {
  text-align: center;
  padding: 32px 0;
  color: #999;
}

.scan-loading {
  color: #396cd8;
  font-weight: 500;
}

.scan-error {
  color: #e5484d;
  font-size: 13px;
  background: #fdf0f0;
  border: 1px solid #f5c6c6;
  border-radius: 8px;
  padding: 10px 14px;
  margin-bottom: 12px;
}

.scan-ok {
  color: #1a7f37;
  font-size: 13px;
  background: #f0fdf4;
  border: 1px solid #bbe8c8;
  border-radius: 8px;
  padding: 10px 14px;
  margin-bottom: 12px;
}

/* 单相册内容搜索 */
.content-search-area {
  margin-top: 16px;
  border-top: 1px dashed #eef0f4;
  padding-top: 14px;
}

.content-search-input-wrap {
  position: relative;
  display: flex;
  align-items: center;
}

.content-search-input {
  width: 100%;
  padding: 10px 36px 10px 14px;
  border: 1px solid #ddd;
  border-radius: 8px;
  font-size: 14px;
  outline: none;
  box-sizing: border-box;
  transition: border-color 0.2s;
}

.content-search-input:focus {
  border-color: #396cd8;
}

.content-search-results {
  margin-top: 10px;
}

.content-hit-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.content-hit-item {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  padding: 8px 12px;
  border-radius: 8px;
  background: #fafbfc;
  border: 1px solid #eef0f4;
  cursor: pointer;
  transition: background 0.2s, border-color 0.2s;
}

.content-hit-item:hover {
  background: #f0f5ff;
  border-color: #bcd4f7;
}

.content-hit-name {
  font-size: 13px;
  font-weight: 500;
  color: #2c3e50;
  min-width: 140px;
}

.content-hit-tags {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.scan-table-wrap {
  overflow-x: auto;
}

.scan-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}

.scan-table th {
  text-align: left;
  padding: 10px 12px;
  background: #f5f7fa;
  font-size: 12px;
  color: #888;
  font-weight: 600;
  border-bottom: 2px solid #e2e5ea;
  white-space: nowrap;
}

.scan-table td {
  padding: 9px 12px;
  border-bottom: 1px solid #f0f2f5;
  color: #2c3e50;
  white-space: nowrap;
}

.scan-table tbody tr:hover td {
  background: #f7f9ff;
}

.scan-table .col-idx {
  color: #bbb;
  font-size: 12px;
  text-align: center;
  width: 40px;
}

.scan-table .col-name {
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
  font-weight: 500;
  cursor: help;
}

.scan-count {
  margin: 12px 0 0;
  font-size: 12px;
  color: #999;
}

/* 内容识别结果分页（R1） */
.vision-pager {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 12px;
  flex-wrap: wrap;
}

.pager-size {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: #555;
}

.pager-size select {
  padding: 4px 6px;
  border: 1px solid #ddd;
  border-radius: 6px;
  font-size: 13px;
  background: #fff;
  outline: none;
}

.pager-nav {
  display: flex;
  align-items: center;
  gap: 10px;
}

.pager-info {
  font-size: 13px;
  color: #666;
}

.pager-nav .btn-mini:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

/* 内容识别工具区：批次选择 + GPU 状态（R3） */
.scan-tool-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  flex-shrink: 0;
}

.batch-select {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: #555;
}

.batch-select select {
  padding: 6px 8px;
  border: 1px solid #ddd;
  border-radius: 6px;
  font-size: 13px;
  background: #fff;
  outline: none;
}

.gpu-status {
  margin: 10px 0 0;
  font-size: 13px;
  color: #8a6d3b;
  background: #fdf6e3;
  border: 1px solid #f0e0b0;
  border-radius: 8px;
  padding: 8px 12px;
}

.gpu-status.gpu-on {
  color: #1a7f37;
  background: #f0fdf4;
  border-color: #bbe8c8;
}

.scan-tool-actions .btn-mini {
  padding: 8px 12px;
}

/* 影调分析区 */
.tone-area {
  margin-top: 16px;
}

/* 影调类型标签：低调深色 / 中间调中性 / 高调浅色 */
.tone-badge {
  display: inline-block;
  font-size: 12px;
  font-weight: 600;
  padding: 2px 10px;
  border-radius: 10px;
}

.tone-low-key {
  background: #2c2c2c;
  color: #fff;
}

.tone-mid-key {
  background: #8a8f98;
  color: #fff;
}

.tone-high-key {
  background: #fff3d6;
  color: #7a5c00;
  border: 1px solid #e8d5a0;
}

/* 平均亮度列 */
.col-luma {
  font-family: Consolas, Monaco, monospace;
  font-size: 12px;
  color: #555;
}

/* 迷你柱状图 */
.col-hist {
  min-width: 220px;
  width: 40%;
}

.mini-hist {
  display: block;
  width: 100%;
  max-width: 360px;
  height: 36px;
  background: #f7f9fc;
  border: 1px solid #eceef2;
  border-radius: 4px;
}

.hist-empty {
  color: #bbb;
}

/* 内容识别区 */
.vision-area {
  margin-top: 16px;
}

/* 进度条 */
.vision-progress {
  margin-bottom: 14px;
}

.progress-track {
  height: 8px;
  border-radius: 4px;
  background: #eef1f6;
  overflow: hidden;
}

.progress-fill {
  height: 100%;
  border-radius: 4px;
  background: linear-gradient(90deg, #396cd8, #6fa0f0);
  transition: width 0.25s ease;
}

.progress-text {
  margin: 6px 0 0;
  font-size: 12px;
  color: #888;
  font-family: Consolas, Monaco, monospace;
}

/* 图片名可点击 */
.img-name-link {
  color: #396cd8;
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 2px;
  font-weight: 500;
  max-width: 300px;
  display: inline-block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
}

.img-name-link:hover {
  color: #2f5cc2;
}

.vision-cat {
  display: inline-block;
  font-size: 12px;
  font-weight: 600;
  padding: 2px 10px;
  border-radius: 10px;
  background: #eef3ff;
  color: #396cd8;
}

/* 动物子类 / 同人标号 / 人数 */
.vision-sub {
  display: inline-block;
  font-size: 11px;
  font-weight: 600;
  margin-left: 6px;
  padding: 2px 8px;
  border-radius: 10px;
  background: #fff7e6;
  color: #d48806;
}

.person-chip {
  display: inline-block;
  font-size: 11px;
  font-weight: 700;
  margin-left: 6px;
  padding: 2px 8px;
  border-radius: 10px;
  background: #f0f5e9;
  color: #4a7d2a;
  border: 1px solid #cfe3bd;
  cursor: help;
  font-family: 'JetBrains Mono', Consolas, monospace;
}

.ppl-note {
  margin-left: 6px;
  font-size: 11px;
  color: #d48806;
  font-weight: 600;
}

/* 人物注册表面板 */
.persons-panel {
  margin-top: 16px;
  padding: 14px 16px;
  border: 1px solid #e8ebf0;
  border-radius: 12px;
  background: #fafbfc;
}
.persons-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 10px;
}
.persons-title {
  font-size: 12px;
  font-weight: 700;
  color: #333;
}
.persons-refresh {
  font-size: 11px;
  padding: 2px 10px;
  border: 1px solid #396cd8;
  color: #396cd8;
  background: #fff;
  border-radius: 6px;
  cursor: pointer;
}
.persons-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: 8px;
}
.person-card {
  border: 1px solid #e8ebf0;
  border-radius: 10px;
  padding: 8px 10px;
  background: #fff;
}
.person-card-top {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 6px;
}
.person-card-id {
  font-size: 11px;
  font-weight: 700;
  color: #4a7d2a;
  background: #f0f5e9;
  padding: 1px 6px;
  border-radius: 6px;
}
.person-card-name {
  font-size: 12px;
  font-weight: 600;
  color: #333;
}
.person-card-count {
  margin-left: auto;
  font-size: 10px;
  color: #999;
}
.person-card-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}
.person-rename-input {
  flex: 1;
  min-width: 0;
  font-size: 11px;
  padding: 2px 8px;
  border: 1px solid #e0e4ea;
  border-radius: 6px;
  outline: none;
}
.btn-mini {
  font-size: 11px;
  padding: 2px 8px;
  border: 1px solid #d9dee5;
  color: #555;
  background: #fff;
  border-radius: 6px;
  cursor: pointer;
  white-space: nowrap;
}
.btn-mini:hover {
  border-color: #396cd8;
  color: #396cd8;
}
.btn-mini.danger:hover {
  border-color: #e5484d;
  color: #e5484d;
}
.mono {
  font-family: Consolas, Monaco, monospace;
}

.vision-err {
  font-size: 12px;
  color: #e5484d;
}

.col-label {
  color: #555;
}

/* 置信度条 */
.conf-bar {
  display: inline-block;
  width: 64px;
  height: 6px;
  border-radius: 3px;
  background: #eef1f6;
  overflow: hidden;
  vertical-align: middle;
  margin-right: 6px;
}

.conf-fill {
  display: block;
  height: 100%;
  border-radius: 3px;
  background: #4a86e8;
}

.conf-val {
  font-family: Consolas, Monaco, monospace;
  font-size: 12px;
  color: #555;
}

/* Top3 候选 */
.col-top3 {
  max-width: 280px;
}

.top3-chip {
  display: inline-block;
  font-size: 11px;
  color: #777;
  background: #f5f7fa;
  border: 1px solid #e8ebf0;
  border-radius: 8px;
  padding: 1px 8px;
  margin: 1px 4px 1px 0;
  white-space: nowrap;
}


.not-found {
  text-align: center;
  padding: 80px 0;
  color: #888;
}
</style>
