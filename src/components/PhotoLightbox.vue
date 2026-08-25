<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { AlbumContentRow } from "../types/content";
import type { PhotoInfo } from "../types/photo";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";

/**
 * 大图查看器（Lightbox）
 *
 * - 全屏遮罩展示当前照片原图
 * - 上一张/下一张（左右方向键）、关闭（ESC）
 * - 底部元数据面板：文件名、AI 分类/人物/置信度、EXIF、影调
 *
 * FEAT-D：自动扫描复用
 *   若当前照片在 photo_content_scan 中尚无记录，watch photo 变化时静默调
 *   ensure_photo_scanned 命令触发单张扫描并落库；扫描完成后 meta 实时刷新。
 *   已有记录时直接使用（零 IO）。
 */

interface LightboxPhoto {
  path: string;
  meta?: AlbumContentRow;
  /** FEAT-D：原图所属相册 ID（用于 ensure_photo_scanned）；非相册上下文（如 Memories 跨相册）传 null */
  albumId?: number | null;
}

const props = defineProps<{
  photos: LightboxPhoto[];
  index: number;
  /** 人物编号 → 自定义命名（无扫描/未命名时回退编号） */
  persons?: Record<string, string>;
}>();

const emit = defineEmits<{ (e: "close"): void }>();

const current = ref(props.index);
const imgLoading = ref(true);

/* ---- 缩放/平移（原图查看）---- */
const scale = ref(1);
const tx = ref(0);
const ty = ref(0);
const dragging = ref(false);
const overlayEl = ref<HTMLElement | null>(null);
let dragLastX = 0;
let dragLastY = 0;

function resetView() {
  scale.value = 1;
  tx.value = 0;
  ty.value = 0;
  dragging.value = false;
}

/** Ctrl + 滚轮：以 15% 步长缩放（范围 20% ~ 800%） */
function onWheel(e: WheelEvent) {
  if (!e.ctrlKey) return;
  e.preventDefault(); // 阻止 WebView 页面缩放
  const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
  const next = Math.min(8, Math.max(0.2, scale.value * factor));
  scale.value = next;
  if (next === 1) {
    tx.value = 0;
    ty.value = 0;
  }
}

function onDragStart(e: PointerEvent) {
  if (scale.value <= 1) return;
  dragging.value = true;
  dragLastX = e.clientX;
  dragLastY = e.clientY;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onDragMove(e: PointerEvent) {
  if (!dragging.value) return;
  tx.value += e.clientX - dragLastX;
  ty.value += e.clientY - dragLastY;
  dragLastX = e.clientX;
  dragLastY = e.clientY;
}

function onDragEnd() {
  dragging.value = false;
}

function toggleZoom() {
  if (scale.value > 1) resetView();
  else scale.value = 2.5;
}

const photo = computed<LightboxPhoto>(() => props.photos[current.value]);

function fileUrl(p: string) {
  return p ? convertFileSrc(p) : "";
}

function prev() {
  if (props.photos.length <= 1) return;
  current.value = (current.value - 1 + props.photos.length) % props.photos.length;
  imgLoading.value = true;
  resetView();
}

function next() {
  if (props.photos.length <= 1) return;
  current.value = (current.value + 1) % props.photos.length;
  imgLoading.value = true;
  resetView();
}

function onKey(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.preventDefault(); // 避免全局 ESC 处理同时触发 router.back
    emit("close");
  }
  else if (e.key === "ArrowLeft") prev();
  else if (e.key === "ArrowRight") next();
}

/* ---- 触屏：左右滑动切换上一张/下一张（缩放状态下滑动留给平移，不切图）---- */
let touchStartX = 0;
let touchStartY = 0;
let touchStartTime = 0;
function onTouchStart(e: TouchEvent) {
  const t = e.touches[0];
  touchStartX = t.clientX;
  touchStartY = t.clientY;
  touchStartTime = Date.now();
}
function onTouchEnd(e: TouchEvent) {
  const t = e.changedTouches[0];
  const dx = t.clientX - touchStartX;
  const dy = t.clientY - touchStartY;
  const dt = Date.now() - touchStartTime;
  if (scale.value !== 1 || Math.abs(dx) < 60 || Math.abs(dy) > 80 || dt > 500) return;
  if (dx > 0) prev();
  else next();
}

