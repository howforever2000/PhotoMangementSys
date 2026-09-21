<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, reactive } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";
import PhotoLightbox from "../components/PhotoLightbox.vue";
import type { SmartHit } from "../types/content";
import type { PersonInfo } from "../types/photo";

/**
 * 智能搜索（FEAT-034）—— 半自然语言 + 多维筛选
 *
 * 关键词宽匹配（内容/地点/标签/相册名/文件名）→ 组合结构化筛选：
 * 时间区间 / 地点 / 类别 / 标签 / 人物标号 / 影调。
 * 「智能解析」按钮会把自然语言拆成结构化筛选自动填充。
 * 结果网格复用 get_photo_thumbs（按 album_id 分组）生成缩略图。
 *
 * FEAT-067 新增两条通道：
 *   - 🖼 以图搜图：选一张照片 → 图像塔编码 → 与已索引向量比余弦（复用现有索引）
 *   - 描述语义：后端把「描述向量」作为第二路语义接进 RRF，前端无感
 *   人物一律走 faces.person_id 精确过滤（人物编号不进向量），下拉里已命名的
 *   人物显示真名，未命名显示编号。
 *
 * 点击行为（与人像画廊 / 时间线一致）：
 *   - 点图片 → PhotoLightbox 大图预览（序列 = 当前全部搜索结果，可左右翻页）
 *   - 卡片右上角「📁 相册名」→ 跳转该照片所属相册（次要入口，点击不触发预览）
 */
const router = useRouter();
const theme = useThemeStore();
const notify = useNotify();

const keyword = ref("");
const searching = ref(false);
const searched = ref(false);
const error = ref("");
const results = ref<SmartHit[]>([]);
const thumbMap = ref<Record<string, string>>({});

/** FEAT-067：以图搜图 —— 当前查询图（空 = 关键词模式） */
const queryImagePath = ref("");
/** 人物注册表（精确过滤下拉：已命名显示真名，未命名显示编号） */
const persons = ref<PersonInfo[]>([]);

function personLabel(p: PersonInfo): string {
  return p.name && p.name !== p.id ? `${p.name}（${p.id}）` : p.id;
}

/** 结构化筛选 */
const filters = reactive({
  dateFrom: "",
  dateTo: "",
  location: "",
  category: "",
  label: "",
  person: "",
  toneType: "",
});

/** 常见大类（供类别 datalist / 智能解析映射） */
const categories = ["portrait", "street", "animal", "landscape_nature", "architecture", "food", "object", "text"];
const tones = [
  { value: "", label: "影调不限" },
  { value: "high-key", label: "高调（明亮）" },
  { value: "mid-key", label: "中间调" },
  { value: "low-key", label: "低调（暗调）" },
];

function fileUrl(p: string): string {
  return p ? convertFileSrc(p) : "";
}

/**
 * FEAT-SEM：语义链路状态（**分档**原因 + 重试入口）
 *
 * 预热的价值：重启应用后 VCR 服务未运行、CLIP 会话未加载，若不预热则首次搜索
 * 要么多等数秒、要么静默降级成纯关键词。
 *
 * 为什么要分档（BUG-2026-0920-006/007）：旧实现只拿到
 * `warmup_semantic_service -> bool`，任何 false 都渲染成同一句「CLIP 模型未下载」；
 * 而真实原因可能是退避中、服务没起来、或索引与当前模型档位不匹配——用户照提示去
 * 「下载模型」，当然解决不了问题。现在改读 `semantic_status`，按 code 分档：
 *   model_missing        模型文件确实没下载        → 引导去「⚙ 性能设置」下载
 *   service_unreachable  服务没起来 / 模型加载中   → 直接搜索即可触发启动
 *   backoff              刚失败过一次的短暂退避    → 倒计时后自动重探
 *   index_model_mismatch 向量在库里但档位不匹配    → 引导去扫描中心重建索引
 *
 * ⚠ 关键约定：本状态**不参与**结果区的条件链。语义不可用只影响召回质量，
 * 绝不该影响结果的展示——此前两者耦合在同一条 v-if 链上，后端明明返回了
 * 417 条结果，页面上却一条都渲染不出来（BUG-2026-0920-007）。
 */
