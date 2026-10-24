<script setup lang="ts">
// 相册卡片组件 —— 从 AlbumList.vue 抽取
// 消除日期视图（未分类/月分组）与地点视图中 3 份重复的卡片模板
import { computed } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Album } from "../types/album";
import { formatSize } from "../types/album";

const props = withDefaults(
  defineProps<{
    album: Album;
    /** 是否处于勾选管理模式 */
    selectMode: boolean;
    /** 当前卡片是否被勾选 */
    selected: boolean;
    /** 是否显示地点徽标（地点排序视图用） */
    showLocation?: boolean;
    /**
     * FEAT-103：渲染变体
     *  - `"card"`（默认）：旧卡片形——带背景/阴影/内边距的全信息卡
     *  - `"tile"`：裸封面 + 两行图注（Canvas 极简版）
     *
     * 为什么用变体而不是新建一个组件：
     *   这张卡上挂着多选、右键菜单、封面来源、合并来源链接、已入库徽标、
     *   路径点击等 6 类业务交互（见 emit 列表）。新建组件就得把这些全部
     *   再实现一遍，或者在外面套一层代理——两种都比加一个 prop 风险高。
     *   而 tile 需要的只是**不同的展示骨架**，交互层完全一样。
     *
     *   同时这也让 AlbumList 的切换可以逐视图推进（日期视图先用 tile，
     *   地点/手动视图暂时还是 card），不必一次改完 2883 行。
     */
    variant?: "card" | "tile";
    /** FEAT-105：mosaic = 参与跨格错落排布的大瓦片。
     *  封面填满格子，图注压在封面底部（海报落款），而不是封面下方。 */
    mosaic?: boolean;
  }>(),
  { showLocation: false, variant: "card", mosaic: false },
);

const emit = defineEmits<{
  (e: "click"): void;
  (e: "contextmenu", ev: MouseEvent): void;
  (e: "toggle-select", ev: MouseEvent): void;
  (e: "open-path", ev: MouseEvent): void;
  /** FEAT-A：点击合并来源路径时触发（path 链接回系统文件夹） */
  (e: "open-source-path", payload: { id: number; path: string }): void;
}>();

/** FEAT-036：该相册是否已入库（只要有一张照片被内容扫描识别入库即视为已入库） */
const isScanned = computed(() => (props.album.scanned_photo_count || 0) > 0);

const isTile = computed(() => props.variant === "tile");

/**
 * tile 变体的图注右侧元信息。
 *
 * 组装顺序有讲究：先给"有多少张"——用户在扫视相册网格时第一眼找的是
 * "这个相册多大/是不是我要的那个"。时间已移到封面左下角（FEAT-105），
 * 不再重复出现在图注里。
 *
 * 为什么不带 size_bytes：在极简版面上，体积对相册浏览几乎无决策价值
 * （用户不会因为某个相册 200MB 而改变行为），但张数会。体积仍保留在
 * tile 的 title 里（悬停可见），不占主视觉。
 */
/** 封面左下角的时间标签：年 + 月（"2024年6月"），日太细不占位 */
const tileTime = computed(() => {
  const t = props.album.shoot_time;
  if (!t) return "";
  const y = t.slice(0, 4);
  const m = t.slice(5, 7);
  return m ? `${y}年${Number(m)}月` : `${y}年`;
});

const tileMeta = computed(() => {
  const parts: string[] = [`${props.album.photo_count} 张`];
  // mosaic 大瓦片：时间标签被图注遮罩盖住（它叠在封面左下角），
  // 所以时间改到图注里；小瓦片的时间在封面左下角，图注不再重复。
  if (props.mosaic && props.album.shoot_time) parts.push(tileTime.value);
  if (props.showLocation && props.album.location) parts.push(props.album.location);
  return parts.join(" · ");
});

/** tile 的 title：把 card 里有但 tile 省掉的字段补回来（悬停可查） */
const tileTitle = computed(() => {
  const lines = [props.album.name, `${props.album.photo_count} 张 · ${formatSize(props.album.size_bytes)}`];
  if (props.album.shoot_time) lines.push(`拍摄：${props.album.shoot_time}`);
  if (props.album.location) lines.push(`地点：${props.album.location}`);
  lines.push(props.album.path);
  return lines.join("\n");
});