onMounted(() => {
  window.addEventListener("keydown", onKey);
  // wheel 必须用非 passive 监听才能 preventDefault 拦截页面缩放
  overlayEl.value?.addEventListener("wheel", onWheel, { passive: false });
});
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKey);
  overlayEl.value?.removeEventListener("wheel", onWheel);
});

// meta 定义在 scanState 之后（FEAT-D）。

/* ---- 照片详细信息（分辨率/文件大小/像素分布图，按需实时读取）---- */
const albumStore = useAlbumStore();
const contentStore = useContentStore();
const photoInfo = ref<PhotoInfo | null>(null);
const infoError = ref("");
const histCanvas = ref<HTMLCanvasElement | null>(null);

/** FEAT-D：扫描状态机（"未扫描" / "扫描中" / "已扫描" / "扫描失败"）
 *  - scanning: 静默后台扫描进行中
 *  - scanFailed: 后端服务未运行/其他原因扫描失败
 *  - 三者独立于 meta：meta 未填时驱动用户可见状态
 */
const scanState = ref<"idle" | "scanning" | "failed" | "done">("idle");

/** FEAT-D：实际用于面板展示的 meta —— 优先用本地缓存（扫描后），其次用 props 传入 */
const meta = computed<AlbumContentRow | undefined>(() => {
  const cur = photo.value;
  return metaOverrides.value[cur.path] ?? cur.meta;
});

async function loadPhotoInfo(path: string) {
  photoInfo.value = null;
  infoError.value = "";
  try {
    photoInfo.value = await albumStore.getPhotoInfo(path);
  } catch (e) {
    infoError.value = String(e);
  }
}

/** RGB 三通道直方图绘制（screen 叠加模式，重叠处趋白） */
function drawHistogram() {
  const cv = histCanvas.value;
  const info = photoInfo.value;
  if (!cv || !info || !info.hist_r.length) return;
  const ctx = cv.getContext("2d");
  if (!ctx) return;
  const w = cv.width;
  const h = cv.height;
  ctx.clearRect(0, 0, w, h);
  // 背景网格
  ctx.strokeStyle = "rgba(255,255,255,0.08)";
  ctx.beginPath();
  for (let i = 1; i < 4; i++) {
    ctx.moveTo((i * w) / 4, 0);
    ctx.lineTo((i * w) / 4, h);
  }
  ctx.stroke();
  const peak = Math.max(
    ...info.hist_r,
    ...info.hist_g,
    ...info.hist_b,
    1
  );
  ctx.globalCompositeOperation = "screen";
  const channels: Array<[number[], string]> = [
    [info.hist_r, "rgba(255,70,70,0.85)"],
    [info.hist_g, "rgba(70,220,120,0.85)"],
    [info.hist_b, "rgba(90,140,255,0.85)"],
  ];
  for (const [hist, color] of channels) {
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(0, h);
    for (let i = 0; i < 256; i++) {
      const x = (i / 255) * w;
      const y = h - (hist[i] / peak) * (h - 2);
      ctx.lineTo(x, y);
    }
    ctx.lineTo(w, h);
    ctx.closePath();
    ctx.fill();
  }
  ctx.globalCompositeOperation = "source-over";
}

watch([photoInfo, histCanvas], () => nextTick(drawHistogram), { deep: false });

// 切换照片时重新加载信息（immediate 覆盖首次打开）
watch(
  () => photo.value.path,
  (p) => {
    void loadPhotoInfo(p);
    void tryEnsureScanned(p);
  },
  { immediate: true },
);

/**
 * FEAT-D：保证当前照片已扫描并落库。
 * - 若 photo.meta 已存在（overrides 或 props 传入）→ 跳过（避免重复 IO）。
 * - 调 ensure_photo_scanned；后端命中库内已有记录直接返回 / 否则单张扫描并落库。
 * - 扫描完成后写入 metaOverrides（path → row），供 meta computed 查找，
 *   不依赖 props 数组项引用（避逸 Vue props 不可变问题）。
 * - 扫描期间 scanState = "scanning" 用于面板提示用户正在后台识别。
 */
