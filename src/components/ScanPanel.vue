<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { listen } from "@tauri-apps/api/event";
import { useContentStore } from "../stores/content";
import type { ScanReport, UnifiedScanRow, VcrGpuStatus } from "../types/content";
import type { ClassifyProgress, PersonInfo, PhotoExif, PhotoTone, VisionResult } from "../types/photo";
import { trace } from "../utils/trace";
import PersonPanel from "./PersonPanel.vue";
import ContentSearch from "./ContentSearch.vue";

const props = defineProps<{ albumId: number; albumPath: string }>();
const contentStore = useContentStore();

// ===================== EXIF 扫描（测试功能） =====================
const scanning = ref(false);
const placeScanning = ref(false);
const localScanning = ref(false);
const scanError = ref("");
const scanResult = ref<PhotoExif[] | null>(null);

const scanPhotos = trace("scanPhotos", async () => {
  if (scanning.value) return;
  scanning.value = true;
  scanError.value = "";
  try {
    scanResult.value = await invoke<PhotoExif[]>("scan_album_photos", { path: props.albumPath });
  } catch (e) {
    scanError.value = `扫描失败：${e}`;
  } finally {
    scanning.value = false;
  }
});

const scanPhotosLocalPlace = trace("scanPhotosLocalPlace", async () => {
  if (localScanning.value) return;
  localScanning.value = true;
  scanError.value = "";
  try {
    scanResult.value = await invoke<PhotoExif[]>("scan_album_photos_local_place", { path: props.albumPath });
  } catch (e) {
    scanError.value = `本地地名查询失败：${e}`;
  } finally {
    localScanning.value = false;
  }
});

const scanPhotosWithPlace = trace("scanPhotosWithPlace", async () => {
  if (placeScanning.value) return;
  placeScanning.value = true;
  scanError.value = "";
  try {
    scanResult.value = await invoke<PhotoExif[]>("scan_album_photos_with_place", { path: props.albumPath });
  } catch (e) {
    scanError.value = `地名查询失败：${e}`;
  } finally {
    placeScanning.value = false;
  }
});

function fmtCoord(lat: number, lon: number): string {
  const la = `${Math.abs(lat).toFixed(4)}°${lat >= 0 ? "N" : "S"}`;
  const lo = `${Math.abs(lon).toFixed(4)}°${lon >= 0 ? "E" : "W"}`;
  return `${la}, ${lo}`;
}

// ===================== 影调分析（测试功能） =====================
const toneScanning = ref(false);
const toneError = ref("");
const toneResult = ref<PhotoTone[] | null>(null);

const scanTones = trace("scanTones", async () => {
  if (toneScanning.value) return;
  toneScanning.value = true;
  toneError.value = "";
  try {
    toneResult.value = await invoke<PhotoTone[]>("scan_album_tones", { path: props.albumPath });
  } catch (e) {
    toneError.value = `影调分析失败：${e}`;
  } finally {
    toneScanning.value = false;
  }
});

function toneLabel(t: PhotoTone["tone_type"]): string {
  switch (t) {
    case "low-key": return "低调";
    case "mid-key": return "中间调";
    case "high-key": return "高调";
    default: return "—";
  }
}

function histBars(h: number[]): { k: number; v: number }[] {
  const bins = 128;
  const merged = new Array(bins).fill(0) as number[];
  for (let i = 0; i < h.length; i++) {
    merged[Math.floor((i * bins) / h.length)] += h[i];
  }
  const max = Math.max(...merged, 1);
  return merged.map((v, k) => ({ k, v: v / max }));
}

// ===================== AI 内容识别（测试功能） =====================
const visionScanning = ref(false);
const visionError = ref("");
const visionScanMessage = ref("");
const visionResult = ref<VisionResult[] | null>(null);
const visionProgress = ref<ClassifyProgress | null>(null);
let unlistenProgress: (() => void) | null = null;

// 批次与 GPU
const scanBatch = ref(8);
const BATCH_OPTIONS = [8, 16, 32];
const gpuStatus = ref<VcrGpuStatus | null>(null);
const gpuLoading = ref(false);

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

