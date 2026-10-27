<script setup lang="ts">
/**
 * OverviewPane —— 智慧相册「全局概览」仪表墙（FEAT-110 建 / FEAT-116 重排）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * FEAT-116 改了什么，为什么（对着截图指认）
 * ──────────────────────────────────────────────────────────────────────────
 * ① **11 块玻璃板 = 11 个同重量的矩形。**
 *    层级只能靠字号硬撑，读起来像贴满便利贴的墙。现在拆成两个声部：
 *      · 裸（bare）   —— 照片库总览 / 今年月度：纯读数，文字直接压在页面上
 *      · 板（panel）  —— 相册 / 人物 / 地点 / 最近导入 / 那年今日：有内容才配板
 *    板从 11 块降到 5 块，剩下的 2 块裸读数反而把"这一屏说的是我的照片库"立住了。
 *
 * ② **同一件事被数了三遍。**
 *    旧版：GPS 覆盖 39.5% 出现在「地点」widget 的脚注 + 「地点定位覆盖」的环 + 环内文字；
 *    人物 12 位出现在页头 meta、tab 计数、人物 widget、人物覆盖环。
 *    现在每项**只出现一次**，且落在它真正被需要的那一块里（见各块注释）。
 *    删掉的块：语义索引（并入照片库总览的一行）、内容分类（并入人物/相册的脚注）、
 *    人物照片覆盖 / 地点定位覆盖（并入对应块的脚注）。信息一条没少，只是不再重复。
 *
 * ③ **金色用光了。**
 *    旧版 4 条金色进度条 + 金色环 + 金色排名条同屏。现在金色只出现在两处：
 *    「已入库」占比线（回答"库建好了吗"）与「今年月度」的当月柱（回答"这个月怎么样"）。
 *    其余一律 --w-fg2 弱色 —— 这是 tokens.css §2.7 的色彩预算第一次真正落地。
 *
 * 数据源全部为现有命令，零新增后端：
 *   list_timeline（跨相册已扫描照片，含 shoot_time/location/person_ids）
 *   list_persons + get_person_avatars_bulk
 *   list_photo_locations（GPS 聚合）
 *   list_categories + category_index_stats（语义分类与索引覆盖）
 *   album store（照片总数 / 存储 / 相册封面）
 */
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useAlbumStore } from "../stores/album";
import { useOverviewStore } from "../stores/overview";
import { getThumbs } from "../utils/thumbQueue";
import WidgetGrid from "./ui/widget/WidgetGrid.vue";
import UiWidget from "./ui/widget/UiWidget.vue";
import BarsCore from "./ui/widget/BarsCore.vue";
import CoversCore from "./ui/widget/CoversCore.vue";
import FacesCore from "./ui/widget/FacesCore.vue";
import RankBarsCore from "./ui/widget/RankBarsCore.vue";
import MemoryCore from "./ui/widget/MemoryCore.vue";

const router = useRouter();
const store = useAlbumStore();
const overview = useOverviewStore();

const loading = ref(true);
const error = ref("");

/* 数据全部来自 overview store（按「变动即失效」缓存，见 stores/overview.ts）。
   这里用 computed 读快照，切回概览时是**零 IPC** 直接渲染。 */
const timeline = computed(() => overview.snapshot?.timeline ?? []);
const persons = computed(() => overview.snapshot?.persons ?? []);
const locations = computed(() => overview.snapshot?.locations ?? []);
const categories = computed(() => overview.snapshot?.categories ?? []);
const catStats = computed(() => overview.snapshot?.catStats ?? null);

/** path → 缩略图可渲染 URL（时间线照片走缩略图管线；相册封面直转） */
const thumbMap = ref<Record<string, string>>({});
/** pid → 头像 URL */
const faceMap = ref<Record<string, string>>({});