interface SemanticStatus {
  ready: boolean;
  code: string;
  message: string;
  backoff_remaining_ms: number;
}

const semStatus = ref<SemanticStatus | null>(null);
const semRetrying = ref(false);
/** 退避到期后自动重探的定时器（离开页面必须清掉，否则切页后仍会触发） */
let semRetryTimer: ReturnType<typeof setTimeout> | null = null;

function clearSemTimer() {
  if (semRetryTimer !== null) {
    clearTimeout(semRetryTimer);
    semRetryTimer = null;
  }
}

/** 只读探测语义状态；若正处于退避，则在到期后自动重探一次 */
async function refreshSemanticStatus() {
  clearSemTimer();
  try {
    const s = await invoke<SemanticStatus>("semantic_status");
    semStatus.value = s;
    // 退避是**有时限**的：到期自动重探，服务恢复后无需用户任何操作即回归
    if (!s.ready && s.code === "backoff" && s.backoff_remaining_ms > 0) {
      semRetryTimer = setTimeout(() => {
        semRetryTimer = null;
        refreshSemanticStatus();
      }, s.backoff_remaining_ms + 500);
    }
  } catch {
    // 探测本身失败：不覆盖已有状态，也不阻塞搜索
  }
}

/** 手动重试：先真正拉起服务（warmup），再刷新状态 */
async function retrySemantic() {
  if (semRetrying.value) return;
  semRetrying.value = true;
  try {
    await invoke<boolean>("warmup_semantic_service");
  } catch {
    /* 失败原因交给随后的状态探测说明 */
  } finally {
    await refreshSemanticStatus();
    semRetrying.value = false;
  }
}

onMounted(() => {
  // 预热：真正让服务与 CLIP 会话起来（fire-and-forget，不阻塞首屏）
  invoke<boolean>("warmup_semantic_service").catch(() => {});
  // 状态：只读探测，用于给出分档提示
  refreshSemanticStatus();
  // 人物下拉（精确过滤用）；失败不阻塞搜索
  invoke<PersonInfo[]>("list_persons")
    .then((list) => {
      persons.value = list;
    })
    .catch(() => {
      persons.value = [];
    });
});

onUnmounted(clearSemTimer);

/**
 * FEAT-067：以图搜图（选一张照片找相似）
 *
 * 查询图会先复用其缩略图做编码（与建索引时同一输入），保证「拿已索引的图查自己」
 * 余弦 = 1.0；结果复用同一套结果网格与缩略图加载。
 */
async function runImageSearch() {
  const picked = await openFileDialog({
    multiple: false,
    directory: false,
    title: "选择一张照片作为搜索样例",
    filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp", "gif"] }],
  });
  if (typeof picked !== "string") return;
  searching.value = true;
  error.value = "";
  thumbMap.value = {};
  try {
    results.value = await invoke<SmartHit[]>("smart_search_by_image", {
      path: picked,
      limit: 60,
      minSimilarity: null,
    });
    queryImagePath.value = picked;
    searched.value = true;
    await loadThumbs();
  } catch (e) {
    error.value = String(e);
    notify.error("以图搜图失败", String(e));
  } finally {
    searching.value = false;
  }
}

async function runSearch() {
  searching.value = true;
  error.value = "";
  thumbMap.value = {};
  try {
    results.value = await invoke<SmartHit[]>("smart_search", {
      keyword: keyword.value,
      dateFrom: filters.dateFrom || null,
      dateTo: filters.dateTo || null,
      location: filters.location || null,
      category: filters.category || null,
      label: filters.label || null,
      personId: filters.person || null,
      toneType: filters.toneType || null,
    });
    queryImagePath.value = ""; // 回到关键词模式
    searched.value = true;
    await loadThumbs();
  } catch (e) {
    error.value = String(e);
    notify.error("智能搜索失败", String(e));
  } finally {
    searching.value = false;
  }
}

