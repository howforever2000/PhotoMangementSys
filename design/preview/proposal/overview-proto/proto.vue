<script setup lang="ts">
/**
 * 全局概览改版原型（可复现稿件，不进 src/，不参与构建）
 *   ?v=a 方案 A「保留原骨架，只补三处短板」
 *   ?v=b 方案 B「年份主轴」
 */
import { computed } from "vue";
import WidgetGrid from "../../../../src/components/ui/widget/WidgetGrid.vue";
import UiWidget from "../../../../src/components/ui/widget/UiWidget.vue";
import BarsCore from "../../../../src/components/ui/widget/BarsCore.vue";
import CoversCore from "../../../../src/components/ui/widget/CoversCore.vue";
import FacesCore from "../../../../src/components/ui/widget/FacesCore.vue";
import RankBarsCore from "../../../../src/components/ui/widget/RankBarsCore.vue";
import MemoryCore from "../../../../src/components/ui/widget/MemoryCore.vue";
import AppHeader from "../../../../src/components/ui/AppHeader.vue";

const props = withDefaults(defineProps<{ variant?: string }>(), { variant: "a" });

const PHOTO = "/design/preview/proposal/lightbox-photo.svg";

/* ── 假数据（与真实快照同形，只用于出图）────────────────── */
const monthValues = [0.42, 0.55, 0.72, 0.61, 0.68, 0.58, 0.74, 0.83, 0.91, 1.0, 0, 0];
const monthLabels = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12"];

const yearList = [
  ["2015", 320], ["2016", 480], ["2017", 610], ["2018", 940],
  ["2019", 1180], ["2020", 1520], ["2021", 1980], ["2022", 2410],
  ["2023", 3120], ["2024", 3680], ["2025", 4210], ["2026", 5095],
] as const;
const yearMax = Math.max(...yearList.map(([, n]) => n));
const yearValues = yearList.map(([, n]) => n / yearMax);
const yearLabels = yearList.map(([y]) => y);

const albumMosaic = Array.from({ length: 4 }, (_, i) => ({
  src: PHOTO,
  name: ["家庭日常", "大理 · 洱海", "女儿成长", "川西环线"][i],
}));
const faceItems = ["阿", "小", "妈", "爸", "团", "?", "?", "外"].map((t) => ({ src: "", text: t }));
/** 人物块：4 列 × 2 行，头像 + 姓名 + 张数 */
const faceList = [
  { name: "阿哲", n: 1840 }, { name: "小雨", n: 1620 }, { name: "妈妈", n: 1310 }, { name: "爸爸", n: 1120 },
  { name: "团子", n: 980 }, { name: "未命名", n: 760 }, { name: "未命名", n: 640 }, { name: "外婆", n: 520 },
];
const places = [
  { label: "朝阳区", value: 2140, ratio: 1 },
  { label: "大理", value: 1860, ratio: 0.87 },
  { label: "徐汇区", value: 1420, ratio: 0.66 },
  { label: "成都", value: 980, ratio: 0.46 },
  { label: "无锡", value: 760, ratio: 0.36 },
];
const recent = [
  { name: "大理 · 洱海", t: "10-28 17:42" },
  { name: "家庭日常", t: "10-28 09:10" },
  { name: "川西环线", t: "10-27 18:03" },
  { name: "婚礼 · 阿哲", t: "10-27 11:26" },
  { name: "扫街 · 上海", t: "10-26 20:41" },
  { name: "女儿成长", t: "10-26 07:55" },
];

const isA = computed(() => props.variant !== "b");

/** 与 SmartAlbum 完全一致的模块页头（让效果图与实际页面同框可比） */
const tabs = [
  { key: "overview", label: "全局概览" },
  { key: "category", label: "内容分类" },
  { key: "face", label: "人物", count: 12 },
  { key: "location", label: "地点" },
  { label: "回忆", to: "/memories" },
  { label: "时间线", to: "/timeline" },
  { label: "智能搜索", to: "/search" },
];
</script>