function fmtSize(bytes: number): string {
  if (!bytes) return "0 B";
  if (bytes >= 1024 ** 3) return (bytes / 1024 ** 3).toFixed(1) + " GB";
  if (bytes >= 1024 ** 2) return (bytes / 1024 ** 2).toFixed(1) + " MB";
  if (bytes >= 1024) return (bytes / 1024).toFixed(1) + " KB";
  return bytes + " B";
}
const fileUrl = (p: string | null | undefined) => (p ? convertFileSrc(p) : "");

/* ───────────────────────── 聚合统计 ───────────────────────── */
const totalPhotos = computed(() => store.albums.reduce((n, a) => n + (a.photo_count || 0), 0));
const totalBytes = computed(() => store.albums.reduce((n, a) => n + (a.size_bytes || 0), 0));
const totalAlbums = computed(() => store.albums.length);
const scanned = computed(() => timeline.value.length);
const knownRatio = computed(() => (totalPhotos.value ? scanned.value / totalPhotos.value : 0));
const indexedRatio = computed(() =>
  catStats.value && catStats.value.known > 0 ? catStats.value.indexed / catStats.value.known : 0,
);

/** 年份跨度 */
const years = computed(() => {
  const ys = timeline.value.map((r) => Number(r.shoot_time?.slice(0, 4))).filter((y) => y > 1900);
  return ys.length ? { min: Math.min(...ys), max: Math.max(...ys) } : null;
});

/** 今年月度柱阵（10 月之前填实，未来月不画柱；高亮当月） */
const now = new Date();
const year = now.getFullYear();
const monthBars = computed(() => {
  const counts = new Array(12).fill(0);
  for (const r of timeline.value) {
    if (!r.shoot_time || Number(r.shoot_time.slice(0, 4)) !== year) continue;
    const m = Number(r.shoot_time.slice(5, 7));
    if (m >= 1 && m <= 12) counts[m - 1]++;
  }
  const max = Math.max(1, ...counts);
  return { values: counts.map((c) => c / max), active: now.getMonth(), counts };
});
/** 今年总数与月均（只统计已经过去的月份，否则"月均"会被未来月份稀释） */
const yearTotal = computed(() => monthBars.value.counts.reduce((n, c) => n + c, 0));
const yearMonthlyAvg = computed(() => {
  const passed = Math.min(12, now.getMonth() + 1);
  return passed ? Math.round(yearTotal.value / passed) : 0;
});

/** 人物照片覆盖（timeline 行里带 person_ids 的比例） */
const faceRatio = computed(() => {
  if (!scanned.value) return 0;
  const withFace = timeline.value.filter((r) => r.person_ids && r.person_ids.length > 0).length;
  return withFace / scanned.value;
});
const facePhotos = computed(() =>
  timeline.value.filter((r) => r.person_ids && r.person_ids.length > 0).length,
);

/** GPS 覆盖与 Top 城市 */
const namedLoc = computed(() => locations.value.filter((g) => g.location !== null));
const geoPhotos = computed(() => namedLoc.value.reduce((n, g) => n + g.count, 0));
/* 分母必须是"全库张数"而不是"已入库张数"：地点聚合覆盖整个库，
   而 scanned 只统计近来入库的那部分，两者不同量纲 —— 之前用 scanned 做分母，
   mock 里出现「591.4% 带 GPS」这种读不通的数。再兜一层 min(1)：
   聚合与总表来自不同查询，极端情况下比例仍可能略超 100%。 */
const gpsRatio = computed(() =>
  totalPhotos.value ? Math.min(1, geoPhotos.value / totalPhotos.value) : 0,
);
/** 地名归一化：聚合名形如「中国·北京市·朝阳区」，只留最后一段并去掉省市后缀 */
const shortPlace = (s: string) => (s.split("·").pop() ?? s).replace(/[省市]$/, "").trim();

/**
 * FEAT-120：地点 Top5。
 * 归一化的分母是**本栏最大值**（条形长度用于互相比较），而不是全库张数 ——
 * 用小分母会让五根条都接近满格，"最多的是哪个"反而看不出来。
 */
