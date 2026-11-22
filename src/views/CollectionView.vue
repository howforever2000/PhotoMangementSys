<script setup lang="ts">
/**
 * CollectionView —— 通用照片集合路由页（FEAT-201）
 *
 * /collection?type=day&date=YYYY-MM-DD      按拍摄日（概览「那年今日」直达）
 * /collection?type=month&year=YYYY&month=M  按月（回忆页月度故事卡直达，FEAT-205）
 * /collection?type=year&year=YYYY           按年（回忆页年度回顾卡直达，FEAT-205）
 * /collection?type=recent                   最近导入（按入库序倒序）
 *
 * 为什么是独立路由而不是 /smart 内的二级视图：
 *   概览页（/smart 的全局概览 tab）与回忆页都可能发起这类跳转；
 *   独立路由让返回逻辑只有一种（history back），不用在宿主页里
 *   各自维护"从哪个 tab 进来的"状态。
 *
 * 后端零重量级新链路：timeline_day / timeline_recent 均为
 * photo_content_scan 单表 + 相册/缩略图 LEFT JOIN 的分页查询
 * （首屏 300 张，滚动追加），不再像 Memories ?day 那样为看一天
 * 的照片拉 15 MB 全量时间线。
 */
import { computed } from "vue";
import { useRoute } from "vue-router";
import { invoke } from "@tauri-apps/api/core";
import PhotoCollectionPage from "../components/PhotoCollectionPage.vue";
import type { CollectionPhoto } from "../types/collection";
import type { TimelineCard } from "../types/timeline";

const route = useRoute();

/** TimelineCard → 集合最小行 */
function mapCard(c: TimelineCard): CollectionPhoto {
  return { path: c.path, albumId: c.album_id, shootTime: c.shoot_time, location: c.location };
}

const config = computed(() => {
  const type = String(route.query.type ?? "");
  if (type === "day") {
    const date = String(route.query.date ?? "");
    if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) return null;
    const yearsAgo = new Date().getFullYear() - Number(date.slice(0, 4));
    return {
      key: `day:${date}`,
      title: yearsAgo > 0 ? `${yearsAgo} 年前的今天 · ${date}` : `${date}`,
      subtitle: "",
      loader: (limit: number, offset: number) =>
        invoke<TimelineCard[]>("timeline_day", { day: date, limit, offset }).then((rows) =>
          rows.map(mapCard),
        ),
    };
  }
  // FEAT-205：回忆页月度故事卡直达（复用 timeline_month 的分页查询）
  if (type === "month") {
    const y = String(route.query.year ?? "");
    const m = String(route.query.month ?? "");
    if (!/^\d{4}$/.test(y) || !/^\d{1,2}$/.test(m)) return null;
    const mm = m.padStart(2, "0");
    return {
      key: `month:${y}-${mm}`,
      title: `${Number(y)} 年 ${Number(m)} 月`,
      subtitle: "",
      loader: (limit: number, offset: number) =>
        invoke<TimelineCard[]>("timeline_month", { year: y, month: mm, limit, offset }).then(
          (rows) => rows.map(mapCard),
        ),
    };
  }
  // FEAT-205：回忆页年度回顾卡直达（timeline_year：整年聚合，含残缺拍摄时间）
  if (type === "year") {
    const y = String(route.query.year ?? "");
    if (!/^\d{4}$/.test(y)) return null;
    return {
      key: `year:${y}`,
      title: `${Number(y)} 年`,
      subtitle: "按拍摄时间升序",
      loader: (limit: number, offset: number) =>
        invoke<TimelineCard[]>("timeline_year", { year: y, limit, offset }).then((rows) =>
          rows.map(mapCard),
        ),
    };
  }
  if (type === "recent") {
    return {
      key: "recent",
      title: "最近导入",
      subtitle: "按导入时间倒序",
      loader: (limit: number, offset: number) =>
        invoke<TimelineCard[]>("timeline_recent", { limit, offset }).then((rows) =>
          rows.map(mapCard),
        ),
    };
  }
  return null;
});
</script>

<template>
  <div class="collection-page">
    <PhotoCollectionPage
      v-if="config"
      :key="config.key"
      :title="config.title"
      :subtitle="config.subtitle"
      :loader="config.loader"
    />
    <div v-else class="collection-bad">
      <p>无效的集合参数</p>
      <p class="collection-bad-hint">需要 ?type=day&date=YYYY-MM-DD 或 ?type=recent</p>
    </div>
  </div>
</template>

<style scoped>
.collection-page {
  min-height: 100%;
}
.collection-bad {
  padding: 60px 20px;
  text-align: center;
  color: var(--fg-2);
}
.collection-bad p {
  margin: 0 0 6px;
}
.collection-bad-hint {
  font-size: 12px;
  opacity: 0.7;
}
</style>