/** 将本地文件路径转为前端可访问的 URL（Tauri asset 协议） */
function fileUrl(path: string | null): string {
  return path ? convertFileSrc(path) : "";
}

/** FEAT-A：合并来源路径点击 —— emit 携带 path，由父组件负责打开系统文件夹。 */
function emitSourcePath(src: { id: number; path: string }, ev: MouseEvent) {
  ev.stopPropagation();
  emit("open-source-path", { id: src.id, path: src.path });
}
</script>

<template>
  <!-- ══════════════ FEAT-103：tile 变体（Canvas 极简） ══════════════
       裸封面 + 两行图注。没有卡片背景、没有阴影、没有内边距 ——
       层级由留白建立，不靠边框。但交互与 card 完全一致（多选/右键/键盘）。
       注意：勾选框仍保留（批量管理是刚需），只是把它从"卡片内左上"
       改成贴在封面上的圆点，避免裸封面上突然出现一个方块。 -->
  <article
    v-if="isTile"
    class="album-tile"
    :class="{ 'tile-selected': selected, 'tile-manage': selectMode, 'tile-mosaic': mosaic }"
    :tabindex="0"
    role="button"
    :aria-pressed="selectMode ? selected : undefined"
    :title="tileTitle"
    @click="emit('click')"
    @contextmenu="(e) => emit('contextmenu', e)"
    @keydown.enter.space.prevent="emit('click')"
  >
    <div class="tile-cover">
      <img v-if="album.cover_path" :src="fileUrl(album.cover_path)" alt="" loading="lazy" decoding="async" />
      <div v-else class="tile-placeholder">📷</div>

      <!-- 已入库角标：从 pill 降为纯文字（封面本身就是底色，不需要 pill 兜底） -->
      <span
        v-if="!isScanned"
        class="tile-badge"
        title="尚未扫描入库，可在详情页执行综合扫描"
      >未入库</span>

      <!-- FEAT-105：拍摄时间移到封面**左下角**，叠在封面上。
           为什么从图注里拿出来：图注在封面下方，位置随卡片排列跳动，
           扫视时找不到；而时间叠在封面左下角，每张卡片自己带着自己的时间，
           眼睛只需要扫一条水平线。参考图也是这个位置。
           底下加一层 scrim 保证浅色封面上的白字可读。 -->
      <span v-if="album.shoot_time" class="tile-time">{{ tileTime }}</span>

      <!-- 勾选圆点：管理模式下常驻；否则仅选中时显示 -->
      <span
        v-if="selectMode || selected"
        class="tile-check"
        :class="{ on: selected }"
        role="checkbox"
        :aria-checked="selected"
        :aria-label="`勾选 ${album.name}`"
        @click.stop="emit('toggle-select', $event)"
      >{{ selected ? "✓" : "" }}</span>
    </div>

    <div class="tile-cap">
      <b class="tile-name">{{ album.name }}</b>
      <span class="tile-meta">{{ tileMeta }}</span>
    </div>
  </article>

  <!-- ══════════════ 原有 card 变体（保持不变） ══════════════ -->
  <article
    v-else
    class="album-card glass-surface"
    :class="{ 'card-selected': selected, 'card-manage': selectMode }"
    :tabindex="0"
    role="button"
    :aria-pressed="selectMode ? selected : undefined"
    @click="emit('click')"
    @contextmenu="(e) => emit('contextmenu', e)"
    @keydown.enter.space.prevent="emit('click')"
  >
    <!-- 勾选模式下显示勾选框 -->
    <div v-if="selectMode" class="card-checkbox" @click.stop="emit('toggle-select', $event)">
      <span :class="['checkmark', { checked: selected }]">✓</span>
    </div>

    <div class="card-cover">
      <img v-if="album.cover_path" :src="fileUrl(album.cover_path)" alt="封面" loading="lazy" />
      <div v-else class="cover-placeholder">📷</div>
      <!-- FEAT-036：已入库 / 未入库 标记 -->
      <span
        class="scan-badge"
        :class="isScanned ? 'scan-badge-in' : 'scan-badge-out'"
        :title="isScanned ? `该相册已扫描入库 ${album.scanned_photo_count} 张照片` : '该相册尚未扫描入库，建议在详情页执行综合扫描'"
      >
        {{ isScanned ? "✓ 已入库" : "○ 未入库" }}
      </span>
    </div>

    <div class="card-body">
      <h3 class="card-name" :title="album.name">{{ album.name }}</h3>

      <!-- 地点徽标（仅地点排序视图显示） -->
      <p v-if="showLocation" class="location-tag" :class="{ 'no-loc': !album.location }">
        📍 {{ album.location || "未知地点" }}
      </p>

      <p v-if="album.description" class="card-desc">{{ album.description }}</p>

      <p class="card-path">
        <span
          class="path-link"
          :title="`在文件资源管理器中打开：${album.path}`"
          @click.stop="emit('open-path', $event)"
        >📁 {{ album.path }}</span>
      </p>

      <!-- FEAT-A：合并来源（FEAT-A）
        当该相册历史上被合并过，列出所有源相册路径。
        每条路径可点击 → 在系统文件管理器中打开源文件夹（已删除的源相册不会跳转；只展示信息） -->
      <div v-if="album.merged_sources && album.merged_sources.length" class="card-merged">
        <div class="merged-head">
          <span class="merged-label">由 {{ album.merged_sources.length }} 个相册合并而来</span>
        </div>
        <ul class="merged-list">
          <li v-for="src in album.merged_sources" :key="src.id" class="merged-item" :title="`${src.name}（已合并）`">
            <span
              class="merged-path"
              :title="`在文件管理器中打开源文件夹：${src.path}`"
              @click.stop="emitSourcePath({ id: src.id, path: src.path }, $event)"
            >📁 {{ src.path }}</span>
          </li>
        </ul>
      </div>

      <div class="card-stats">
        <span class="stat-item">🖼️ {{ album.photo_count }} 张</span>
        <span class="stat-item">💾 {{ formatSize(album.size_bytes) }}</span>
        <span v-if="album.shoot_time" class="stat-item">📅 {{ album.shoot_time }}</span>
      </div>
    </div>
  </article>