function gpuStatusText(): string {
  const g = gpuStatus.value;
  if (!g) return "未检测";
  if (!g.running) return "服务未运行";
  if (g.use_gpu) return `GPU 加速已启用（${g.provider}）`;
  return `CPU 推理（未检测到可用 GPU${g.gpu.length ? "；已安装 GPU 提供方 " + g.gpu.join(",") : ""}）`;
}

// 分页
const PAGE_SIZES = [10, 20, 50, 100];
const pageSize = ref(50);
const currentPage = ref(1);

const pagedVision = computed(() => {
  if (!visionResult.value) return [];
  const start = (currentPage.value - 1) * pageSize.value;
  return visionResult.value.slice(start, start + pageSize.value);
});

const visionPageCount = computed(() =>
  Math.max(1, Math.ceil((visionResult.value?.length ?? 0) / pageSize.value))
);

const visionRowOffset = computed(() => (currentPage.value - 1) * pageSize.value);

watch(pageSize, () => { currentPage.value = 1; });
watch([visionResult, pageSize], () => {
  if (currentPage.value > visionPageCount.value) currentPage.value = visionPageCount.value;
});

function changeVisionPage(p: number) {
  currentPage.value = Math.min(Math.max(1, p), visionPageCount.value);
}

// 人物
const persons = ref<PersonInfo[]>([]);

const loadPersons = trace("loadPersons", async () => {
  try {
    persons.value = await invoke<PersonInfo[]>("list_persons");
  } catch {
    persons.value = [];
  }
});

// 分类标签映射
function categoryLabel(cat: string): string {
  const map: Record<string, string> = {
    animal: "动物", animal_pet: "动物", food: "食物", flower: "花朵",
    plant: "植物", plant_flower: "植物花卉", architecture: "建筑",
    cityscape: "城市风光", sports: "运动", landscape_nature: "自然风景",
    landscape: "自然风景", text: "文本截图", document: "文档",
    vehicle: "车辆", portrait: "人物特写", street: "扫街",
    night_scene: "夜景", other: "其他",
  };
  return map[cat] ?? cat;
}

function animalSubLabel(sub: string): string {
  const map: Record<string, string> = { dog: "狗", cat: "猫", bird: "鸟", flower: "花", plant: "植物" };
  return map[sub] ?? sub ?? "";
}

const openImage = trace("openImage", async (path: string) => {
  try {
    await openPath(path);
  } catch (e) {
    alert(`无法打开图片：${path}\n\n${e}`);
  }
});

const classifyAlbum = trace("classifyAlbum", async () => {
  if (visionScanning.value) return;
  visionScanning.value = true;
  visionError.value = "";
  visionProgress.value = null;
  currentPage.value = 1;
  try {
    if (!unlistenProgress) {
      unlistenProgress = await listen<ClassifyProgress>("classify-progress", (e) => {
        visionProgress.value = e.payload;
      });
    }
    const outcome = await contentStore.scanAlbumContent(props.albumId, scanBatch.value);
    visionResult.value = outcome.results;
    const r = outcome.report;
    visionScanMessage.value = `✅ 已写入内容库 ${r.written}/${r.total} 张${r.failed ? `，失败 ${r.failed}` : ""}；识别结果已可用于照片内容搜索`;
    await loadPersons();
    fetchGpuStatus();
  } catch (e) {
    visionError.value = `内容识别失败：${e}`;
  } finally {
    visionScanning.value = false;
  }
});

// ===================== 组合扫描（FEAT-026） =====================
const toneLabelMap: Record<string, string> = {
  "low-key": "低调", "mid-key": "中间调", "high-key": "高调",
  LowKey: "低调", MidKey: "中间调", HighKey: "高调",
};

const comboScanTypes = ref<string[]>([]);
const comboBatch = ref(8);
const comboScanning = ref(false);
const comboError = ref("");
const comboRows = ref<UnifiedScanRow[]>([]);
const comboReport = ref<ScanReport | null>(null);
const readScanning = ref(false);

const scanAlbumCombined = trace("scanAlbumCombined", async () => {
  if (comboScanning.value) return;
  if (comboScanTypes.value.length === 0) {
    comboError.value = "请至少勾选一项扫描类型";
    return;
  }
  comboScanning.value = true;
  comboError.value = "";
  try {
    const result = await contentStore.scanAlbumCombined(props.albumId, comboScanTypes.value, comboBatch.value);
    comboReport.value = result.report;
  } catch (e) {
    comboError.value = `组合扫描失败：${e}`;
  } finally {
    comboScanning.value = false;
  }
});