const scanningPhoto = ref(false);
/** path → 扫描后落库的 row（FEAT-D 缓存层） */
const metaOverrides = ref<Record<string, AlbumContentRow>>({});

async function tryEnsureScanned(path: string) {
  if (metaOverrides.value[path] || props.photos.find((p) => p.path === path)?.meta) {
    scanState.value = "done";
    return;
  }
  const p = photo.value;
  if (p.albumId == null) {
    // 非相册上下文（Memories / Timeline）不触发
    scanState.value = "idle";
    return;
  }
  if (scanningPhoto.value) return;
  scanningPhoto.value = true;
  scanState.value = "scanning";
  try {
    const row = await contentStore.ensurePhotoScanned(p.albumId, path);
    if (row) {
      metaOverrides.value = { ...metaOverrides.value, [path]: row };
      scanState.value = "done";
    } else {
      // 后端返回 null（服务未运行 / 识别失败 / 照片丢失）—— 提示用户“未扫描”
      scanState.value = "failed";
    }
  } catch {
    scanState.value = "failed";
  } finally {
    scanningPhoto.value = false;
  }
}

/** FEAT-D：手动重试扫描（点击面板中的“手动重试”按钮） */
async function retryScan() {
  if (scanningPhoto.value) return;
  await tryEnsureScanned(photo.value.path);
}

/** 字节数人性化显示 */
function fmtSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return (bytes / 1024 / 1024).toFixed(2) + " MB";
  if (bytes >= 1024) return (bytes / 1024).toFixed(1) + " KB";
  return bytes + " B";
}

/** 人物显示名：自定义命名优先，否则回退编号 */
function personLabel(pid: string): string {
  const name = props.persons?.[pid];
  return name && name !== pid ? `${name}（${pid}）` : pid;
}
</script>