const TOP_N = 5;
const topPlaces = computed(() => {
  const rows = [...namedLoc.value].sort((a, b) => b.count - a.count).slice(0, TOP_N);
  const max = rows[0]?.count || 1;
  return rows.map((r) => ({ label: shortPlace(r.location ?? ""), value: r.count, ratio: r.count / max }));
});

/**
 * FEAT-120：年份 Top5（按张数取前五多）。
 * 取完前五后**按年份升序摆柱**：按张数从高到低排，柱阵会变成一条永远单调下降的
 * 斜坡 —— 那是「排行」，不是「这些年照片怎么分布」。升序之后形状才是可读的，
 * 再把张数最多的那根高亮出来，两个信息都在（哪几年多 + 峰值在哪一年）。
 */
const yearTop = computed(() => {
  const counts = new Map<number, number>();
  for (const r of timeline.value) {
    const y = Number(r.shoot_time?.slice(0, 4));
    if (y > 1900) counts.set(y, (counts.get(y) ?? 0) + 1);
  }
  const picked = [...counts.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, TOP_N)
    .sort((a, b) => a[0] - b[0]);
  const max = Math.max(1, ...picked.map(([, c]) => c));
  let peak = 0;
  picked.forEach(([, c], i) => {
    if (i > 0 && c > picked[peak][1]) peak = i;
  });
  return {
    labels: picked.map(([y]) => String(y)),
    values: picked.map(([, c]) => c / max),
    /* 柱宽在样式里封顶（见 .ov-years 的 max-width）：
       柱数不足 5（如片库只有一年）时不补空槽位 —— 空槽会被读成"那几年没拍照片"，
       那是编出来的信息。封顶之后唯一的柱就是一根 34px 的柱，剩下的留白本身
       已经把"只有一年"说清楚了。 */
    peak,
    /* 口径：年份只能从 list_timeline 得到（**已入库**照片），
       而地点是 list_photo_locations 的**全库**聚合 —— 两者不同分母，
       所以两栏各自标自己的总数，不跨栏相加。 */
    total: scanned.value,
  };
});

/** 最近导入：时间线倒序前 6 张 */
const recent = computed(() => [...timeline.value].reverse().slice(0, 6));
const recentCovers = computed(() =>
  recent.value.map((r) => ({ src: thumbMap.value[r.path] || "", name: r.album_name ?? "" })),
);

/** 相册拼贴：按张数取前 3 个有封面的相册 */
const albumMosaic = computed(() =>
  [...store.albums]
    .filter((a) => a.cover_path)
    .sort((a, b) => (b.photo_count || 0) - (a.photo_count || 0))
    .slice(0, 3)
    .map((a) => ({ src: fileUrl(a.cover_path), name: a.name })),
);
const avgPerAlbum = computed(() => (totalAlbums.value ? Math.round(totalPhotos.value / totalAlbums.value) : 0));

/** 人物：face_count 前 6（折叠由 FacesCore 处理） */
const topPersons = computed(() => [...persons.value].sort((a, b) => b.face_count - a.face_count).slice(0, 6));
const faceItems = computed(() =>
  topPersons.value.map((p) => ({ src: faceMap.value[p.id] || "", text: (p.name || "?").slice(0, 1) })),
);

/** 分类：启用的语义分类与计数（只用于「照片库总览」的一行元信息） */
const enabledCats = computed(() => categories.value.filter((c) => c.enabled));
const catCount = computed(() => catStats.value?.semantic_categories ?? enabledCats.value.length);
const catIndexed = computed(() => catStats.value?.indexed ?? scanned.value);
const catStale = computed(() => catStats.value?.stale ?? 0);

