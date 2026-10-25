<script setup lang="ts">
/**
 * AppHeader —— 大组件内部导航条（FEAT-106 重做）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 结构
 * ──────────────────────────────────────────────────────────────────────────
 *   [← 主页] [图标][组件名 · 读数] │ 分区1 分区2 分区3 …              [搜索] [头像]
 *
 * 这条栏**不跨大组件**。它上面每一项都属于"当前这个大组件"内部的功能分区。
 * 跨大组件的入口只有一个地方：Home 的四张卡（点「← 主页」回去换）。
 *
 * 演进记录（很重要，避免再走回头路）：
 *   第 1 版 NAV = 相册/时间线/人物/地点/语义 —— 后四项是 /smart 内部的 tab，
 *            却和"相册"平起平坐，层级错位。
 *   第 2 版把导航整条删掉，只留「← 主页 + 模块名」—— 矫枉过正：
 *            大组件内部确实需要一条常驻的分区导航。
 *   第 3 版（FEAT-105）栏内 = **本组件内部的分区**，由每个页面通过 `tabs` 传入。
 *   第 4 版（现在，FEAT-106）：版式重做为**一块独立的磨砂玻璃板**，
 *            悬在页面底之上、与内容区有明确的厚度差（见下方玻璃说明）。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 玻璃：这条栏是**浮在内容之上**的一层，所以用 --glass-*（浮层玻璃）
 * ──────────────────────────────────────────────────────────────────────────
 * 语义约束（比数值更重要）：--glass-* 只允许被「浮在内容之上」的层消费。
 * 面板/卡片用 --panel-glass-*（墙板玻璃），两者刻意不合并：
 * 一个是"浮起来的工具条"，一个是"墙上的板"。
 *
 * 模糊度要克制（18px）：顶栏后面滚过的是正文，糊到看不清就失去导航可读性。
 * ══════════════════════════════════════════════════════════════════════════
 */
import { computed } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAuthStore } from "../../stores/auth";

export interface HeaderTab {
  /** 稳定 key（状态型分区用） */
  key?: string;
  label: string;
  /** 计数徽标 */
  count?: number | string;
  /** 跳转路由（路由型分区用，如 /scan/test） */
  to?: string;
  /** 置灰但可见（"有这块功能，当前条件不可用"） */
  disabled?: boolean;
  /** 置灰原因，写进 title —— 灰掉必须讲原因 */
  why?: string;
}

const props = withDefaults(
  defineProps<{
    /** 大组件名（如「相册管理」） */
    title?: string;
    /** 组件 emoji */
    icon?: string;
    /** 一行读数 */
    meta?: string;
    /** 组件内部的分区（不传则不渲染导航区，用于登录页等独立壳） */
    tabs?: HeaderTab[];
    /** 当前激活分区的 key（状态型） */
    active?: string;
    /** 紧凑高度 */
    compact?: boolean;
  }>(),
  { title: "", icon: "", meta: "", tabs: () => [], active: "", compact: false },
);

const emit = defineEmits<{ pick: [key: string] }>();

const router = useRouter();
const route = useRoute();
const auth = useAuthStore();
const username = computed(() => auth.user?.username ?? "");

/**
 * 路由型 tab 是否处于选中态。
 *
 * 为什么不能直接写 `$route.path === t.to`：
 *   内容分类与人物、地点都住在 /smart 上，靠 `?tab=` 区分 ——
 *   它们的 `to` 是 "/smart?tab=category" 这种**带 query 的字符串**，
 *   而 `$route.path` 永远只是 "/smart"，两者永远不会相等，
 *   结果就是这两个 tab 永远不高亮（用户不知道自己在哪一块）。
 *
 * 现在：分别比较 path 与 tab query；
 *   缺省 tab 视为该页的**第一个** tab（/smart 默认就是内容分类）——
 *   否则从 Home 直接进 /smart 时导航栏会一个高亮都没有。
 */