async function loadThumbs() {
  const byAlbum = new Map<number, string[]>();
  for (const r of results.value) {
    const aid = r.album_id ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    if (!thumbMap.value[r.path]) byAlbum.get(aid)!.push(r.path);
  }
  await Promise.all(
    [...byAlbum.keys()].map(async (aid) => {
      const paths = byAlbum.get(aid)!;
      if (!paths.length) return;
      try {
        const pairs = await invoke<[string, string][]>("get_photo_thumbs", { albumId: aid, paths });
        for (const [path, thumb] of pairs) if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
      } catch {
        /* 单相册失败不阻塞 */
      }
    }),
  );
}

function resetFilters() {
  keyword.value = "";
  filters.dateFrom = "";
  filters.dateTo = "";
  filters.location = "";
  filters.category = "";
  filters.label = "";
  filters.person = "";
  filters.toneType = "";
  results.value = [];
  searched.value = false;
  thumbMap.value = {};
  queryImagePath.value = "";
}

/** 智能解析自然语言 → 填充结构化筛选 + 关键词 */
function parseNatural() {
  const q = keyword.value.trim();
  if (!q) {
    notify.warning("请先输入搜索词");
    return;
  }
  const now = new Date();
  const thisYear = now.getFullYear();
  let kwRemain = q;
  let dateFrom = "";
  let dateTo = "";
  let category = "";
  let tone = "";

  // 1) 年份
  const yearM = q.match(/(20\d{2})/);
  if (yearM) {
    const y = yearM[1];
    dateFrom = `${y}-01-01`;
    dateTo = `${y}-12-31`;
    kwRemain = kwRemain.replace(yearM[0], " ");
  }
  if (/去年/.test(q)) {
    const y = thisYear - 1;
    dateFrom = `${y}-01-01`;
    dateTo = `${y}-12-31`;
    kwRemain = kwRemain.replace(/去年/g, " ");
  }
  if (/今年/.test(q)) {
    dateFrom = `${thisYear}-01-01`;
    dateTo = `${thisYear}-12-31`;
    kwRemain = kwRemain.replace(/今年/g, " ");
  }
  // 2) 季节
  const seasonMap: Record<string, [string, string]> = {
    春: ["03-01", "05-31"],
    夏: ["06-01", "08-31"],
    秋: ["09-01", "11-30"],
    冬: ["12-01", "12-31"],
  };
  for (const key of Object.keys(seasonMap)) {
    if (new RegExp(`${key}(天|季|季)`).test(q) || q.includes(`${key}年`)) {
      const [m1, m2] = seasonMap[key];
      const yr = yearM ? yearM[1] : thisYear;
      dateFrom = `${yr}-${m1}`;
      dateTo = `${yr}-${m2}`;
      kwRemain = kwRemain.replace(new RegExp(`${key}(天|季)?`, "g"), " ");
      break;
    }
  }
  // 3) 影调
  if (/高调|明亮|亮调/.test(q)) tone = "high-key";
  else if (/低调|暗调|较暗|暗/.test(q)) tone = "low-key";
  else if (/中间调/.test(q)) tone = "mid-key";
  if (tone) kwRemain = kwRemain.replace(/高调|明亮|亮调|低调|暗调|较暗|中间调/g, " ");

  // 4) 大类
  const catMap: Record<string, string> = {
    人像: "portrait", 肖像: "portrait",
    街: "street", 街道: "street", 街拍: "street",
    动物: "animal", 宠物: "animal", 猫: "animal", 狗: "animal",
    风景: "landscape_nature", 自然: "landscape_nature", 风光: "landscape_nature",
    建筑: "architecture",
    食物: "food", 美食: "food",
    文字: "text", 文档: "text",
  };
  for (const key of Object.keys(catMap)) {
    if (q.includes(key)) {
      category = catMap[key];
      kwRemain = kwRemain.replace(new RegExp(key, "g"), " ");
      break;
    }
  }
  // 5) 地点锚点：在/于 X（提取到下一个空格/结束）
  const locM = kwRemain.match(/(?:在|于|@)\s*([\u4e00-\u9fa5A-Za-z0-9·]+)/);
  if (locM) {
    filters.location = locM[1];
    kwRemain = kwRemain.replace(locM[0], " ");
  }

  // 回填
  filters.dateFrom = dateFrom || filters.dateFrom;
  filters.dateTo = dateTo || filters.dateTo;
  filters.category = category || filters.category;
  filters.toneType = tone || filters.toneType;
  keyword.value = kwRemain.replace(/\s+/g, " ").trim();

  runSearch();
}