/** 那年今日（过去年份同月同日） */
const mm = String(now.getMonth() + 1).padStart(2, "0");
const dd = String(now.getDate()).padStart(2, "0");
const onThisDay = computed(() =>
  timeline.value.filter(
    (r) => r.shoot_time?.slice(5, 7) === mm && r.shoot_time?.slice(8, 10) === dd && Number(r.shoot_time.slice(0, 4)) < year,
  ),
);
const memoryHero = computed(() => [...onThisDay.value].reverse()[0] ?? null);
const memoryCover = computed(() => (memoryHero.value ? thumbMap.value[memoryHero.value.path] || "" : ""));
const memoryYearsAgo = computed(() =>
  memoryHero.value ? year - Number(memoryHero.value.shoot_time!.slice(0, 4)) : 0,
);

async function loadThumb(paths: { albumId: number; path: string }[]) {
  const byAlbum = new Map<number, string[]>();
  for (const { albumId, path } of paths) {
    if (thumbMap.value[path]) continue;
    if (!byAlbum.has(albumId)) byAlbum.set(albumId, []);
    byAlbum.get(albumId)!.push(path);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, ps]) => {
      try {
        for (const [path, thumb] of await getThumbs(aid, ps)) thumbMap.value[path] = convertFileSrc(thumb);
      } catch {
        /* 缩略图失败不阻塞概览 */
      }
    }),
  );
}