</template>

<style scoped>
.album-card {
  position: relative;
  background: var(--card-bg, #fff);
  color: var(--text, #333);
  border-radius: var(--radius-card);
  overflow: hidden;
  box-shadow: var(--shadow-1);
  cursor: pointer;
  transition: transform var(--dur-2), box-shadow var(--dur-2);
}
.album-card:focus-visible {
  outline: none;
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-primary) 45%, transparent);
}

.album-card:hover {
  transform: translateY(-4px);
  box-shadow: 0 8px 20px rgba(0, 0, 0, 0.15);
}

.card-cover {
  position: relative;
  height: 160px;
  background: var(--panel-bg, var(--color-surface-2));
  display: flex;
  align-items: center;
  justify-content: center;
}

/* FEAT-036：已入库 / 未入库 徽标（封面右上角） */
.scan-badge {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 3;
  padding: 2px 8px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 600;
  backdrop-filter: blur(3px);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.2);
  pointer-events: none;
}
.scan-badge-in {
  background: var(--color-ok-solid, var(--color-ok-solid));
  color: #fff;
}
.scan-badge-out {
  /* 深灰实底压住卡片光泽层：白字 11px 需 ≥4.5:1（原 0.75 透底被新光泽层提亮到 4.43） */
  background: rgba(96, 88, 82, 0.88);
  color: #fff;
}

.card-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.cover-placeholder {
  font-size: 48px;
  opacity: 0.4;
}

.card-body {
  padding: 14px;
}

.card-name {
  margin: 0 0 6px;
  font-size: 16px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--text, var(--color-text));
}

.card-desc {
  margin: 0 0 8px;
  font-size: 13px;
  color: var(--sub-text, var(--color-text-2));
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  line-height: 1.5;
}