/**
 * 跳转该照片所属相册（卡片小按钮 / 大图工具栏共用）
 *
 * 带 `?focus=<path>`：相册详情页读 route.query.focus → PhotoGrid 滚动定位 + 高亮，
 * 与时间线「↗ 去相册」保持同一约定（FEAT-034-B），否则用户跳过去还得自己找。
 */
function goAlbum(albumId: number | null | undefined, path?: string) {
  if (albumId == null) return;
  lightboxOpen.value = false;
  router.push({
    path: `/album/${albumId}`,
    query: path ? { focus: path } : undefined,
  });
}

/** 大图工具栏「📁 在相册中查看」：只拿到 albumId，路径取当前浏览到的那张 */
function goAlbumFromLightbox(albumId: number) {
  goAlbum(albumId, lightboxPhotos.value[lightboxIndex.value]?.path);
}

/* ---- 大图预览（复用 PhotoLightbox：与人像/时间线同一范式）---- */
const lightboxOpen = ref(false);
const lightboxIndex = ref(0);

/** 浏览序列 = 当前全部搜索结果（含语义命中），只渲染当前张 */
const lightboxPhotos = computed(() =>
  results.value.map((r) => ({ path: r.path, albumId: r.album_id })),
);

function openPhoto(r: SmartHit) {
  const idx = results.value.findIndex((x) => x.path === r.path);
  if (idx < 0) return;
  lightboxIndex.value = idx;
  lightboxOpen.value = true;
}

const toneLabel = (t: string | null) =>
  t === "high-key" ? "高调" : t === "mid-key" ? "中间调" : t === "low-key" ? "低调" : "";

function showTag(r: SmartHit): string {
  if (r.label) return r.label;
  if (r.category) return r.category;
  return "照片";
}

// 进入页面不再自动拉空结果：让用户主动输入词后再搜索，避免首屏全空造成的「好像坏掉」错觉
</script>