async function load() {
  loading.value = true;
  error.value = "";
  try {
    if (!store.albums.length) await store.fetchAlbums();
    await overview.load();

    /* 人物头像（bulk，一次往返）。头像单独拿、不并进快照：
       它依赖人物列表而且在裁剪缓存里，重算成本低，但没必要每次重算统计都重拉。 */
    try {
      const top = [...persons.value].sort((a, b) => b.face_count - a.face_count).slice(0, 6).map((p) => p.id);
      if (top.length) {
        const paths = await invoke<(string | null)[]>("get_person_avatars_bulk", { pids: top });
        top.forEach((pid, i) => {
          if (paths[i]) faceMap.value[pid] = convertFileSrc(paths[i]!);
        });
      }
    } catch {
      /* 无头像则显示姓名首字 */
    }

    /* 缩略图：最近 6 张 + 那年今日封面（thumbQueue 内部已有 path 级缓存，重复调用无 IPC） */
    const need = [...recent.value, ...(memoryHero.value ? [memoryHero.value] : [])]
      .filter((r) => r.album_id != null)
      .map((r) => ({ albumId: r.album_id!, path: r.path }));
    await loadThumb(need);
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

/** 手动刷新（强制重算，忽略缓存） */
async function refresh() {
  overview.invalidate();
  await load();
}

onMounted(load);

/* ───────────────────────── 导航 ───────────────────────── */
function goFace(pid?: string) {
  router.push({ path: "/smart", query: { tab: "face", ...(pid ? { person: pid } : {}) } });
}
function goLocation() {
  router.push({ path: "/smart", query: { tab: "location" } });
}
function goAlbum(id?: number | null) {
  if (id != null) router.push(`/album/${id}`);
}
</script>

<template>
  <div class="overview">
    <!-- 骨架：仅首次构建（无任何缓存快照）时出现；
         之后再进概览直接渲染缓存，不会闪一下白块。 -->
    <div v-if="loading && !overview.snapshot" class="ov-skel" aria-label="正在汇总照片库">
      <span v-for="i in 7" :key="i" class="ov-skel-card"></span>
    </div>

    <!-- 失败：不阻塞智慧相册其他分区，给重试 -->
    <div v-else-if="error" class="ov-error">
      <p>全局概览加载失败：{{ error }}</p>
      <button type="button" @click="refresh">重试</button>
    </div>

    <WidgetGrid v-else>
      <!-- ══ 行 1｜声部 A：纯读数（无板） ══════════════════════════════════
           「我的库有多大、建到什么程度」—— 这页要回答的第一个问题。
           数字直接印在背景上：没有板，它就不是"一个卡片"，而是版面本身。 -->
      <UiWidget :span="7" :rows="2" variant="bare" label-cn="照片库" :to="'/albums'">
        <div class="ov-hero">
          <div class="ov-hero-main">
            <div class="ov-figure">
              <span class="w-num-xl">{{ totalPhotos.toLocaleString() }}</span>
              <span class="w-stat-unit">张照片</span>
            </div>
            <!-- 年份跨度放在元信息行，而不是标题右侧：
                 标题行右侧紧挨着右边那一块的标题（间距只有 14px），
                 两个右对齐的小字会被读成同一行。 -->
            <div class="w-meta-row ov-hero-meta">
              <span>{{ fmtSize(totalBytes) }}</span>
              <span>{{ totalAlbums }} 个相册</span>
              <span v-if="years">{{ years.min }} – {{ years.max }}</span>
            </div>
            <!-- 入库完成度：金色预算第 1 处。整屏只有这条线和下面的当月柱是金色。 -->
            <div class="ov-ingest">
              <span class="ov-ingest-label">已入库 <b>{{ scanned.toLocaleString() }}</b> / {{ totalPhotos.toLocaleString() }}</span>
              <span class="ov-ingest-pct">{{ (knownRatio * 100).toFixed(1) }}%</span>
            </div>
            <div class="w-pbar hot"><i :style="{ width: `${Math.min(100, knownRatio * 100)}%` }"></i></div>
          </div>

          <!-- 右栏：语义索引。它回答"我现在能搜到多少"，与"库有多大"是同一件事的两面，
               所以留在同一块里，用一根发丝竖线分开 —— 而不是再开一块 widget。
               这块同时也把 A 的右半边填满：7 列宽的读数块如果只有左半边有字，
               视觉重量会全压在左边，和右边 5 列的柱阵失衡。 -->
          <aside class="ov-hero-side">
            <span class="ov-side-label">语义索引</span>
            <div class="ov-side-num">{{ (indexedRatio * 100).toFixed(1) }}<em>%</em></div>
            <p class="ov-side-note">{{ catIndexed.toLocaleString() }} 张已建索引 · {{ catCount }} 个分类</p>
            <p v-if="catStale > 0" class="ov-side-note">
              <span class="hot">{{ catStale }} 张</span> 属其他模型档
            </p>
            <p v-else class="ov-side-note">索引完整</p>
          </aside>
        </div>
      </UiWidget>

      <!-- 「今年怎么样」——柱阵是"节奏"，不是"占比"，所以用柱不用线 -->
      <UiWidget :span="5" :rows="2" variant="bare" label-cn="今年月度" :to="'/timeline'">
        <template #right>{{ year }} · 当月 {{ monthBars.counts[now.getMonth()] }} 张</template>
        <div class="ov-months">
          <BarsCore :values="monthBars.values" :active-indexes="[monthBars.active]" />
        </div>
        <div class="w-meta-row ov-months-meta">
          <span>全年 {{ yearTotal.toLocaleString() }} 张</span>
          <span>月均 {{ yearMonthlyAvg }} 张</span>
        </div>
      </UiWidget>

      <!-- ══ 行 2｜声部 B：有内容才配板 ═══════════════════════════════════ -->

      <!-- 相册：拼贴是"内容"，需要承载面 -->
      <UiWidget :span="4" :rows="2" label-cn="相册" clickable @click="router.push('/albums')">
        <template #right>{{ totalAlbums }} 个</template>
        <CoversCore v-if="albumMosaic.length" :items="albumMosaic" variant="mosaic" @pick="() => router.push('/albums')" />
        <div v-else class="ov-empty">还没有相册</div>
        <p class="w-stat-note ov-foot">共 {{ totalPhotos.toLocaleString() }} 张 · 平均 {{ avgPerAlbum }} 张/册</p>
      </UiWidget>

      <!-- 人物：覆盖率先在这块说清，别处不再重复 -->
      <UiWidget :span="4" :rows="2" label-cn="人物" clickable @click="goFace()">
        <template #right>{{ persons.length }} 位</template>
        <FacesCore v-if="faceItems.length" :items="faceItems" :max="5" @pick="(i: number) => goFace(topPersons[i]?.id)" />
        <div v-else class="ov-empty">尚未识别出人物</div>
        <p class="w-stat-note ov-foot">{{ (faceRatio * 100).toFixed(1) }}% 的照片含人物 · {{ facePhotos.toLocaleString() }} 张</p>
      </UiWidget>

      <!-- 地点 / 年份（FEAT-120）：原来这里是一张「假装的地图」（点阵 + 橙点），
           在 400×168 里只能读出「有定位信息」这一个结论 —— 而那句话脚注已经说完了。
           现在换成两个榜：左地点 Top5、右年份 Top5。

           **两栏之间不画分隔线**：406px 若是切出一根竖线，板里就多出一道假结构，
           读成「两个小格子」；而这两栏其实是同一句话的两面 —— 照片都在哪、什么时候拍的。
           不画线之后靠**形态差异**区分：左边横向排行条（地名是无序的，横条最省高度、
           地名读得清），右边竖柱阵（年份是有序的，柱才看得出「哪几年多、哪几年空」）。

           口径不同必须各自标明：地点来自 list_photo_locations（全库聚合），
           年份只能从 list_timeline 算（已入库）。所以右栏自带「已入库 N 张」，
           不与左栏相加、不共用一个百分比 —— 这正是不再出现「591.4% 带 GPS」的前提。 -->
      <UiWidget :span="4" :rows="2" label-cn="地点" clickable @click="goLocation()">
        <template #right>{{ namedLoc.length }} 城</template>
        <div v-if="topPlaces.length || yearTop.labels.length" class="ov-split">
          <RankBarsCore v-if="topPlaces.length" dense tone="muted" :items="topPlaces" />
          <p v-else class="ov-split-none">无定位信息</p>
          <div v-if="yearTop.labels.length" class="ov-split-r">
            <div class="ov-years">
              <BarsCore :values="yearTop.values" :labels="yearTop.labels" :active-indexes="[yearTop.peak]" />
            </div>
            <p class="ov-years-note">已入库 {{ yearTop.total.toLocaleString() }} 张</p>
          </div>
          <p v-else class="ov-split-none">无拍摄时间</p>
        </div>
        <div v-else class="ov-empty">照片里还没有定位信息</div>
        <p class="w-stat-note ov-foot">{{ (gpsRatio * 100).toFixed(1) }}% 带 GPS · {{ geoPhotos.toLocaleString() }} 张</p>
      </UiWidget>

      <!-- ══ 行 3｜内容为主的两块 ════════════════════════════════════════ -->

      <!-- 最近导入：照片下出血铺到板缘 —— 板是相框，不是"贴着照片的纸" -->
      <UiWidget :span="8" :rows="2" label-cn="最近导入" bleed class="ov-film" :to="'/timeline'">
        <template #right>时间线 →</template>
        <CoversCore
          v-if="recentCovers.length"
          :items="recentCovers"
          variant="strip"
          @pick="(i: number) => goAlbum(recent[i]?.album_id)"
        />
        <div v-else class="ov-empty">还没有已入库的照片</div>
      </UiWidget>

      <!-- 那年今日：整块就是一张照片，标题浮在照片上（头部只留标题，不再重复"x 年前"） -->
      <UiWidget :span="4" :rows="2" label-cn="那年今日" cover :to="'/memories'">
        <MemoryCore
          v-if="memoryHero"
          :src="memoryCover"
          :title="`${memoryYearsAgo} 年前的今天`"
          :subtitle="`${memoryHero.shoot_time!.slice(0, 4)}${memoryHero.location ? ' · ' + memoryHero.location : ''}`"
          @activate="router.push('/memories')"
        />
        <div v-else class="ov-empty ov-empty-mem">往年今天的照片会出现在这里</div>
      </UiWidget>
    </WidgetGrid>
  </div>
</template>

<style scoped>
.overview {
  padding: 6px var(--sp-8) 48px;
}

/* ── 照片库总览（裸读数）─────────────────────────────────────────────
   左栏纵向节奏：大数字 → 元信息 → 入库行 → 占比线；
   右栏是一条发丝竖线隔开的「语义索引」小读数块。
   入库行与占比线贴底（margin-top:auto），于是它和右边「今年月度」的
   柱阵脚注落在同一条底线上 —— 这是"错落但整齐"里"整齐"的那一半。 */
.ov-hero {
  flex: 1;
  display: flex;
  align-items: stretch;
  gap: 24px;
  min-height: 0;
  padding-top: 6px;
}
.ov-hero-main {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.ov-figure {
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.ov-hero-meta {
  margin-top: 10px;
}

/* 右栏：唯一的一根竖发丝线（不做成第二条数据带，避免"两条统计轴"） */
.ov-hero-side {
  flex: 0 0 172px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding-left: 22px;
  border-left: 1px solid var(--rule);
  min-width: 0;
}
.ov-side-label {
  font-size: 9.5px;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  color: var(--fg-2);
  font-weight: 600;
}
.ov-side-num {
  margin-top: 4px;
  font-size: 34px;
  font-weight: 300;
  letter-spacing: -1.3px;
  line-height: 1;
  color: var(--fg);
  font-variant-numeric: tabular-nums;
}
.ov-side-num em {
  font-style: normal;
  font-size: 15px;
  letter-spacing: 0;
  color: var(--fg-2);
  margin-left: 1px;
}
.ov-side-note {
  margin: 0;
  font-size: 10.5px;
  color: var(--fg-2);
  font-variant-numeric: tabular-nums;
}
.ov-side-note:first-of-type {
  margin-top: 8px;
}
.ov-side-note .hot {
  /* 跟随**当前预设的仪表墙强调色**，而不是全站的落日金：
     松绿/陶土/青瓷这几套的墙色已经换过，这里若仍用金色，
     会出现"整块墙是薄荷绿、只有这一行是金色"的串色。 */
  color: var(--w-accent);
}

.ov-ingest {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-top: auto;
  padding-top: 14px;
  font-size: 10.5px;
  color: var(--fg-2);
}
.ov-ingest b {
  color: var(--fg);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.ov-ingest-pct {
  color: var(--fg);
  font-variant-numeric: tabular-nums;
}
.ov-ingest + .w-pbar {
  margin-top: 7px;
}

/* ── 今年月度 ───────────────────────────────────────────────────────── */
.ov-months {
  flex: 1;
  display: flex;
  min-height: 0;
  padding: 14px 0 12px;
}
.ov-months-meta {
  margin-top: auto;
}

/* ── 空态 / 骨架 / 错误 ─────────────────────────────────────────────── */
.ov-empty {
  flex: 1;
  display: grid;
  place-items: center;
  font-size: 11px;
  color: var(--w-fg2, var(--color-text-2));
  text-align: center;
}
.ov-empty-mem {
  margin-top: 10px;
  border-radius: var(--w-radius-inner);
  border: 1px dashed var(--w-line);
}
/* FEAT-117：满幅板（cover）的空态。
   这块板的"照片"位没有照片时由 widget.css 补一层底：深色模式近黑、浅色模式极淡墨渍
   （浅色页上摆一块黑盒子反而更抢眼）。底色换了，字也必须跟着换挡 ——
   写在这里而不是 widget.css，因为这个空态文案归本组件所有。 */
.ui-widget--cover .ov-empty {
  color: rgba(255, 255, 255, 0.74);
}
/* 注意：这里不能写 :global(...) —— Vue 的 scoped 编译器会把整条规则丢掉（编译产物里查无此规则）。
   直接写 body 前缀即可：scoped 只给最后一段加 [data-v-*]，body 本来就不是组件内部元素。 */
body:not(.theme-dark) .ui-widget--cover .ov-empty {
  color: var(--color-text-3);
}
.ov-skel {
  display: grid;
  grid-template-columns: repeat(12, 1fr);
  grid-auto-rows: 84px;
  gap: 14px;
  padding: 6px var(--sp-8);
}
.ov-skel-card {
  grid-column: span 4;
  grid-row: span 2;
  border-radius: var(--w-radius);
  background: linear-gradient(100deg, rgba(128, 128, 128, 0.1) 30%, rgba(128, 128, 128, 0.2) 50%, rgba(128, 128, 128, 0.1) 70%);
  background-size: 200% 100%;
  animation: ov-shimmer 1.3s infinite;
}
.ov-skel-card:nth-child(1) { grid-column: span 7; }
.ov-skel-card:nth-child(2) { grid-column: span 5; }
.ov-skel-card:nth-child(6) { grid-column: span 8; }
@keyframes ov-shimmer {
  from { background-position: 200% 0; }
  to { background-position: -200% 0; }
}
.ov-error {
  padding: 40px var(--sp-8);
  text-align: center;
  color: var(--fg-2);
  font-size: 12px;
}
.ov-error button {
  margin-top: 10px;
  padding: 6px 16px;
  border-radius: var(--r-ui);
  border: 1px solid var(--w-line);
  background: transparent;
  color: inherit;
  cursor: pointer;
}

/* 面板脚注统一贴底：三块面板的同一行文字因此落在同一条基线上 */
.overview .ov-foot {
  margin-top: auto;
  padding-top: 8px;
}

/* ── 地点 / 年份两栏（FEAT-120）─────────────────────────────────────────────
   刻意**没有分隔线**：406px 宽的板里再切一根竖线，会被读成"两个小格子"，
   而这两栏其实是同一句话的两面。靠形态（横排行 vs 竖柱阵）区分就够。
   两栏等宽 + 18px 间距，左栏底端因此与右栏的柱基对齐。 */
.ov-split {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 18px;
  /* 顶对齐：两栏共用同一条顶线（左榜第一行 = 柱区顶）。
     试过底对齐 —— 左榜最后一行会跟右栏的「已入库 N 张」撞在同一行上，
     那行小字被读成"第 5 名的脚注"，比留白糟。 */
  align-items: start;
  margin-top: 12px;
}
.ov-split-r {
  display: flex;
  flex-direction: column;
  gap: 5px;
}
/* 柱区固定高 + 底部留出 15px 给 BarsCore 绝对定位的年份小字，
   否则年份会被板的 padding 裁掉。 */
.ov-years {
  display: flex;
  height: 132px;
  padding-bottom: 15px;
}
/* 柱宽封顶：BarsCore 的柱是 flex:1，柱数少时会被拉成满宽的色块。
   封顶到 34px 后，5 根柱刚好铺满一栏，1 根柱就只是一根柱。 */
.ov-years :deep(.w-bars i) {
  max-width: 34px;
}
.ov-years-note {
  margin: 0;
  font-size: 9px;
  color: var(--fg-3);
  font-variant-numeric: tabular-nums;
}
.ov-split-none {
  margin: 0;
  font-size: 10px;
  color: var(--fg-3);
}

/* 最近导入：出血后封面不再有独立圆角，整条读作"一条胶片" */
.ov-film :deep(.w-covers.strip) {
  margin-top: 0;
  gap: 2px;
}
.ov-film :deep(.w-cv) {
  border-radius: 0;
  box-shadow: none;
}
/* 那年今日：整块是照片，取消芯子自己的圆角与外边距（板已经给了圆角） */
.overview :deep(.ui-widget--cover .w-memory) {
  margin-top: 0;
  border-radius: 0;
  box-shadow: none;
}

@media (max-width: 1000px) {
  .overview {
    padding: 4px var(--sp-5) 40px;
  }
  /* 窄屏：右栏落到大数字下面，竖发丝线改为横线（竖线在窄屏上会切断阅读） */
  .ov-hero {
    flex-direction: column;
    gap: 16px;
  }
  .ov-hero-side {
    flex: none;
    padding-left: 0;
    padding-top: 12px;
    border-left: none;
    border-top: 1px solid var(--rule);
  }
}
</style>