function isTabOn(to?: string): boolean {
  if (!to) return false;
  const target = router.resolve(to);
  if (target.path !== route.path) return false;
  const want = target.query.tab;
  const cur = route.query.tab;
  if (want === undefined) return cur === undefined || cur === "";
  // /smart 不带 tab 时默认落在内容分类上
  if (cur === undefined || cur === "") return want === "category";
  return cur === want;
}
</script>

<template>
  <header class="mh" :class="{ 'mh-compact': compact }">
    <div class="mh-inner">
      <!-- 唯一出口：回 Home 换大组件（这不是导航项，是"退出本组件"） -->
      <button class="mh-back" type="button" title="回到主页（切换其他组件）" @click="router.push('/home')">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path d="M15 5l-7 7 7 7" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        <span>主页</span>
      </button>

      <!-- 组件名：图标 + 名字 + 读数，三者读成一块 -->
      <div class="mh-name">
        <span v-if="icon" class="mh-emo" aria-hidden="true">{{ icon }}</span>
        <span class="mh-title">{{ title || "相册" }}</span>
        <span v-if="meta" class="mh-meta">{{ meta }}</span>
      </div>

      <!-- 组件内部分区导航：每一项都属于本组件 -->
      <nav v-if="tabs.length" class="mh-nav" aria-label="组件内分区">
        <template v-for="t in tabs" :key="t.key || t.label">
          <RouterLink v-if="t.to && !t.disabled" :to="t.to" class="mh-nav-item" :class="{ on: isTabOn(t.to) }">
            {{ t.label }}
            <span v-if="t.count !== undefined && t.count !== ''" class="mh-nav-n">{{ t.count }}</span>
          </RouterLink>
          <button
            v-else
            type="button"
            class="mh-nav-item"
            :class="{ on: !!t.key && t.key === props.active, off: t.disabled }"
            :disabled="t.disabled"
            :title="t.disabled ? t.why : undefined"
            @click="!t.disabled && t.key && emit('pick', t.key)"
          >
            {{ t.label }}
            <span v-if="t.count !== undefined && t.count !== ''" class="mh-nav-n">{{ t.count }}</span>
          </button>
        </template>
      </nav>

      <div class="mh-sp"></div>

      <button class="mh-search" type="button" title="搜索照片（⌘K）" @click="router.push('/search')">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <circle cx="11" cy="11" r="6.5" stroke="currentColor" stroke-width="2" />
          <path d="M16 16l4.5 4.5" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
        </svg>
        <span class="mh-search-txt">搜索照片</span>
      </button>

      <button
        v-if="username"
        class="mh-user"
        type="button"
        :title="`${username} · 账户设置`"
        @click="router.push('/home')"
      >
        <span aria-hidden="true">👤</span>
      </button>
    </div>
  </header>
</template>

<style scoped>
/* ══════════════════════════════════════════════════════════════════════════
   顶栏 = 一块浮在页面之上的磨砂玻璃板
   ──────────────────────────────────────────────────────────────────────────
   关键认知（FEAT-106）：顶栏**不是**"页面底色 + 一条线"，而是一块**板**。
   之前它是页面底色 + 发丝线，内容从下方滚过时两者糊在一起 —— 读不出厚度。
   现在板有明确的边界：
     · 板体（玻璃填充 + 模糊）
     · 板的底缘（一条暗壁 = 厚度读数）
     · 板下的柔光（悬浮感）
   板与内容之间还留了 10px 的空气（--mh-gap），让内容从板下穿过而不是贴着。
   ══════════════════════════════════════════════════════════════════════════ */
.mh {
  position: sticky;
  top: 0;
  z-index: var(--z-sticky);
  padding: 10px var(--sp-8) 0;
  /* 板体本身不画背景，只给下面那层 .mh-inner 画玻璃 —— 这样板的内边距
     （上下留白）也是玻璃的一部分，而不是一块贴在玻璃上的透明区。 */
  background: transparent;
}