const readAlbumContent = trace("readAlbumContent", async () => {
  if (readScanning.value) return;
  readScanning.value = true;
  try {
    const { rows } = await contentStore.readAlbumContent(props.albumId, 1, 1000);
    comboRows.value = rows.map((r) => ({
      file_name: r.path.split("/").pop()?.split("\\").pop() ?? r.path,
      path: r.path,
      iso: r.iso, aperture: r.aperture, shutter_speed: r.shutter_speed,
      focal_length: r.focal_length, shoot_time: r.shoot_time,
      iso_num: r.iso_num, focal_num: r.focal_num,
      aperture_num: r.aperture_num, shutter_num: r.shutter_num,
      tone_type: r.tone_type, avg_luma: r.avg_luma,
      category: r.category, sub_category: r.sub_category,
      label: r.label, confidence: r.confidence,
      top3: [] as VisionResult["top3"],
      person_ids: r.person_ids, person_count: r.person_count,
    }));
  } catch (e) {
    comboError.value = `读表失败：${e}`;
  } finally {
    readScanning.value = false;
  }
});
</script>

<template>
  <!-- ============ 组合扫描（FEAT-026） ============ -->
  <section class="scan-area combo-area">
    <div class="scan-toolbar">
      <div>
        <h3 class="scan-title">🧩 组合扫描</h3>
        <p class="scan-sub">勾选扫描类型（可多选），一次完成 EXIF / 影调 / AI 内容识别；勾选「内容识别」时结果同时写入内容库，可用于智能搜索</p>
      </div>
      <div class="combo-actions">
        <button class="btn btn-primary" :disabled="comboScanning" @click="scanAlbumCombined">
          {{ comboScanning ? "扫描中…" : "开始组合扫描" }}
        </button>
      </div>
    </div>

    <div class="combo-controls">
      <div class="combo-checks">
        <label class="combo-check" :class="{ active: comboScanTypes.includes('basic') }">
          <input type="checkbox" value="basic" v-model="comboScanTypes" />
          <span class="combo-check-label">EXIF 基础</span>
          <span class="combo-check-desc">ISO / 焦段 / 光圈 / 快门</span>
        </label>
        <label class="combo-check" :class="{ active: comboScanTypes.includes('tone') }">
          <input type="checkbox" value="tone" v-model="comboScanTypes" />
          <span class="combo-check-label">影调分析</span>
          <span class="combo-check-desc">低调 / 中间调 / 高调</span>
        </label>
        <label class="combo-check" :class="{ active: comboScanTypes.includes('ai') }">
          <input type="checkbox" value="ai" v-model="comboScanTypes" />
          <span class="combo-check-label">AI 内容识别</span>
          <span class="combo-check-desc">写入内容库 · 支持搜索</span>
        </label>
      </div>
      <div class="combo-meta">
        <label class="batch-select">批次<select v-model="comboBatch"><option v-for="b in BATCH_OPTIONS" :key="b" :value="b">{{ b }}</option></select></label>
        <button class="btn btn-mini" :disabled="gpuLoading" @click="fetchGpuStatus">{{ gpuLoading ? "检测中…" : "检测 GPU" }}</button>
        <div v-if="gpuStatus" class="gpu-info" :class="{ ok: gpuStatus.use_gpu }">GPU: {{ gpuStatus.use_gpu ? "✅ " + gpuStatus.provider : "❌ CPU (" + gpuStatus.provider + ")" }}</div>
      </div>
    </div>

    <p v-if="comboError" class="scan-error">{{ comboError }}</p>
    <div v-if="!comboScanning && !comboRows.length && !comboError" class="scan-empty"><p>请勾选扫描类型后点击「开始组合扫描」</p></div>
    <div v-if="comboScanning" class="scan-empty"><p class="scan-loading">⏳ 正在扫描… 请稍候</p></div>

    <div v-if="comboRows.length" class="scan-table-wrap">
      <div class="scan-toolbar" style="padding:10px 14px 0;border:none">
        <p class="scan-sub">{{ comboReport ? "✅ 已写入内容库 " + comboReport.written + "/" + comboReport.total + " 张" : "✅ 扫描完成" }}</p>
        <div class="scan-actions" style="display:flex;gap:8px;flex-wrap:wrap">
          <button class="btn btn-primary" :disabled="comboScanning" @click="scanAlbumCombined">重新扫描</button>
          <button class="btn btn-ghost" :disabled="readScanning" @click="readAlbumContent">{{ readScanning ? "读取中…" : "📖 读表" }}</button>
        </div>
      </div>
      <table class="scan-table combo-table">
        <thead><tr><th class="col-idx">#</th><th>照片名字</th><th>ISO</th><th>焦段</th><th>光圈</th><th>快门</th><th>影调</th><th>AI 类别</th><th>细类</th></tr></thead>
        <tbody>
          <tr v-for="(r, i) in comboRows" :key="r.path">
            <td class="col-idx">{{ i + 1 }}</td>
            <td class="col-name" :title="r.path"><span class="img-name-link" @click="openImage(r.path)">{{ r.file_name }}</span></td>
            <td>{{ r.iso ?? r.iso_num != null ? "ISO " + r.iso_num : "—" }}</td>
            <td>{{ r.focal_length ?? r.focal_num != null ? r.focal_num + "mm" : "—" }}</td>
            <td>{{ r.aperture ?? r.aperture_num != null ? "f/" + r.aperture_num : "—" }}</td>
            <td>{{ r.shutter_speed ?? (r.shutter_num ? (1 / r.shutter_num).toFixed(1) + "s" : "—") }}</td>
            <td><span v-if="r.tone_type" class="tone-badge tone-" :class="r.tone_type">{{ toneLabelMap[r.tone_type] || r.tone_type }}</span><span v-else>—</span></td>
            <td>{{ r.category ?? "—" }}</td>
            <td>{{ r.label ?? "—" }}</td>
          </tr>
        </tbody>
      </table>
      <p class="scan-count">共 {{ comboRows.length }} 张图片</p>
    </div>
  </section>

  <!-- ============ 图片 EXIF 扫描（测试功能） ============ -->
  <section class="scan-area">
    <div class="scan-toolbar">
      <div>
        <h3 class="scan-title">图片 EXIF 扫描</h3>
        <p class="scan-sub">扫描相册目录内所有图片的拍摄参数（ISO / 焦段 / 光圈 / 快门速度 / 拍摄时间），点击照片名字可用默认看图程序打开原图</p>
      </div>
      <button class="btn btn-primary" :disabled="scanning" @click="scanPhotos">{{ scanning ? "扫描中…" : scanResult ? "重新扫描" : "开始扫描" }}</button>
    </div>
    <p v-if="scanError" class="scan-error">{{ scanError }}</p>
    <div v-if="!scanning && !scanResult && !scanError" class="scan-empty"><p>点击「开始扫描」查看相册内照片的 EXIF 信息</p></div>
    <div v-if="scanning" class="scan-empty"><p class="scan-loading">⏳ 正在扫描… 请稍候</p></div>
    <div v-if="scanResult" class="scan-table-wrap">
      <div class="scan-toolbar" style="padding:10px 14px 0;border:none">
        <p class="scan-sub">地点列：默认显示坐标链接（点击打开地图定位）；「本地省/市」秒回离线反查，未命中（国外/公海）仍显示坐标链接；「精确地名」联网反查到区县级</p>
        <div class="scan-actions" style="display:flex;gap:8px;flex-wrap:wrap">
          <button class="btn btn-primary" :disabled="scanning || placeScanning || localScanning" @click="scanPhotos">重新扫描</button>
          <button class="btn btn-ghost" :disabled="placeScanning || localScanning" @click="scanPhotosLocalPlace">{{ localScanning ? "本地解析中…" : "本地省/市（秒回·离线）" }}</button>
          <button class="btn btn-ghost" :disabled="placeScanning || localScanning" @click="scanPhotosWithPlace">{{ placeScanning ? "地名查询中…" : "精确地名（联网）" }}</button>
        </div>
      </div>
      <table class="scan-table">
        <thead><tr><th class="col-idx">#</th><th>照片名字</th><th>ISO</th><th>焦段</th><th>光圈</th><th>快门速度</th><th>拍摄时间</th><th>地点</th></tr></thead>
        <tbody>
          <tr v-for="(p, i) in scanResult" :key="p.path">
            <td class="col-idx">{{ i + 1 }}</td>
            <td class="col-name" :title="p.path"><span class="img-name-link" @click="openImage(p.path)">{{ p.file_name }}</span></td>
            <td>{{ p.iso ?? "—" }}</td>
            <td>{{ p.focal_length ?? "—" }}</td>
            <td>{{ p.aperture ?? "—" }}</td>
            <td>{{ p.shutter_speed ?? "—" }}</td>
            <td>{{ p.shoot_time ?? "—" }}</td>
            <td class="col-place">
              <template v-if="p.place">{{ p.place }}</template>
              <template v-else-if="p.lat !== null && p.lon !== null">
                <a v-if="p.map_url" :href="p.map_url" target="_blank" rel="noopener" class="gps-link">{{ fmtCoord(p.lat, p.lon) }}</a>
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

  <!-- ============ 图片影调分析（测试功能） ============ -->
  <section class="scan-area tone-area">
    <div class="scan-toolbar">
      <div>
        <h3 class="scan-title">图片影调分析</h3>
        <p class="scan-sub">下采样后统计灰度直方图，按平均亮度法判断影调（低调 &lt; 85 ≤ 中间调 ≤ 170 &lt; 高调），点击照片名字可用默认看图程序打开原图</p>
      </div>
      <button class="btn btn-primary" :disabled="toneScanning" @click="scanTones">{{ toneScanning ? "分析中…" : toneResult ? "重新分析" : "开始分析" }}</button>
    </div>
    <p v-if="toneError" class="scan-error">{{ toneError }}</p>
    <div v-if="!toneScanning && !toneResult && !toneError" class="scan-empty"><p>点击「开始分析」查看相册内照片的灰度分布与影调类型</p></div>
    <div v-if="toneScanning" class="scan-empty"><p class="scan-loading">⏳ 正在解码分析… 请稍候</p></div>
    <div v-if="toneResult" class="scan-table-wrap">
      <svg width="0" height="0" style="position:absolute"><defs><linearGradient id="hist-grad" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#396cd8" /><stop offset="100%" stop-color="#8fb3f0" /></linearGradient></defs></svg>
      <table class="scan-table tone-table">
        <thead><tr><th class="col-idx">#</th><th>照片名字</th><th>影调类型</th><th>平均亮度 L̄</th><th>灰度柱状图分布</th></tr></thead>
        <tbody>
          <tr v-for="(p, i) in toneResult" :key="p.path">
            <td class="col-idx">{{ i + 1 }}</td>
            <td class="col-name" :title="p.path"><span class="img-name-link" @click="openImage(p.path)">{{ p.file_name }}</span></td>
            <td><span class="tone-badge" :class="p.tone_type ? 'tone-' + p.tone_type : ''">{{ toneLabel(p.tone_type) }}</span></td>
            <td class="col-luma">{{ p.avg_luma != null ? p.avg_luma.toFixed(1) : "—" }}</td>
            <td class="col-hist">
              <svg v-if="p.histogram.length > 0" viewBox="0 0 256 36" preserveAspectRatio="none" class="mini-hist">
                <rect v-for="b in histBars(p.histogram)" :key="b.k" :x="b.k * 2" :width="2" :height="1 + b.v * 33" :y="36 - (1 + b.v * 33)" fill="url(#hist-grad)" />
              </svg>
              <span v-else class="hist-empty">—</span>
            </td>
          </tr>
        </tbody>
      </table>
      <p class="scan-count">共 {{ toneResult.length }} 张图片（下采样至 256px 后统计）</p>
    </div>
  </section>

  <!-- ============ 内容识别（YOLOv8n-cls，测试功能） ============ -->
  <section class="scan-area vision-area">
    <div class="scan-toolbar">
      <div>
        <h3 class="scan-title">内容识别</h3>
        <p class="scan-sub">三路识别（YOLOv8s-cls 分类 + YOLOv8n 检测 + Places365 场景）融合为相册大类：人物特写/扫街/自然风景/建筑城市/动物/文本等；人物自动标号（同人同 P 编号），动物细分狗/猫/鸟。点击照片名字可用默认看图程序打开原图</p>
      </div>
      <div class="scan-tool-actions">
        <label class="batch-select">批次<select v-model="scanBatch" title="推理批次：每批提交识别的图片数"><option v-for="b in BATCH_OPTIONS" :key="b" :value="b">{{ b }}</option></select></label>
        <button class="btn btn-mini" :disabled="gpuLoading" @click="fetchGpuStatus" title="检测 GPU 加速可行性">{{ gpuLoading ? "检测中…" : "检测 GPU" }}</button>
        <button class="btn btn-primary" :disabled="visionScanning" @click="classifyAlbum">{{ visionScanning ? "识别中…" : visionResult ? "重新识别" : "开始识别" }}</button>
      </div>
    </div>

    <p v-if="gpuStatus" class="gpu-status" :class="{ 'gpu-on': gpuStatus.use_gpu }">🖥 {{ gpuStatusText() }}</p>
    <p v-if="visionError" class="scan-error">{{ visionError }}</p>
    <p v-if="visionScanMessage && !visionError" class="scan-ok">{{ visionScanMessage }}</p>
    <div v-if="!visionScanning && !visionResult && !visionError" class="scan-empty"><p>点击「开始识别」用 AI 识别相册图片内容（首次会启动识别服务，需等待约 10 秒）</p></div>

    <div v-if="visionScanning || (visionProgress && visionProgress.total > 0)" class="vision-progress">
      <div class="progress-track"><div class="progress-fill" :style="{ width: (visionProgress ? (visionProgress.current / visionProgress.total) * 100 : 0) + '%' }"></div></div>
      <p v-if="visionProgress" class="progress-text">{{ visionProgress.current }} / {{ visionProgress.total }} 张（成功 {{ visionProgress.done }} · 失败 {{ visionProgress.failed }}）</p>
    </div>

    <div v-if="visionResult" class="scan-table-wrap">
      <table class="scan-table vision-table">
        <thead><tr><th class="col-idx">#</th><th>照片名字</th><th>内容类别</th><th>识别细类</th><th>置信度</th><th>Top3 候选</th><th>耗时</th></tr></thead>
        <tbody>
          <tr v-for="(p, i) in pagedVision" :key="p.path">
            <td class="col-idx">{{ visionRowOffset + i + 1 }}</td>
            <td><span class="img-name-link" :title="p.path" @click="openImage(p.path)">{{ p.file_name }}</span></td>
            <td>
              <template v-if="p.error"><span class="vision-err">{{ p.error }}</span></template>
              <template v-else>
                <span class="vision-cat">{{ categoryLabel(p.category) }}</span>
                <span v-if="p.sub_category" class="vision-sub">{{ animalSubLabel(p.sub_category) }}</span>
                <span v-for="pid in p.person_ids" :key="pid" class="person-chip" :title="'同人标号：此人与其他 P 编号照片为同一人'">{{ pid }}</span>
              </template>
            </td>
            <td class="col-label">{{ p.label || "—" }}<span v-if="p.person_count > 1" class="ppl-note">{{ p.person_count }} 人</span></td>
            <td><span class="conf-bar"><span class="conf-fill" :style="{ width: Math.round(p.confidence * 100) + '%' }"></span></span><span class="conf-val">{{ (p.confidence * 100).toFixed(1) }}%</span></td>
            <td class="col-top3"><template v-if="p.top3.length"><span v-for="(t, ti) in p.top3" :key="ti" class="top3-chip">{{ categoryLabel(t.category) }}·{{ t.label }}</span></template><span v-else>—</span></td>
            <td class="col-luma">{{ p.elapsed_ms ? p.elapsed_ms.toFixed(0) + "ms" : "—" }}</td>
          </tr>
        </tbody>
      </table>
      <!-- 分页 -->
      <div class="vision-pager">
        <label class="pager-size">每页<select v-model="pageSize"><option v-for="s in PAGE_SIZES" :key="s" :value="s">{{ s }}</option></select>条</label>
        <div class="pager-nav">
          <button class="btn-mini" :disabled="currentPage <= 1" @click="changeVisionPage(currentPage - 1)">上一页</button>
          <span class="pager-info">第 {{ currentPage }} / {{ visionPageCount }} 页</span>
          <button class="btn-mini" :disabled="currentPage >= visionPageCount" @click="changeVisionPage(currentPage + 1)">下一页</button>
        </div>
      </div>
      <p class="scan-count">共 {{ visionResult.length }} 张图片（识别服务常驻，重复识别更快）</p>
    </div>

    <!-- 人物管理 -->
    <PersonPanel :persons="persons" @refresh="loadPersons" />

    <!-- 内容搜索 + 过滤 -->
    <ContentSearch :album-id="props.albumId" />
  </section>