<template>
  <div ref="overlayEl" class="lb-overlay" @click.self="emit('close')" @touchstart="onTouchStart" @touchend="onTouchEnd">
    <button class="lb-close" title="关闭 (Esc)" @click="emit('close')">✕</button>

    <!-- 操作提示与当前倍率 -->
    <div class="lb-zoombar">
      <span class="lb-hint">
        <kbd>←</kbd> <kbd>→</kbd> 切图 · <kbd>Esc</kbd> 关闭 · <kbd>Ctrl</kbd>+滚轮缩放 · 双击放大/复原
      </span>
      <span v-if="scale !== 1" class="lb-scale">{{ Math.round(scale * 100) }}%</span>
    </div>

    <button v-if="photos.length > 1" class="lb-nav lb-prev" title="上一张 (←)" @click="prev">‹</button>
    <button v-if="photos.length > 1" class="lb-nav lb-next" title="下一张 (→)" @click="next">›</button>

    <div
      class="lb-main"
      :class="{ pannable: scale > 1, dragging }"
      @pointerdown="onDragStart"
      @pointermove="onDragMove"
      @pointerup="onDragEnd"
      @pointercancel="onDragEnd"
      @dblclick.prevent="toggleZoom"
    >
      <div v-show="imgLoading" class="lb-loading">加载中…</div>
      <!-- 直接指向原文件路径（convertFileSrc），非缩略图 -->
      <img
        :src="fileUrl(photo.path)"
        @load="imgLoading = false"
        class="lb-img"
        :class="{ hidden: imgLoading }"
        :style="{ transform: `translate(${tx}px, ${ty}px) scale(${scale})` }"
        alt=""
        draggable="false"
      />
    </div>

    <!-- 元数据面板 -->
    <aside class="lb-meta">
      <h4>照片信息</h4>
      <div class="lb-filename">{{ photo.path }}</div>

      <dl>
        <dt>分辨率</dt><dd v-if="photoInfo">{{ photoInfo.width }} × {{ photoInfo.height }} px（{{ (photoInfo.width * photoInfo.height / 1e6).toFixed(1) }} MP）</dd>
        <template v-else-if="infoError"><dt>提示</dt><dd>无法读取图片信息</dd></template>
        <template v-if="photoInfo">
          <dt>文件大小</dt><dd>{{ fmtSize(photoInfo.file_size) }}</dd>
          <dt>格式</dt><dd>{{ photoInfo.format.toUpperCase() }}</dd>
        </template>
        <template v-if="meta?.category">
          <dt>AI 分类</dt><dd>{{ meta.category }}<span v-if="meta.sub_category"> / {{ meta.sub_category }}</span></dd>
          <dt v-if="meta.label">标签</dt><dd v-if="meta.label">{{ meta.label }}</dd>
          <dt v-if="meta.confidence">置信度</dt><dd v-if="meta.confidence">{{ (meta.confidence * 100).toFixed(1) }}%</dd>
        </template>
        <template v-if="(meta?.person_ids ?? []).length">
          <dt>人物</dt>
          <dd>{{ meta!.person_ids.map(personLabel).join("、") }}<span v-if="meta!.person_count > 0">（{{ meta!.person_count }}）</span></dd>
        </template>
        <template v-if="meta?.shoot_time">
          <dt>拍摄时间</dt><dd>{{ meta.shoot_time }}</dd>
        </template>
        <template v-if="meta?.iso || meta?.iso_num">
          <dt>ISO</dt><dd>{{ meta.iso ?? meta.iso_num }}</dd>
        </template>
        <template v-if="meta?.aperture || meta?.aperture_num">
          <dt>光圈</dt><dd>f/{{ meta.aperture ?? meta.aperture_num }}</dd>
        </template>
        <template v-if="meta?.shutter_speed || meta?.shutter_num">
          <dt>快门</dt><dd>{{ meta.shutter_speed ?? meta.shutter_num }}</dd>
        </template>
        <template v-if="meta?.focal_length || meta?.focal_num">
          <dt>焦距</dt><dd>{{ meta.focal_length ?? meta.focal_num }}mm</dd>
        </template>
        <template v-if="meta?.tone_type">
          <dt>影调</dt><dd>{{ meta.tone_type }}<span v-if="meta.avg_luma != null">（亮度 {{ meta.avg_luma.toFixed(1) }}）</span></dd>
        </template>
        <template v-if="!meta && scanState === 'scanning'">
          <dt>AI 识别</dt><dd class="lb-scanning">⏳ 正在后台识别该照片的 AI/EXIF 信息…识别完成后面板自动刷新。</dd>
        </template>
        <template v-else-if="!meta && scanState === 'failed'">
          <dt>提示</dt>
          <dd>
            尚未扫描该照片的 AI/EXIF 信息。
            <br />运行相册详情页的「<b>综合扫描</b>」后即可在此显示分类、人物与拍摄参数。
            <br /><button class="lb-retry-btn" :disabled="scanningPhoto" @click="retryScan">⟳ 重试扫描</button>
          </dd>
        </template>
        <template v-else-if="!meta">
          <dt>提示</dt>
          <dd>
            尚未扫描该照片的 AI/EXIF 信息。
            <br />运行相册详情页的「<b>综合扫描</b>」后即可在此显示分类、人物与拍摄参数。
            <br /><button class="lb-retry-btn" :disabled="scanningPhoto" @click="retryScan">⟳ 立即扫描</button>
          </dd>
        </template>
      </dl>

      <!-- 像素分布图（RGB 三通道直方图，screen 叠加） -->
      <div v-if="photoInfo?.hist_r?.length" class="lb-hist-wrap">
        <div class="lb-hist-title">像素分布（R/G/B）</div>
        <canvas ref="histCanvas" width="300" height="90" class="lb-hist"></canvas>
      </div>

      <div class="lb-counter">{{ current + 1 }} / {{ photos.length }}</div>
    </aside>
  </div>
</template>

<style scoped>
.lb-overlay {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(0, 0, 0, 0.92);
  display: flex;
  align-items: center;
  justify-content: center;
}

.lb-close {
  position: absolute;
  top: 16px;
  right: 16px;
  z-index: 2;
  background: rgba(255, 255, 255, 0.15);
  border: none;
  color: #fff;
  font-size: 22px;
  width: 40px;
  height: 40px;
  border-radius: 50%;
  cursor: pointer;
  line-height: 1;
}
.lb-close:hover { background: rgba(255, 255, 255, 0.3); }