.card-path {
  margin: 0;
  font-size: 12px;
  color: var(--muted, #999);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.path-link {
  display: inline-block;
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  vertical-align: bottom;
  color: var(--color-link);
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 2px;
}

.path-link:hover {
  color: var(--color-primary-hover);
}

.card-stats {
  display: flex;
  gap: 14px;
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--card-border, #f0f0f0);
}

.stat-item {
  font-size: 12px;
  color: var(--sub-text, #666);
}

/* 勾选模式 */
.card-checkbox {
  position: absolute;
  top: 10px;
  left: 10px;
  z-index: 5;
  display: flex;
  align-items: center;
  justify-content: center;
}

.checkmark {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 2px solid var(--input-border, #ccc);
  background: var(--card-bg, rgba(255, 255, 255, 0.9));
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 13px;
  color: transparent;
  cursor: pointer;
  transition: all var(--dur-2);
}

.checkmark.checked {
  background: var(--color-primary);
  border-color: var(--color-primary);
  color: var(--color-on-primary);
}

.card-selected {
  border: 2px solid var(--color-primary);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--color-primary) 20%, transparent);
}

/* 勾选管理模式：禁用悬停上浮，光标为默认 */
.card-manage {
  cursor: default;
}

.card-manage:hover {
  transform: none;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.08);
}

/* 地点徽标 */
.location-tag {
  display: inline-block;
  margin: 0 0 8px;
  padding: 2px 10px;
  font-size: 12px;
  color: var(--color-primary-ink);
  background: #eef3ff;
  border-radius: 12px;
}