<template>
  <div class="proto-page">
    <AppHeader title="智慧相册" icon="🧠" meta="12 位人物 · 12 个相册" :tabs="tabs" active="overview" />
    <div class="proto">
      <!-- ══════════ 方案 A ══════════ -->
      <WidgetGrid v-if="isA">
        <!-- ① 照片库（7×2）：上半大数字，下半「入库 / 索引」两栏铺满整块宽度 -->
        <UiWidget :span="7" :rows="2" variant="bare" label-cn="照片库" to="/albums">
          <div class="hero">
            <div class="figure">
              <span class="w-num-xl">24,980</span>
              <span class="w-stat-unit">张照片</span>
            </div>
            <div class="w-meta-row">
              <span>137.2 GB</span><span>12 个相册</span><span>2015 – 2026</span>
            </div>
            <div class="hero-split">
              <div class="hero-col">
                <span class="mini-label">已入库</span>
                <div class="mini-num">73.7<em>%</em></div>
                <div class="w-pbar hot"><i style="width: 73.7%"></i></div>
                <div class="mini-note">18,420 / 24,980 张</div>
              </div>
              <div class="hero-col">
                <span class="mini-label">语义索引</span>
                <div class="mini-num">93.5<em>%</em></div>
                <div class="mini-note">17,230 张已建索引 · 9 个分类</div>
                <div class="mini-note"><span class="hot">118 张</span> 属其他模型档</div>
              </div>
            </div>
          </div>
        </UiWidget>

        <!-- ② 今年月度（5×2）：补上 1…12 的月份刻度 -->
        <UiWidget :span="5" :rows="2" variant="bare" label-cn="今年月度" to="/timeline">
          <template #right>2026 · 当月 416 张</template>
          <div class="months">
            <BarsCore :values="monthValues" :labels="monthLabels" :active-indexes="[9]" />
          </div>
          <div class="w-meta-row months-meta">
            <span>全年 5,095 张</span>
            <span>月均 510 张</span>
          </div>
        </UiWidget>

        <!-- ③ 三个入口：宽度按内容形态分配 -->
        <UiWidget :span="4" :rows="2" label-cn="相册" clickable>
          <template #right>12 个</template>
          <CoversCore :items="albumMosaic" variant="mosaic" />
          <p class="w-stat-note foot">共 24,980 张 · 平均 2,082 张/册</p>
        </UiWidget>

        <UiWidget :span="4" :rows="2" label-cn="人物" clickable>
          <template #right>12 位</template>
          <div class="faces">
            <div v-for="(f, i) in faceList" :key="i" class="face">
              <span class="face-dot"><img :src="PHOTO" alt="" /></span>
              <span class="face-name">{{ f.name }}</span>
              <span class="face-num">{{ f.n }}</span>
            </div>
          </div>
          <p class="w-stat-note foot">61.1% 的照片含人物 · 11,260 张</p>
        </UiWidget>

        <UiWidget :span="4" :rows="2" label-cn="地点" clickable>
          <template #right>8 城</template>
          <RankBarsCore dense tone="muted" :items="places" />
          <p class="w-stat-note foot">34.8% 带 GPS · 8,700 张</p>
        </UiWidget>

        <!-- ④ 最近导入（8×2）：细带 → 3×2 网格，每张带相册名与日期 -->
        <UiWidget :span="8" :rows="2" label-cn="最近导入" to="/timeline">
          <template #right>时间线 →</template>
          <div class="recent">
            <div v-for="r in recent" :key="r.name + r.t" class="recent-cell">
              <img :src="PHOTO" alt="" />
              <div class="recent-cap">
                <span class="recent-name">{{ r.name }}</span>
                <span class="recent-time">{{ r.t }}</span>
              </div>
            </div>
          </div>
        </UiWidget>

        <!-- ⑤ 那年今日（4×2）：与现状一致的大封面，保住"情感钩子"的重量 -->
        <UiWidget :span="4" :rows="2" label-cn="那年今日" cover to="/memories">
          <MemoryCore src="/design/preview/proposal/lightbox-photo.svg" title="3 年前的今天" subtitle="2023 · 江苏无锡 · 那天一共 7 张" />
        </UiWidget>
      </WidgetGrid>

      <!-- ══════════ 方案 B ══════════ -->
      <WidgetGrid v-else>
        <UiWidget :span="12" :rows="2" variant="bare" label-cn="这些年" to="/timeline">
          <template #right>2015 – 2026 · 共 24,108 张有拍摄时间</template>
          <div class="spine">
            <div class="spine-head">
              <div class="figure">
                <span class="w-num-xl">24,980</span>
                <span class="w-stat-unit">张照片</span>
              </div>
              <div class="w-meta-row">
                <span>137.2 GB</span><span>12 个相册</span><span>峰值 2026 年 5,095 张</span>
              </div>
              <div class="spine-ingest">
                <span>已入库 <b>18,420</b> / 24,980</span>
                <span>73.7%</span>
              </div>
              <div class="w-pbar hot"><i style="width: 73.7%"></i></div>
            </div>
            <div class="spine-chart">
              <BarsCore :values="yearValues" :labels="yearLabels" :active-indexes="[11]" />
            </div>
          </div>
        </UiWidget>

        <UiWidget :span="5" :rows="2" label-cn="相册" clickable>
          <template #right>12 个</template>
          <CoversCore :items="albumMosaic" variant="mosaic" />
          <p class="w-stat-note foot">共 24,980 张 · 平均 2,082 张/册</p>
        </UiWidget>

        <UiWidget :span="3" :rows="2" label-cn="人物" clickable>
          <template #right>12 位</template>
          <FacesCore :items="faceItems" :max="6" />
          <p class="w-stat-note foot">61.1% 含人物</p>
        </UiWidget>

        <UiWidget :span="4" :rows="2" label-cn="地点" clickable>
          <template #right>8 城</template>
          <RankBarsCore dense tone="muted" :items="places" />
          <p class="w-stat-note foot">34.8% 带 GPS · 8,700 张</p>
        </UiWidget>

        <UiWidget :span="8" :rows="2" label-cn="最近导入" to="/timeline">
          <template #right>时间线 →</template>
          <div class="recent recent--strip">
            <div v-for="r in recent" :key="r.name + r.t" class="recent-cell">
              <img :src="PHOTO" alt="" />
              <div class="recent-cap"><span class="recent-time">{{ r.t }}</span></div>
            </div>
          </div>
        </UiWidget>

        <UiWidget :span="4" :rows="2" label-cn="那年今日" cover to="/memories">
          <MemoryCore src="/design/preview/proposal/lightbox-photo.svg" title="3 年前的今天" subtitle="2023 · 江苏无锡 · 共 7 张" />
        </UiWidget>
      </WidgetGrid>
    </div>
  </div>