</template>

<style scoped>
/* ---- 通用扫描区 ---- */
.scan-area {
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  margin-bottom: 20px;
  background: #fff;
}

.scan-toolbar {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 14px 10px;
  flex-wrap: wrap;
}

.scan-title {
  font-size: 16px;
  font-weight: 600;
  margin: 0;
  color: #1f2328;
}

.scan-sub {
  font-size: 12px;
  color: #667085;
  margin: 4px 0 0 0;
  max-width: 640px;
  line-height: 1.5;
}

.scan-tool-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.scan-error {
  padding: 8px 14px;
  color: #e5484d;
  font-size: 13px;
  background: #fef2f2;
  margin: 0;
}

.scan-ok {
  padding: 8px 14px;
  color: #15803d;
  font-size: 13px;
  background: #f0fdf4;
  margin: 0;
}

.scan-empty {
  padding: 24px 14px;
  text-align: center;
  color: #667085;
  font-size: 13px;
}

.scan-loading {
  animation: pulse 1.5s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 0.5; }
  50% { opacity: 1; }
}

.scan-table-wrap {
  padding: 0 14px 14px;
}

.scan-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  margin-top: 8px;
}

.scan-table th {
  text-align: left;
  padding: 6px 8px;
  background: #f8f9fa;
  border-bottom: 2px solid #e5e7eb;
  font-weight: 600;
  color: #333;
  white-space: nowrap;
}