.mh-inner {
  display: flex;
  align-items: center;
  gap: 14px;
  height: 52px;
  padding: 0 16px 0 12px;
  border-radius: var(--panel-glass-radius);
  /* FEAT-108：改用浮层专用令牌 --float-*。
     旧写法 `var(--glass-blur)` 期望它是完整 filter 值，
     但 main.css 把同名令牌定义成长度并后加载覆盖 → `18px saturate(...)`
     是无效值，**整条 backdrop-filter 被丢弃**（实测 computed = "none"）。
     边框同理：`border: var(--glass-border)` 拿到纯颜色，宽度默认成 3px。 */
  background-color: var(--float-fill);
  /* FEAT-108：浮层玻璃 = 真实边缘折射 + 轻模糊。
     `url(#liquid-lens)` 引用 GlassFilters.vue（挂在 App 根）里定义的 SVG 滤镜，
     它用 feDisplacementMap 把**背后的内容**沿噪声场弯折 —— 这才是“厚度”的来源，
     而不是靠 1px 高光假装。

     ⚠️ 滤镜顺序：url() 必须写在 blur() **前面**。
     反过来的话 Chromium 会先模糊再把糊掉的图去位移，弯折完全不明显（实测）。

     ⚠️ 只给浮层（顶栏/弹窗/菜单）。一屏几十张卡片每张都跑位移 = 直接掉帧。 */
  -webkit-backdrop-filter: url(#liquid-lens) blur(var(--float-blur)) saturate(var(--float-saturate));
  backdrop-filter: url(#liquid-lens) blur(var(--float-blur)) saturate(var(--float-saturate));
  border: 1px solid var(--float-border);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.30),
    inset 0 -1px 0 rgba(0, 0, 0, 0.34),
    0 10px 26px -16px rgba(0, 0, 0, 0.72);
  /* 板顶镜面高光（与 .panel-glass 同一道光，方向固定左上 135°） */
  position: relative;
  overflow: hidden;
}

.mh-inner::before {
  content: "";
  position: absolute;
  inset: 0;
  border-radius: inherit;
  pointer-events: none;
  background: linear-gradient(
    135deg,
    rgba(255, 255, 255, 0.13) 0%,
    rgba(255, 255, 255, 0.035) 20%,
    rgba(255, 255, 255, 0) 44%
  );
}

/* 紧凑模式：板矮 8px（内容多的页面用） */
.mh-compact .mh-inner {
  height: 44px;
}

/* —— 返回主页 —— */
.mh-back {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 26px;
  padding: 0 10px 0 8px;
  border-radius: 7px;
  border: none;
  background: none;
  box-shadow: inset 0 0 0 1px var(--c-rule);
  font-size: 11.5px;
  color: var(--c-fg-2);
  cursor: pointer;
  flex-shrink: 0;
  transition: color var(--dur-2) var(--ease-out), box-shadow var(--dur-2) var(--ease-out);
}
.mh-back:hover {
  color: var(--c-fg);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 45%, transparent);
}

