<script setup lang="ts">
import { computed, onMounted, ref, reactive } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useRouter } from "vue-router";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";
import type { SmartHit } from "../types/content";

/**
 * 智能搜索（FEAT-034）—— 半自然语言 + 多维筛选
 *
 * 关键词宽匹配（内容/地点/标签/相册名/文件名）→ 组合结构化筛选：
 * 时间区间 / 地点 / 类别 / 标签 / 人物标号 / 影调。
 * 「智能解析」按钮会把自然语言拆成结构化筛选自动填充。
 * 结果网格复用 get_photo_thumbs（按 album_id 分组）生成缩略图。
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

function openAlbum(r: SmartHit) {
  if (r.album_id != null) router.push(`/album/${r.album_id}`);
}

const hasResults = computed(() => searched.value && !error.value);
const toneLabel = (t: string | null) =>
  t === "high-key" ? "高调" : t === "mid-key" ? "中间调" : t === "low-key" ? "低调" : "";

function showTag(r: SmartHit): string {
  if (r.label) return r.label;
  if (r.category) return r.category;
  return "照片";
}

onMounted(() => {
  runSearch();
});
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
        placeholder='试试「2023年的猫」/「成都 人像」/「去年春天」/「暗调」'
        @keyup.enter="runSearch"
      />
      <button class="btn ss-btn-smart" title="解析自然语言并搜索" @click="parseNatural">✨ 智能解析</button>
      <button class="btn ss-btn-go" :disabled="searching" @click="runSearch">
        {{ searching ? "搜索中…" : "搜索" }}
      </button>
    </div>

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
        <label>人物</label>
        <input v-model="filters.person" type="text" class="f-text" placeholder="如 P001" />
      </div>
      <div class="f-item">
        <label>影调</label>
        <select v-model="filters.toneType" class="f-select">
          <option v-for="t in tones" :key="t.value" :value="t.value">{{ t.label }}</option>
        </select>
      </div>
      <button class="btn f-reset" @click="resetFilters">清空</button>
    </div>

    <!-- 加载 -->
    <div v-if="searching" class="ss-loading">正在搜索…</div>

    <!-- 错误 -->
    <div v-else-if="error" class="ss-empty">
      <div class="ss-empty-icon">⚠️</div>
      <p class="ss-empty-text">搜索失败：{{ error }}</p>
    </div>

    <!-- 空结果 -->
    <div v-else-if="searched && results.length === 0" class="ss-empty">
      <div class="ss-empty-icon">🔍</div>
      <p class="ss-empty-title">没有找到匹配的照片</p>
      <p class="ss-empty-text">试试放宽条件，或先在相册中执行「组合扫描」让照片具备 AI 内容与拍摄时间。</p>
    </div>

    <!-- 结果 -->
    <div v-else-if="hasResults" class="ss-results">
      <p class="ss-count">找到 {{ results.length }} 张照片</p>
      <div class="ss-grid">
        <figure
          v-for="r in results"
          :key="r.id"
          class="ss-card"
          :title="[r.label, r.location, r.album_name].filter(Boolean).join(' · ')"
          @click="openAlbum(r)"
        >
          <img v-if="thumbMap[r.path]" :src="fileUrl(thumbMap[r.path])" loading="lazy" class="ss-thumb" alt="" />
          <div v-else class="ss-thumb ss-thumb-ph">🖼️</div>
          <figcaption class="ss-cap">
            <span class="ss-tag">{{ showTag(r) }}</span>
            <span v-if="r.location" class="ss-loc">📍 {{ r.location }}</span>
            <span v-if="toneLabel(r.tone_type)" class="ss-tone">{{ toneLabel(r.tone_type) }}</span>
          </figcaption>
        </figure>
      </div>
    </div>
  </div>
</template>

<style scoped>
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
}
.ss-subtitle {
  margin: 0;
  opacity: 0.7;
  font-size: 13px;
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