<template>
  <div class="ss-page" :style="{ color: theme.textColor }">
    <header class="ss-header">
      <button class="btn" @click="router.push('/home')">← 返回主页</button>
      <h1 class="ss-title">🔎 智能搜索</h1>
      <p class="ss-subtitle">自然语言 + 多维筛选，跨相册检索照片（AI 扫描入库）</p>
    </header>

    <!-- 搜索栏 -->
    <div class="ss-searchbar">
      <input
        v-model="keyword"
        class="ss-input"
        type="text"
        placeholder='试试「海边日落」/「一只猫」/「2023年的聚会」/「成都 人像」'
        @keyup.enter="runSearch"
      />
      <button class="btn ss-btn-smart" title="解析自然语言并搜索" @click="parseNatural">✨ 智能解析</button>
      <button class="btn ss-btn-go" :disabled="searching" @click="runSearch">
        {{ searching ? "搜索中…" : "搜索" }}
      </button>
      <!-- FEAT-067：以图搜图（复用现有图像向量索引，不需要重建） -->
      <button class="btn ss-btn-img" :disabled="searching" title="选一张照片，找相似照片" @click="runImageSearch">
        🖼 以图搜图
      </button>
    </div>

    <!-- 以图搜图：查询图回显 + 退出 -->
    <p v-if="queryImagePath" class="ss-imgq">
      🖼 以图搜图：{{ queryImagePath.split(/[\\/]/).pop() }}
      <button class="btn ss-btn-x" title="退出以图搜图" @click="queryImagePath = ''; results = []; searched = false">✕</button>
    </p>

    <!-- 结构化筛选芯片 -->
    <div class="ss-filters">
      <div class="f-item">
        <label>日期</label>
        <input v-model="filters.dateFrom" type="date" class="f-date" />
        <span>至</span>
        <input v-model="filters.dateTo" type="date" class="f-date" />
      </div>
      <div class="f-item">
        <label>地点</label>
        <input v-model="filters.location" type="text" class="f-text" placeholder="如 成都" />
      </div>
      <div class="f-item">
        <label>类别</label>
        <input v-model="filters.category" type="text" class="f-text" list="cat-list" placeholder="portrait / street…" />
        <datalist id="cat-list">
          <option v-for="c in categories" :key="c" :value="c" />
        </datalist>
      </div>
      <div class="f-item">
        <label>标签</label>
        <input v-model="filters.label" type="text" class="f-text" placeholder="如 猫 / golden retriever" />
      </div>
      <div class="f-item">
        <!-- FEAT-067：人物走 faces.person_id 精确过滤（不进向量，靠人脸识别） -->
        <label>人物</label>
        <select v-model="filters.person" class="f-select">
          <option value="">人物不限</option>
          <option v-for="p in persons" :key="p.id" :value="p.id">{{ personLabel(p) }}</option>
        </select>
      </div>
      <div class="f-item">
        <label>影调</label>
        <select v-model="filters.toneType" class="f-select">
          <option v-for="t in tones" :key="t.value" :value="t.value">{{ t.label }}</option>
        </select>
      </div>
      <button class="btn f-reset" @click="resetFilters">清空</button>
    </div>

    <!-- 语义不可用提示（独立区块：与下方结果区的条件链完全解耦，绝不隐藏结果） -->
    <p v-if="semStatus && !semStatus.ready" class="ss-warn">
      ⚠ {{ semStatus.message }}
      <span v-if="semStatus.code === 'model_missing'">
        前往
        <router-link to="/scan" class="ss-link">扫描中心 → ⚙ 性能设置</router-link>
      </span>
      <span v-else-if="semStatus.code === 'index_model_mismatch'">
        前往<router-link to="/scan" class="ss-link">扫描中心</router-link>重建
      </span>
      <button class="ss-retry" :disabled="semRetrying" @click="retrySemantic">
        {{ semRetrying ? "重试中…" : "重试" }}
      </button>
    </p>

    <!-- 加载 -->
    <div v-if="searching" class="ss-loading">正在搜索…</div>

    <!-- 错误 -->
    <div v-else-if="error" class="ss-empty">
      <div class="ss-empty-icon">⚠️</div>
      <p class="ss-empty-text">搜索失败：{{ error }}</p>
    </div>

    <!-- 未开始搜索：引导输入 -->
    <div v-else-if="!searched" class="ss-empty">
      <div class="ss-empty-icon">🔍</div>
      <p class="ss-empty-title">输入关键词开始检索</p>
      <p class="ss-empty-text">试试「<b>2023年的猫</b>」/「<b>成都 人像</b>」/「<b>去年春天</b>」/「<b>暗调</b>」；也可以使用下方的结构化筛选。</p>
    </div>

    <!-- 空结果 -->
    <div v-else-if="results.length === 0" class="ss-empty">
      <div class="ss-empty-icon">🔍</div>
      <p class="ss-empty-title">没有找到匹配的照片</p>
      <p class="ss-empty-text">试试放宽条件，或先在相册中执行「组合扫描」让照片具备 AI 内容与拍摄时间。</p>
      <p class="ss-empty-text">
        想用「海边日落」「一只猫」这类自然语言搜图？去
        <router-link to="/scan" class="ss-link">扫描中心</router-link>
        勾选「语义向量」构建索引（需在模型设置中下载 CLIP 模型）。
      </p>
    </div>

    <!-- 结果 -->
    <div v-else class="ss-results">
      <p class="ss-count">
        找到 {{ results.length }} 张照片{{ queryImagePath ? "（以图搜图，按相似度排序）" : "" }}
      </p>
      <div class="ss-grid">
        <figure
          v-for="r in results"
          :key="r.id"
          class="ss-card"
          :title="[r.label, r.location, r.album_name].filter(Boolean).join(' · ') + '（点击预览）'"
          @click="openPhoto(r)"
        >
          <img v-if="thumbMap[r.path]" :src="fileUrl(thumbMap[r.path])" loading="lazy" class="ss-thumb" alt="" />
          <div v-else class="ss-thumb ss-thumb-ph">🖼️</div>
          <!-- 相册跳转（次要入口：点它才跳，点图片是预览） -->
          <button
            v-if="r.album_id != null"
            class="ss-album"
            :title="`在相册中查看：${r.album_name ?? '未命名相册'}`"
            @click.stop="goAlbum(r.album_id, r.path)"
          >📁 {{ r.album_name ?? "相册" }}</button>
          <figcaption class="ss-cap">
            <span class="ss-tag">{{ showTag(r) }}</span>
            <span v-if="r.semantic_score != null" class="ss-sem" title="语义相似度（CLIP 向量余弦）">✨ AI 匹配 {{ Math.round(r.semantic_score * 100) }}%</span>
            <span v-if="r.location" class="ss-loc">📍 {{ r.location }}</span>
            <span v-if="toneLabel(r.tone_type)" class="ss-tone">{{ toneLabel(r.tone_type) }}</span>
          </figcaption>
        </figure>
      </div>
    </div>

    <!-- 大图预览（与人像/时间线同一组件；支持 ←/→ 翻页、Esc 关闭、打星、标签、去相册） -->
    <PhotoLightbox
      v-if="lightboxOpen"
      :photos="lightboxPhotos"
      :index="lightboxIndex"
      @close="lightboxOpen = false"
      @open-album="goAlbumFromLightbox"
    />
  </div>
