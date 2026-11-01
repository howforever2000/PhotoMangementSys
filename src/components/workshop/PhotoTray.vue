<script setup lang="ts">
/**
 * 素材托盘（FEAT-150）
 * ===================================================================
 * 工坊里所有工具的共同输入都是一种东西：**一组照片**。所以托盘不归某个工具，
 * 它归页面 —— 换工具时素材不动，这是「工具 × 输入 → 输出」这套结构能成立的前提。
 *
 * 为什么不做成"每个工具自己选照片"：那样压缩选一次、OCR 又选一次，
 * 用户从相册发过来的一组照片到了第二个工具就失效了；而且每个工具都要
 * 复写一遍选择器，越写越像。
 *
 * 纯展示组件：不 invoke、不弹窗，增删都 emit 给 Workshop.vue。
 */
import { ref, watch } from "vue";
import { resolvePhotoSrc } from "../../utils/photoSrc";
import { baseName } from "../../utils/pathParts";

const props = withDefaults(
  defineProps<{
    paths: string[];
    /** 清空按钮是否显示（工具运行中时藏在父级更好，这里只做展示） */
    showClear?: boolean;
  }>(),
  { showClear: true },
);

const emit = defineEmits<{
  add: [];
  remove: [path: string];
  clear: [];
}>();

/**
 * 缩略图地址：HEIC/HEIF 必须走后端预览缓存（WebView2 不认识），
 * 其余格式 `resolvePhotoSrc` 直接返回 asset URL，等于零开销。
 */
const srcMap = ref<Record<string, string>>({});

async function ensureSrc(path: string) {
  if (srcMap.value[path] !== undefined) return;
  try {
    const url = await resolvePhotoSrc(path);
    srcMap.value = { ...srcMap.value, [path]: url };
  } catch {
    // 解析失败留空 → 模板回落成占位块，不阻断整条托盘
    srcMap.value = { ...srcMap.value, [path]: "" };
  }
}

watch(
  () => props.paths,
  (list) => list.forEach((p) => void ensureSrc(p)),
  { immediate: true },
);
</script>

<template>
  <section class="tray">
    <div class="tray-head">
      <span class="tray-label">素材</span>
      <span class="tray-count">{{ paths.length }} 张</span>
      <button
        v-if="showClear && paths.length"
        class="tray-clear"
        type="button"
        @click="emit('clear')"
      >
        清空
      </button>
    </div>

    <!-- 空态：说清「照片从哪来」，而不是只显示一块空框 -->
    <div v-if="!paths.length" class="tray-empty">
      <span>还没有照片。从相册里「发送到创意工坊」，或直接添加：</span>
      <button class="tray-add-inline" type="button" @click="emit('add')">＋ 添加照片</button>
    </div>

    <div v-else class="tray-strip">
      <figure v-for="p in paths" :key="p" class="tray-item" :title="p">
        <img v-if="srcMap[p]" :src="srcMap[p]" :alt="baseName(p)" loading="lazy" />
        <span v-else class="tray-thumb-fallback" aria-hidden="true" />
        <figcaption>{{ baseName(p) }}</figcaption>
        <button class="tray-x" type="button" title="从素材里移除" @click="emit('remove', p)">
          ✕
        </button>
      </figure>
      <button class="tray-add" type="button" title="继续添加照片" @click="emit('add')">＋</button>
    </div>
  </section>
</template>

<style scoped>
.tray {
  margin-bottom: 14px;
}

.tray-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  height: 18px;
}

/* 分组小标：与「处理 / 创作」分区标同一套字阶，让页面只有一种小标语言 */
.tray-label {
  font-size: var(--fs-label);
  letter-spacing: 0.14em;
  color: var(--fg-2);
}

.tray-count {
  font-size: var(--fs-meta);
  color: var(--fg-3, var(--fg-2));
  font-variant-numeric: tabular-nums;
}

.tray-sp {
  flex: 1;
}

/* 分隔点：清空与计数之间只靠间距会读成一串，加一个小点分开 */
.tray-clear::before {
  content: "·";
  margin-right: 4px;
  color: var(--fg-2);
}
.tray-clear {
  border: none;
  background: none;
  padding: 2px 4px;
  /* 紧跟在计数后：它们俩是一件事（"这几张 / 全不要了"），
     推到行尾会变成孤零零一个词挂在 1000px 外 */
  margin-left: 2px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  cursor: pointer;
  border-radius: 4px;
  transition: color var(--dur-2) var(--ease-out);
}
.tray-clear:hover {
  color: var(--c-danger);
}

/* 空态：一层虚线底 + 一句话，不做成一个大空框（空框会显得页面坏了） */
.tray-empty {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 14px 16px;
  border-radius: var(--r-ui);
  border: 1px dashed var(--c-rule);
  font-size: var(--fs-body);
  color: var(--fg-2);
}

.tray-add-inline {
  border: none;
  background: none;
  padding: 0;
  font-size: var(--fs-body);
  color: var(--c-accent);
  cursor: pointer;
}
.tray-add-inline:hover {
  text-decoration: underline;
}

.tray-strip {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  overflow-x: auto;
  padding-bottom: 4px;
  scrollbar-width: thin;
}

.tray-item {
  position: relative;
  flex: 0 0 auto;
  width: 72px;
  margin: 0;
}

.tray-item img,
.tray-thumb-fallback {
  display: block;
  width: 72px;
  height: 72px;
  object-fit: cover;
  border-radius: var(--r-cover);
  background: var(--hover, rgba(120, 120, 130, 0.12));
  box-shadow: inset 0 0 0 1px var(--c-rule);
}

/* 文件名放在缩略图下面一行，超出省略：用户在托盘里靠它认图，缩略图太小认不准 */
.tray-item figcaption {
  margin-top: 4px;
  font-size: 10px;
  line-height: 1.3;
  color: var(--fg-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tray-x {
  position: absolute;
  top: -5px;
  right: -5px;
  width: 17px;
  height: 17px;
  display: grid;
  place-items: center;
  padding: 0;
  border-radius: 50%;
  border: 1px solid var(--c-rule);
  background: var(--c-bg, #14131a);
  color: var(--fg-2);
  font-size: 9px;
  cursor: pointer;
  opacity: 0;
  transition: opacity var(--dur-2) var(--ease-out), color var(--dur-2) var(--ease-out);
}
.tray-item:hover .tray-x,
.tray-x:focus-visible {
  opacity: 1;
}
.tray-x:hover {
  color: var(--c-danger);
}

.tray-add {
  flex: 0 0 auto;
  width: 72px;
  height: 72px;
  border-radius: var(--r-cover);
  border: 1px dashed var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: 18px;
  cursor: pointer;
  transition: color var(--dur-2) var(--ease-out), border-color var(--dur-2) var(--ease-out);
}
.tray-add:hover {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 45%, transparent);
}
</style>