.scan-table td {
  padding: 5px 8px;
  border-bottom: 1px solid #f0f0f0;
  vertical-align: middle;
}

.scan-table tr:hover td {
  background: #fafbfc;
}

.col-idx {
  width: 32px;
  text-align: center;
  color: #888;
}

.col-name {
  max-width: 200px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.scan-count {
  font-size: 12px;
  color: #667085;
  margin: 8px 0 0;
  text-align: right;
}

/* 图片名可点击 */
.img-name-link {
  color: #396cd8;
  cursor: pointer;
  text-decoration: underline dotted;
}

.img-name-link:hover {
  color: #2f5cc2;
}

/* 批次选择 */
.batch-select {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: #667085;
}

.batch-select select {
  padding: 2px 4px;
  border: 1px solid #d0d5dd;
  border-radius: 4px;
  font-size: 12px;
  background: #fff;
}

/* 按钮（通用） */
.btn {
  padding: 8px 16px;
  border-radius: 8px;
  border: 1px solid #ddd;
  background: #fff;
  cursor: pointer;
  font-size: 14px;
  transition: all 0.2s;
}
.btn:hover { border-color: #396cd8; color: #396cd8; }
.btn:disabled { opacity: 0.6; cursor: not-allowed; }
.btn-primary { background: #396cd8; color: #fff; border-color: #396cd8; }
.btn-primary:hover { background: #2f5cc2; color: #fff; }
.btn-ghost { background: transparent; color: #396cd8; border-color: #396cd8; }
.btn-ghost:hover { background: rgba(57, 108, 216, 0.08); color: #2f5cc2; }
.btn-mini { padding: 2px 6px; font-size: 11px; border: 1px solid #d0d5dd; border-radius: 3px; background: #fff; cursor: pointer; }
.btn-mini:disabled { opacity: 0.5; cursor: not-allowed; }

/* ---- EXIF 地点 ---- */
.gps-link { color: #396cd8; text-decoration: none; border-bottom: 1px dashed #396cd8; }
.gps-link:hover { color: #2f5cc2; }
.gps-alt { margin-left: 6px; color: #888; font-size: 12px; }
.col-place { max-width: 220px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

/* ---- 影调分析 ---- */
.tone-table th:nth-child(4), .tone-table td:nth-child(4) { width: 72px; }
.tone-table th:nth-child(5), .tone-table td:nth-child(5) { width: 260px; }
.col-luma { font-variant-numeric: tabular-nums; }
.col-hist { padding: 2px 8px !important; }
.mini-hist { display: block; width: 256px; height: 36px; }
.hist-empty { color: #bbb; }
.tone-badge { display: inline-block; padding: 1px 6px; border-radius: 3px; font-size: 11px; }
.tone-badge.tone-low-key, .tone-badge.tone-low_key { background: #2a2a2a; color: #fff; }
.tone-badge.tone-mid-key, .tone-badge.tone-mid_key { background: #667085; color: #fff; }
.tone-badge.tone-high-key, .tone-badge.tone-high_key { background: #e5e7eb; color: #1f2328; }

/* ---- 内容识别 ---- */
.vision-progress { padding: 8px 14px; }
.progress-track { height: 6px; background: #e5e7eb; border-radius: 3px; overflow: hidden; }
.progress-fill { height: 100%; background: #396cd8; transition: width 0.3s; border-radius: 3px; }
.progress-text { font-size: 11px; color: #667085; margin: 4px 0 0; }
.vision-cat { font-weight: 600; color: #1f2328; }
.vision-sub { margin-left: 4px; font-size: 11px; color: #667085; }
.vision-err { color: #e5484d; font-size: 11px; }
.col-label { max-width: 160px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ppl-note { margin-left: 4px; font-size: 10px; color: #667085; }
.conf-bar { display: inline-block; width: 50px; height: 6px; background: #e5e7eb; border-radius: 3px; overflow: hidden; vertical-align: middle; }
.conf-fill { display: block; height: 100%; background: #22c55e; border-radius: 3px; }
.conf-val { margin-left: 4px; font-size: 11px; color: #667085; }
.col-top3 { max-width: 200px; }
.top3-chip { display: inline-block; padding: 1px 6px; border-radius: 3px; background: #eef3fb; color: #396cd8; font-size: 11px; margin-right: 2px; }
.person-chip { display: inline-block; padding: 1px 6px; border-radius: 3px; background: #fef3c7; color: #9a6b00; font-size: 11px; margin-right: 2px; }
.gpu-status { padding: 4px 14px; font-size: 12px; color: #667085; }
.gpu-status.gpu-on { color: #15803d; }

/* 分页 */
.vision-pager { display: flex; align-items: center; justify-content: flex-end; gap: 12px; margin-top: 8px; flex-wrap: wrap; }
.pager-size { display: flex; align-items: center; gap: 4px; font-size: 12px; color: #667085; }
.pager-size select { padding: 2px 4px; border: 1px solid #d0d5dd; border-radius: 4px; font-size: 12px; }
.pager-nav { display: flex; align-items: center; gap: 6px; }
.pager-info { font-size: 12px; color: #667085; }

/* ---- 组合扫描 ---- */
.combo-area { margin-bottom: 24px; }
.combo-controls { display: flex; flex-wrap: wrap; gap: 12px; padding: 12px 14px; background: #f8f9fa; border-radius: 6px; margin-bottom: 12px; }
.combo-checks { display: flex; gap: 10px; flex-wrap: wrap; flex: 1; min-width: 0; }
.combo-check { display: inline-flex; align-items: center; gap: 6px; padding: 6px 10px; border: 1px solid #d0d5dd; border-radius: 6px; cursor: pointer; transition: all 0.15s; }
.combo-check:hover { border-color: #396cd8; background: #eef3fb; }
.combo-check.active { border-color: #396cd8; background: #eef3fb; }
.combo-check input[type="checkbox"] { margin-right: 2px; }
.combo-check-label { font-weight: 500; font-size: 13px; }
.combo-check-desc { font-size: 11px; color: #667085; }
.combo-meta { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.gpu-info { font-size: 12px; color: #667085; }
.gpu-info.ok { color: #15803d; }
.combo-table th:nth-child(7), .combo-table td:nth-child(7) { width: 80px; }
.combo-table th:nth-child(8), .combo-table td:nth-child(8) { width: 120px; }
.combo-table th:nth-child(9), .combo-table td:nth-child(9) { width: 180px; }

/* 扫描工具区 */
.scan-tool-actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
</style>