</template>

<style scoped>
/* 卡片右上角「📁 相册名」：次要入口，不会被点击预览误触 */
.ss-album {
  position: absolute;
  right: 6px;
  top: 6px;
  max-width: calc(100% - 12px);
  padding: 2px 8px;
  font-size: 10.5px;
  line-height: 1.6;
  color: #fff;
  background: rgba(0, 0, 0, 0.55);
  border: none;
  border-radius: 999px;
  cursor: pointer;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  backdrop-filter: blur(3px);
}
.ss-album:hover {
  background: rgba(57, 108, 216, 0.9);
}
.ss-warn {
  margin: 0 0 12px;
  padding: 9px 12px;
  border-radius: 10px;
  font-size: 12.5px;
  line-height: 1.7;
  background: rgba(180, 83, 9, 0.1);
  border: 1px solid rgba(180, 83, 9, 0.3);
  color: #b45309;
}
/* 提示条内的「重试」：语义通道失败后的一键恢复入口 */
.ss-retry {
  margin-left: 8px;
  padding: 1px 10px;
  font-size: 12px;
  line-height: 1.6;
  color: #b45309;
  background: rgba(180, 83, 9, 0.08);
  border: 1px solid rgba(180, 83, 9, 0.45);
  border-radius: 999px;
  cursor: pointer;
  transition: background 0.15s, opacity 0.15s;
}
.ss-retry:hover:not(:disabled) {
  background: rgba(180, 83, 9, 0.2);
}
.ss-retry:disabled {
  opacity: 0.55;
  cursor: default;
}
.ss-page {
  padding: 20px;
  max-width: 1200px;
  margin: 0 auto;
  min-height: 100vh;
  box-sizing: border-box;
}
.ss-header {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
  margin-bottom: 16px;
}
.ss-title {
  font-size: 22px;
  margin: 0;
  font-weight: 700;
  /* 页面背景上的标题：on-bg 对比色（BUG-2026-0919-004） */
  color: var(--color-on-bg, inherit);
}
.ss-subtitle {
  margin: 0;
  opacity: 0.7;
  font-size: 13px;
  color: var(--color-on-bg-2, inherit);
}
.ss-searchbar {
  display: flex;
  gap: 8px;
  margin-bottom: 14px;
}
.ss-input {
  flex: 1;
  padding: 10px 12px;
  border: 1px solid var(--border, rgba(127, 127, 127, 0.3));
  background: var(--input-bg, rgba(127, 127, 127, 0.06));
  color: inherit;
  border-radius: 8px;
  font-size: 15px;
}
.ss-btn-smart {
  white-space: nowrap;
}
/* FEAT-067：以图搜图按钮（与「智能解析」同排，绿色区分） */
.ss-btn-img {
  white-space: nowrap;
  background: rgba(22, 163, 74, 0.12);
  border-color: rgba(22, 163, 74, 0.45);
}
.ss-imgq {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 0 12px;
  padding: 8px 12px;
  border-radius: 10px;
  font-size: 12.5px;
  background: rgba(22, 163, 74, 0.1);
  border: 1px solid rgba(22, 163, 74, 0.3);
  word-break: break-all;
}
.ss-btn-x {
  padding: 2px 8px;
  font-size: 11px;
  line-height: 1.6;
}
.ss-btn-go {
  white-space: nowrap;
}
.ss-filters {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  align-items: center;
  background: var(--card-bg, rgba(127, 127, 127, 0.05));
  padding: 12px;
  border-radius: 12px;
  margin-bottom: 18px;
}
.f-item {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
}
.f-item label {
  opacity: 0.8;
}
.f-date,
.f-text,
.f-select {
  padding: 6px 8px;
  border: 1px solid var(--border, rgba(127, 127, 127, 0.3));
  background: var(--input-bg, rgba(127, 127, 127, 0.06));
  color: inherit;
  border-radius: 6px;
  font-size: 13px;
}
.f-date {
  width: 130px;
}
.f-text {
  width: 130px;
}
.f-select {
  width: 130px;
}
.f-reset {
  margin-left: auto;
}
.ss-loading {
  text-align: center;
  opacity: 0.7;
  padding: 30px;
}
.ss-empty {
  text-align: center;
  padding: 70px 20px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}