.location-tag.no-loc {
  color: var(--muted, #999);
  background: var(--panel-bg, var(--color-surface-2));
}

/* FEAT-A：合并来源列表 */
.card-merged {
  margin: 6px 0 4px;
  padding: 6px 8px;
  background: rgba(255, 153, 0, 0.06);
  border: 1px solid rgba(255, 153, 0, 0.18);
  border-radius: 6px;
}
.merged-head {
  font-size: 11px;
  font-weight: 600;
  color: #9a5b00;
  margin-bottom: 4px;
  display: flex;
  align-items: center;
  gap: 4px;
}
.merged-label {
  letter-spacing: 0.2px;
}
.merged-list {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-height: 88px;
  overflow-y: auto;
}
.merged-item {
  font-size: 11px;
  line-height: 1.45;
}
.merged-path {
  display: inline-block;
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  vertical-align: bottom;
  color: #a06700;
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 2px;
}
.merged-path:hover {
  color: #6f4800;
}
/* ══════════════════════════════════════════════════════════════════════════
   FEAT-103：tile 变体 —— 裸封面 + 两行图注
   ──────────────────────────────────────────────────────────────────────────
   为什么没有卡片容器：
     深色底上，卡片边框是一圈 1px 灰线。10 张卡并按时，这些线彼此平行、
     等距、无层级差异，视觉上形成"网格噪声"—— 用户看到的是"一堆框"，
     而不是"一堆照片"。裸封面时照片自己的边缘就是边界，且每张照片的
     明暗不同，天然产生分隔。
   唯一保留的"框"是 1px 内描边：不是装饰，是防止封面亮部与页面背景糊在
   一起的必要边界（见 tokens.css 的 --cover-inset）。
   ══════════════════════════════════════════════════════════════════════════ */
.album-tile {
  display: flex;
  flex-direction: column;
  cursor: pointer;
  /* FEAT-105：Rainmeter 皮肤化 —— 瓦片也是一块磨砂玻璃板。
     但**封面本身不透明**（照片是内容，不能透），所以玻璃效果落在：
       · 封面下方的图注区（真正的玻璃，能透出壁纸）
       · 板边缘的亮边与投影（让每块读起来是"浮在壁纸上的一块板"）
     封面保持不透明是必须的：照片透出壁纸会变成鬼影。 */
  transition:
    filter var(--dur-2) var(--ease-out),
    transform var(--dur-1) var(--ease-out);
}

.tile-cover {
  position: relative;
  width: 100%;
  /* ⚠️ 不用 aspect-ratio。实测（Playwright 截图）它在两种情况下都会塔成 0：
       ① 封面为空（占位符绝对定位，不撑高）
       ② 有 <img> 但 img 用 height:100% —— 图片高度依赖父级，
          父级高度又依赖图片（aspect-ratio 需要至少一个已定尺寸的子项）
     改用经典的 padding-bottom 百分比（16/10 = 62.5%）：
       百分比宽度在**任何**包含块里都能解算，不依赖子项，也不依赖 flex 算法。
     代价是不能直接用 var 参数化比例（百分比 padding 无法读自定义属性），
     但这个组件只需要一种比例，直接写死更可靠。 */
  height: 0;
  padding-bottom: 62.5%;
  flex-shrink: 0;
  border-radius: var(--r-cover);
  overflow: hidden;
  background: rgba(255, 255, 255, 0.04);
  box-shadow: var(--cover-inset), var(--cover-topline);
  transition: box-shadow var(--dur-2) var(--ease-out);
}

.tile-cover img {
  /* 与 height:0 + padding-bottom 的占位盒配合：
     图片必须绝对定位铺满，否则它会参与流布局，把 padding-bottom 撑出的
     空间再顶高一层（变成双重高度）。 */
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.tile-placeholder {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  font-size: 22px;
  opacity: 0.32;
}

/* 角标：纯文字 + 文字阴影（不再用 pill）。
   裸封面上叠一个半透明药丸会变成"贴纸"，而 9px 大写文字压在照片上
   已经足够可读 —— 因为照片底部本来就有暗角。 */
.tile-badge {
  position: absolute;
  top: 9px;
  left: 9px;
  font-size: 9px;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  font-weight: var(--fw-strong);
  color: rgba(255, 255, 255, 0.86);
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.6);
  pointer-events: none;
}

/* FEAT-105：拍摄时间标签（封面左下角）
   —— 叠在封面上，位置固定，不再随图注跳动。
   一层从下往上的黑 scrim + 白色字：浅色封面（雪、天空）上也能读。
   不用 backdrop-filter —— 一屏几十张，每张都模糊背景会拖垮滚动。 */
.tile-time {
  position: absolute;
  left: 0;
  bottom: 0;
  padding: 10px 9px 7px;
  font-size: 10.5px;
  font-weight: var(--fw-em);
  letter-spacing: 0.02em;
  color: rgba(255, 255, 255, 0.92);
  background: linear-gradient(to top, rgba(0, 0, 0, 0.62), rgba(0, 0, 0, 0));
  pointer-events: none;
}

/* 勾选圆点：22px，与封面的 5px 圆角形成对比但不突兀 */
.tile-check {
  position: absolute;
  top: 8px;
  right: 8px;
  width: 22px;
  height: 22px;
  display: grid;
  place-items: center;
  border-radius: 50%;
  font-size: 11px;
  font-weight: 700;
  color: #fff;
  background: rgba(10, 13, 19, 0.42);
  box-shadow:
    inset 0 0 0 1.5px rgba(255, 255, 255, 0.75),
    0 2px 6px rgba(0, 0, 0, 0.35);
  backdrop-filter: blur(6px);
  transition:
    background var(--dur-2) var(--ease-out),
    box-shadow var(--dur-2) var(--ease-out);
}
.tile-check.on {
  background: var(--c-accent);
  color: #1a1206;
  box-shadow: inset 0 0 0 1.5px rgba(255, 255, 255, 0.9);
}

/* 选中：封面外环 + 图注提亮（不模糊、不对整个卡片做滤镜） */
.tile-selected .tile-cover {
  box-shadow:
    inset 0 0 0 1px rgba(255, 255, 255, 0.16),
    0 0 0 2px var(--c-accent);
}

.tile-cap {
  padding-top: 11px;
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
}

/* FEAT-105：小瓦片的图注区也做成玻璃（封面下那一条）。
   参考图里每块面板的下方都有一条独立的磨砂条（标签 + 微型图表）。 */
.album-tile:not(.tile-mosaic) .tile-cap {
  margin: 0 -1px -1px;
  padding: 9px 10px 8px;
  border-radius: 0 0 var(--r-cover) var(--r-cover);
  background-color: var(--panel-glass-fill);
  -webkit-backdrop-filter: blur(var(--panel-glass-blur)) saturate(var(--panel-glass-saturate));
  backdrop-filter: blur(var(--panel-glass-blur)) saturate(var(--panel-glass-saturate));
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.22),
    inset 0 -1px 0 rgba(0, 0, 0, 0.30);
}

/* FEAT-105：mosaic 变体（跨格的大瓦片）
   默认 .tile-cover 用 padding-bottom:62.5% 定一个固定宽高比，
   在跨 2×2 的大格子里只会占住上方一块，底下留白 —— 看起来像"图没铺满"。
   这里在大格子里改成：封面填满剩余高度（flex:1），
   图注压在封面**底部**（绝对定位 + 渐变遮罩），像海报的落款。 */