.lb-nav {
  position: absolute;
  top: 50%;
  transform: translateY(-50%);
  z-index: 2;
  background: rgba(255, 255, 255, 0.15);
  border: none;
  color: #fff;
  font-size: 40px;
  width: 56px;
  height: 80px;
  cursor: pointer;
  line-height: 1;
}
.lb-nav:hover { background: rgba(255, 255, 255, 0.3); }
.lb-prev { left: 12px; }
.lb-next { right: 12px; }

.lb-main {
  position: relative;
  max-width: 74vw;
  max-height: 86vh;
  display: flex;
  align-items: center;
  justify-content: center;
}

.lb-main.pannable {
  cursor: grab;
  overflow: visible;
}
.lb-main.pannable.dragging {
  cursor: grabbing;
}

.lb-img {
  max-width: 74vw;
  max-height: 86vh;
  object-fit: contain;
  display: block;
  transition: transform 0.12s ease-out;
  will-change: transform;
  transform-origin: center center;
  user-select: none;
}
.lb-img.hidden { visibility: hidden; }

/* 顶部操作提示 + 缩放倍率 */
.lb-zoombar {
  position: absolute;
  top: 16px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 2;
  display: flex;
  align-items: center;
  gap: 12px;
  background: rgba(255, 255, 255, 0.12);
  border-radius: 999px;
  padding: 6px 16px;
  backdrop-filter: blur(4px);
}
.lb-hint {
  color: rgba(255, 255, 255, 0.75);
  font-size: 12px;
}
.lb-hint kbd {
  display: inline-block;
  min-width: 22px;
  padding: 1px 6px;
  margin: 0 1px;
  font-size: 11px;
  font-family: inherit;
  line-height: 1.4;
  color: #fff;
  background: rgba(255, 255, 255, 0.18);
  border: 1px solid rgba(255, 255, 255, 0.35);
  border-radius: 4px;
  vertical-align: 1px;
}
.lb-scale {
  color: #fff;
  font-size: 13px;
  font-weight: 600;
  min-width: 44px;
  text-align: center;
}

.lb-loading {
  position: absolute;
  color: #fff;
  font-size: 14px;
}

.lb-meta {
  position: absolute;
  left: 16px;
  bottom: 16px;
  background: rgba(0, 0, 0, 0.6);
  color: #e7e9ee;
  border-radius: 10px;
  padding: 14px 18px;
  max-width: 340px;
  backdrop-filter: blur(4px);
  font-size: 13px;
}
.lb-meta h4 { margin: 0 0 8px; color: #fff; font-size: 14px; }
.lb-filename {
  font-size: 12px;
  color: #aab0bd;
  word-break: break-all;
  margin-bottom: 8px;
}
.lb-meta dl {
  margin: 0;
  display: grid;
  grid-template-columns: 72px 1fr;
  gap: 4px 10px;
}
.lb-meta dt { color: #9aa1ae; }
.lb-meta dd { margin: 0; color: #e7e9ee; }
.lb-counter {
  margin-top: 10px;
  font-size: 12px;
  color: #aab0bd;
}

/* FEAT-D：扫描中/未扫描提示 */
.lb-scanning {
  color: #ffd43b;
  font-size: 12.5px;
  line-height: 1.5;
  animation: lb-scan-pulse 1.2s ease-in-out infinite;
}
@keyframes lb-scan-pulse {
  0%, 100% { opacity: 0.7; }
  50% { opacity: 1; }
}
.lb-retry-btn {
  margin-top: 6px;
  background: rgba(255, 255, 255, 0.12);
  color: #fff;
  border: 1px solid rgba(255, 255, 255, 0.35);
  border-radius: 6px;
  padding: 3px 10px;
  font-size: 12px;
  cursor: pointer;
  transition: background 0.15s, transform 0.1s;
}
.lb-retry-btn:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.22);
  transform: translateY(-1px);
}
.lb-retry-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* 像素分布图（RGB 直方图） */
.lb-hist-wrap {
  margin-top: 10px;
}
.lb-hist-title {
  font-size: 12px;
  color: #9aa1ae;
  margin-bottom: 4px;
}
.lb-hist {
  width: 100%;
  height: 90px;
  display: block;
  background: rgba(255, 255, 255, 0.06);
  border-radius: 6px;
}
</style>