.ss-empty-icon {
  font-size: 50px;
}
.ss-empty-title {
  font-size: 18px;
  font-weight: 600;
  margin: 0;
}
.ss-empty-text {
  opacity: 0.7;
  max-width: 460px;
  line-height: 1.6;
  margin: 0;
}
.ss-count {
  opacity: 0.8;
  font-size: 14px;
  margin: 0 0 12px;
}
.ss-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 10px;
}
.ss-card {
  margin: 0;
  position: relative;
  aspect-ratio: 1 / 1;
  border-radius: 10px;
  overflow: hidden;
  cursor: pointer;
  background: rgba(127, 127, 127, 0.1);
  transition: transform 0.12s;
}
.ss-card:hover {
  transform: translateY(-2px);
}
.ss-thumb {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.ss-thumb-ph {
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 30px;
  opacity: 0.6;
}
.ss-cap {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 6px 8px;
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  font-size: 11px;
  color: #fff;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.65));
}
.ss-tag {
  font-weight: 600;
}
/* FEAT-SEM：语义命中徽标（与 ss-tag 同级，淡金色调区分） */
.ss-sem {
  font-weight: 600;
  color: #ffd76a;
}
.ss-link {
  color: #6aa6ff;
  text-decoration: underline;
}
.ss-loc,
.ss-tone {
  opacity: 0.95;
}
@media (max-width: 640px) {
  .ss-page {
    padding: 12px;
  }
  .ss-searchbar {
    flex-wrap: wrap;
  }
  .ss-input {
    flex-basis: 100%;
  }
  .ss-grid {
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
  }
}
</style>