.album-tile.tile-mosaic {
  position: relative; /* 图注的 absolute 定位锚 —— 封面是 relative 但它是兄弟节点，
                          所以图注需要卡片自己当锚，否则会相对整行定位（宽度变 1440px） */
  display: flex;
  flex-direction: column;
  height: 100%;
}
.album-tile.tile-mosaic .tile-cover {
  /* 覆盖 62.5% 的 padding-bottom 方案：改用 flex 填满 */
  height: auto;
  padding-bottom: 0;
  flex: 1;
  min-height: 0;
}
.album-tile.tile-mosaic .tile-cap {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 24px 11px 9px;
  /* 遮罩是**深色**渐变，所以图注文字必须是**浅色**（深色底上白字）。
     浅色预设下这块板底很亮，深色遮罩 + 深色字会糊成一片 ——
     所以浅色下改用浅色遮罩 + 深色字。 */
  background: linear-gradient(to top, rgba(0, 0, 0, 0.74), rgba(0, 0, 0, 0));
  pointer-events: none;
}
.album-tile.tile-mosaic .tile-name {
  color: #fff;
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.5);
}
.album-tile.tile-mosaic .tile-meta {
  color: rgba(255, 255, 255, 0.84);
}
/* 浅色预设：板底亮 → 遮罩与文字都反过来 */
body:not(.theme-dark) .album-tile.tile-mosaic .tile-cap {
  background: linear-gradient(to top, rgba(255, 255, 255, 0.88), rgba(255, 255, 255, 0));
}
body:not(.theme-dark) .album-tile.tile-mosaic .tile-name {
  color: #17161a;
  text-shadow: none;
}
body:not(.theme-dark) .album-tile.tile-mosaic .tile-meta {
  color: rgba(23, 22, 26, 0.7);
}
/* 浅色下封面左下角的时间标签也得反过来（它原本叠在深色遮罩上） */
body:not(.theme-dark) .album-tile.tile-mosaic .tile-time {
  display: none;
}
/* 大格子里不再显示封面左下角的独立时间标签（时间已在图注里） */
.album-tile.tile-mosaic .tile-time {
  display: none;
}

.tile-name {
  font-size: var(--fs-title);
  font-weight: var(--fw-em);
  letter-spacing: var(--ls-title);
  color: var(--c-fg);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.tile-meta {
  font-size: var(--fs-meta);
  color: var(--c-fg-2);
  letter-spacing: var(--ls-meta);
  white-space: nowrap;
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
}

.album-tile:hover .tile-cover {
  box-shadow:
    inset 0 0 0 1px rgba(255, 255, 255, 0.16),
    var(--cover-topline);
}
.album-tile:hover {
  filter: brightness(1.06);
}

.album-tile:active {
  transform: scale(0.985);
  transition-duration: var(--dur-1);
}

.album-tile:focus-visible {
  outline: none;
}
.album-tile:focus-visible .tile-cover {
  box-shadow:
    inset 0 0 0 1px rgba(255, 255, 255, 0.16),
    0 0 0 1.5px var(--c-accent),
    0 0 0 3.5px transparent;
}

/* 浅色模式：占位底与描边换成暗色系 */
body:not(.theme-dark) .tile-cover {
  background: rgba(23, 22, 26, 0.05);
}

/* 角标在浅色下的可读性：
   角标原本是"白字 + 黑文字阴影"，专为"压在照片上"设计 —— 照片总有暗部。
   但**没有封面的相册**（新建/封面文件被移走）底是浅灰，白字直接消失。
   这里给无封面场景一个实底：用 backdrop-filter 取不可靠（浅灰上无意义），
   直接给一个半透黑药丸 + 纯白字。 */
body:not(.theme-dark) .tile-cover:not(:has(img)) .tile-badge {
  color: #fff;
  background: rgba(23, 22, 26, 0.55);
  padding: 2px 7px;
  border-radius: 4px;
  text-shadow: none;
}
/* 深色下同样处理：深灰占位底上白字尚可读，但加一层薄底更稳 */
body.theme-dark .tile-cover:not(:has(img)) .tile-badge {
  background: rgba(255, 255, 255, 0.12);
  padding: 2px 7px;
  border-radius: 4px;
  color: rgba(255, 255, 255, 0.92);
}
</style>