</template>

<style scoped>
.proto {
  padding: 6px 52px 48px;
}
.proto-page {
  min-height: 100vh;
}
.hot {
  color: var(--w-accent);
}
.figure {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

/* ── 照片库：上半读数 + 下半两栏 ─────────────────────── */
.hero {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
.hero-split {
  margin-top: auto;
  display: flex;
  padding-top: 18px;
}
.hero-col {
  flex: 1 1 0;
  min-width: 0;
  padding-right: 30px;
}
.hero-col + .hero-col {
  padding-left: 30px;
  padding-right: 0;
  border-left: 1px solid var(--rule);
}
.mini-label {
  font-size: 10px;
  letter-spacing: 0.14em;
  color: var(--fg-2);
  font-weight: 600;
}
.mini-num {
  margin-top: 1px;
  font-size: 30px;
  font-weight: 300;
  letter-spacing: -1.2px;
  line-height: 1.15;
  color: var(--fg);
  font-variant-numeric: tabular-nums;
}
.mini-num em {
  font-style: normal;
  font-size: 14px;
  letter-spacing: 0;
  color: var(--fg-2);
  margin-left: 1px;
}
.mini-note {
  font-size: 10.5px;
  color: var(--fg-3);
  font-variant-numeric: tabular-nums;
  line-height: 1.7;
}
/* .w-pbar 默认 margin-top:auto（贴板底），这里要贴着读数 */
.hero .w-pbar {
  margin-top: 10px;
}

/* ── 面板脚注 ───────────────────────────────────────── */
.foot {
  margin-top: auto;
  padding-top: 8px;
}

/* ── 今年月度 ───────────────────────────────────────── */
.months {
  flex: 1;
  display: flex;
  min-height: 0;
  padding: 12px 0 10px;
}
.months-meta {
  margin-top: auto;
}

/* ── 人物：4×2 头像 + 姓名 ──────────────────────────── */
.faces {
  flex: 1;
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  grid-template-rows: repeat(2, 1fr);
  gap: 10px 8px;
  align-content: center;
  min-height: 0;
  margin-top: 6px;
}
.face {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  min-width: 0;
}
.face-dot {
  width: 52px;
  height: 52px;
  border-radius: 50%;
  overflow: hidden;
  background: rgba(128, 128, 128, 0.25);
  box-shadow: 0 3px 10px rgba(0, 0, 0, 0.4), inset 0 0 0 1px rgba(255, 255, 255, 0.08);
}
.face-dot img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.face-name {
  font-size: 10.5px;
  color: var(--fg-2);
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.face-num {
  font-size: 9px;
  color: var(--fg-3);
  font-variant-numeric: tabular-nums;
}

/* ── 最近导入网格 ───────────────────────────────────── */
.recent {
  flex: 1;
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  grid-template-rows: repeat(2, 1fr);
  gap: 10px;
  margin-top: 10px;
  min-height: 0;
}
.recent--strip {
  grid-template-columns: repeat(6, 1fr);
  grid-template-rows: 1fr;
  gap: 2px;
  margin-top: 8px;
}
.recent-cell {
  position: relative;
  overflow: hidden;
  border-radius: 10px;
  background: rgba(128, 128, 128, 0.12);
  min-height: 0;
}
.recent-cell img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.recent-cap {
  position: absolute;
  inset: auto 0 0 0;
  display: flex;
  justify-content: space-between;
  gap: 8px;
  padding: 14px 10px 8px;
  font-size: 10px;
  color: rgba(255, 255, 255, 0.9);
  background: linear-gradient(0deg, rgba(0, 0, 0, 0.62), transparent);
}
.recent-name {
  font-weight: 500;
}
.recent-time {
  opacity: 0.72;
  font-variant-numeric: tabular-nums;
}

/* ── 方案 B：年份主轴 ───────────────────────────────── */
.spine {
  display: flex;
  gap: 34px;
  flex: 1;
  min-height: 0;
  padding-top: 4px;
}
.spine-head {
  flex: 0 0 280px;
  display: flex;
  flex-direction: column;
}
.spine-ingest {
  display: flex;
  justify-content: space-between;
  margin-top: 16px;
  font-size: 10.5px;
  color: var(--fg-2);
}
.spine-ingest b {
  color: var(--fg);
  font-variant-numeric: tabular-nums;
}
.spine-head .w-pbar {
  margin-top: 8px;
}
.spine-chart {
  flex: 1;
  display: flex;
  min-height: 0;
  padding-bottom: 4px;
}
</style>