/* —— 组件名 —— */
.mh-name {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
  flex-shrink: 0;
}
.mh-emo {
  font-size: 15px;
  align-self: center;
  /* emoji 降浓：四个高饱和色块是整屏最跳的元素，
     退回"素色面板上的图标"而不是"贴纸" */
  filter: saturate(0.78) brightness(1.02);
}
.mh-title {
  font-size: var(--fs-head);
  font-weight: var(--fw-strong);
  letter-spacing: -0.018em;
  color: var(--c-fg);
  white-space: nowrap;
}
.mh-meta {
  font-size: var(--fs-meta);
  color: var(--c-fg-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* —— 组件内部分区导航：与组件名之间一条发丝竖线分隔，
       让"我是谁"和"我能去哪"读成两件事 —— */
.mh-nav {
  display: flex;
  align-items: center;
  gap: 2px;
  min-width: 0;
  padding-left: 14px;
  border-left: 1px solid var(--c-rule);
  height: 22px;
}
.mh-nav-item {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 11px;
  border: none;
  background: none;
  border-radius: 7px;
  font-size: var(--fs-body);
  letter-spacing: var(--ls-body);
  color: var(--c-fg-2);
  text-decoration: none;
  white-space: nowrap;
  cursor: pointer;
  transition: color var(--dur-2) var(--ease-out), background var(--dur-2) var(--ease-out);
}
.mh-nav-item:hover {
  color: var(--c-fg);
  background: var(--hover, rgba(255, 255, 255, 0.055));
}
.mh-nav-item.on {
  color: var(--c-fg);
  font-weight: var(--fw-em);
  background: var(--hover, rgba(255, 255, 255, 0.055));
}
/* 选中下划线贴在文字下方，比"整块高亮"轻，与全站的 tab 语言一致 */
.mh-nav-item.on::after {
  content: "";
  position: absolute;
  left: 11px;
  right: 11px;
  bottom: 3px;
  height: 1.5px;
  border-radius: 2px;
  background: var(--c-accent);
}
.mh-nav-item.off {
  /* 置灰项用 --c-fg-2 + opacity .75：既弱于可点项（--c-fg），又保底可读。
     用户得先看清这个词才知道这块功能叫什么，再去 hover 读 why。 */
  color: var(--c-fg-2);
  opacity: 0.75;
  cursor: not-allowed;
}
.mh-nav-item.off:hover {
  background: none;
}
.mh-nav-n {
  font-size: 10px;
  line-height: 1;
  padding: 2px 5px;
  border-radius: 4px;
  color: var(--c-fg-2);
  background: var(--hover, rgba(255, 255, 255, 0.07));
  font-variant-numeric: tabular-nums;
}
.mh-nav-item.on .mh-nav-n {
  color: var(--c-accent-bright, var(--c-accent));
  background: color-mix(in srgb, var(--c-accent) 14%, transparent);
}

.mh-sp {
  flex: 1;
}

/* —— 搜索：栏里唯一的独立控件（玻璃药丸） —— */
.mh-search {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 30px;
  padding: 0 14px 0 11px;
  border-radius: var(--r-pill);
  border: 1px solid var(--c-rule);
  background: var(--hover, rgba(255, 255, 255, 0.05));
  color: var(--c-fg-2);
  cursor: pointer;
  flex-shrink: 0;
  transition: color var(--dur-2) var(--ease-out), border-color var(--dur-2) var(--ease-out);
}
.mh-search-txt {
  color: inherit;
}
.mh-search:hover {
  color: var(--c-fg);
  border-color: color-mix(in srgb, var(--c-accent) 40%, transparent);
}

/* —— 用户 —— */
.mh-user {
  width: 28px;
  height: 28px;
  border-radius: 50%;
  display: grid;
  place-items: center;
  padding: 0;
  border: none;
  background: linear-gradient(140deg, #b98a45, #6d4a2a);
  cursor: pointer;
  flex-shrink: 0;
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.3),
    0 2px 8px rgba(0, 0, 0, 0.35);
}
.mh-user span {
  font-size: 12px;
  opacity: 0.85;
}

/* 窄窗：分区导航横向滚动而不是换行（换行会把顶栏撑成两行，吸顶高度不稳定） */
@media (max-width: 1180px) {
  .mh {
    padding: 8px var(--sp-5) 0;
  }
  .mh-inner {
    gap: 10px;
  }
  .mh-meta {
    display: none;
  }
  .mh-nav {
    padding-left: 10px;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .mh-nav::-webkit-scrollbar {
    display: none;
  }
}
@media (max-width: 860px) {
  .mh-title {
    font-size: var(--fs-title);
  }
  .mh-search-txt {
    display: none;
  }
  .mh-search {
    padding: 0 10px;
  }
  .mh-nav-item {
    padding: 0 7px;
  }
}
</style>
