<script setup lang="ts">
/**
 * AppHeader —— 全站统一顶栏（FEAT-103 · P5）
 *
 * 目的：消掉 11 个页面里各自手写的「← 返回主页」按钮，并给所有页面
 * 一个一致的导航锚点。此前每个页面都在重复实现同一件事，且文案不一
 * （"返回主页" / "← 返回" / "返回图片扫描"），用户无法建立空间感。
 *
 * 形态选择：**顶栏常驻**，不用侧边栏。
 *   相册产品的核心行为是横向浏览大量封面，纵向侧栏会永久占用 200px
 *   的封面空间；顶栏 56px 换来的封装比更高。低频入口（归档/隐藏/扫描/
 *   工坊/设置）走头像菜单。
 *
 * 玻璃：整个应用里玻璃只出现在浮层。这里顶栏**不是**玻璃 ——
 *   它是结构性元素，用纯色 + 底部发丝线。滚动时也不变成玻璃
 *   （滚动玻璃会让页面内容从半透明的导航下透过去，是"好看但读不清"的典型）。
 */
import { computed } from "vue";
import { useRouter, useRoute } from "vue-router";
import { useAuthStore } from "../../stores/auth";

withDefaults(
  defineProps<{
    /** 页面标题（当前页） */
    title?: string;
    /** 是否显示主导航（登录后才显示） */
    showNav?: boolean;
    /** 紧凑高度（副页面用） */
    compact?: boolean;
  }>(),
  { title: "", showNav: true, compact: false },
);

const router = useRouter();
const route = useRoute();
const auth = useAuthStore();

/** 主导航项：只放 5 个高频入口。
 *  为什么是这 5 个：相册/时间线/人物/地点/语义 覆盖了"找照片"的全部主路径。
 *  归档、隐藏、扫描、工坊、设置都是低频（每周一次以下），收进头像菜单。
 *
 *  ⚠️ tab key 必须与 SmartAlbum.vue 的 tabs 定义一致（face / category / location），
 *  否则高亮永远落在"人物"上（默认回退值）。 */
const NAV = [
  { label: "相册", path: "/albums", tab: null },
  { label: "时间线", path: "/timeline", tab: null },
  { label: "人物", path: "/smart", tab: "face" },
  { label: "地点", path: "/smart", tab: "location" },
  { label: "语义", path: "/smart", tab: "category" },
] as const;

/** 导航项的完整 href（/smart 需要带 query） */
function hrefOf(item: (typeof NAV)[number]): string {
  return item.tab ? `${item.path}?tab=${item.tab}` : item.path;
}

/** 当前激活项：按 path + tab 匹配（/album/3 也算"相册"） */
function isActive(item: (typeof NAV)[number]): boolean {
  if (item.path === "/albums") {
    return route.path === "/albums" || route.path.startsWith("/album/");
  }
  if (item.path === "/timeline") return route.path === "/timeline";
  if (item.path === "/smart") {
    if (route.path !== "/smart") return false;
    return (route.query.tab ?? "face") === item.tab;
  }
  // 前面的分支已穷尽 NAV 的所有 path；这里保留一个显式 fallback
  // 而不写 `return route.path === item.path` —— TS 会把 item 收窄为 never
  return false;
}

const username = computed(() => auth.user?.username ?? "");
</script>

<template>
  <header class="u-header" :class="{ 'u-header-compact': compact }">
    <!-- 品牌：点击回首页（这是唯一的"返回"入口，所有页面共享） -->
    <button class="u-brand" type="button" title="回到首页" @click="router.push('/home')">
      <span class="u-brand-dot" aria-hidden="true"></span>
      <span class="u-brand-txt">{{ title || "相册" }}</span>
    </button>

    <nav v-if="showNav" class="u-nav" aria-label="主导航">
      <RouterLink
        v-for="item in NAV"
        :key="item.label"
        :to="hrefOf(item)"
        class="u-nav-item"
        :class="{ on: isActive(item) }"
      >
        {{ item.label }}
      </RouterLink>
    </nav>

    <div class="u-header-spacer"></div>

    <!-- 右侧：搜索入口 + 用户 -->
    <button class="u-search" type="button" title="搜索照片（⌘K）" @click="router.push('/search')">
      <span aria-hidden="true">🔍</span>
      <span class="u-search-txt">搜索照片</span>
    </button>

    <button
      v-if="username"
      class="u-user"
      type="button"
      :title="`${username} · 账户设置`"
      @click="router.push('/home')"
    >
      <span class="u-user-avatar" aria-hidden="true">👤</span>
    </button>
  </header>
</template>

<style scoped>
.u-header {
  display: flex;
  align-items: center;
  gap: 38px;
  height: 56px;
  padding: 0 var(--sp-8);
  /* 结构层：纯色 + 底部发丝线。不是玻璃 —— 见组件头注释。 */
  border-bottom: 1px solid var(--c-rule);
  background: transparent;
  /* 吸顶：长页面滚动时导航始终可达（用户不必滚回顶部才能换页） */
  position: sticky;
  top: 0;
  z-index: var(--z-sticky);
  /* 吸顶后页面内容会从下方滚过：给一层与页面底色相同的实底，
     避免文字在半透明的导航区里糊在一起。
     （这不是玻璃，是与页面同色的遮罩） */
  backdrop-filter: none;
  background: var(--c-bg);
}

.u-header-compact {
  height: 48px;
  gap: 24px;
}

/* —— 品牌 —— */
.u-brand {
  display: inline-flex;
  align-items: center;
  gap: 9px;
  padding: 0;
  background: none;
  border: none;
  cursor: pointer;
  flex-shrink: 0;
}
.u-brand-dot {
  width: 9px;
  height: 9px;
  border-radius: 3px;
  background: var(--c-accent);
}
.u-brand-txt {
  font-size: 14px;
  font-weight: var(--fw-em);
  letter-spacing: 0.02em;
  color: var(--c-fg);
}

/* —— 主导航：文字 + 当前项金色下划线 —— */
.u-nav {
  display: flex;
  align-items: center;
  gap: 24px;
}
.u-nav-item {
  position: relative;
  padding: 6px 0;
  font-size: var(--fs-body);
  letter-spacing: var(--ls-body);
  color: var(--c-fg-2);
  text-decoration: none;
  transition: color var(--dur-2) var(--ease-out);
}
.u-nav-item:hover {
  color: var(--c-fg);
}
.u-nav-item.on {
  color: var(--c-fg);
}
/* 下划线用 1.5px + bottom:-2px：贴在文字基线下方一点点，
   比贴在元素盒子底部（-6px）更贴近字，视觉上"挂"在词上而不是容器上。 */
.u-nav-item.on::after {
  content: "";
  position: absolute;
  left: 0;
  right: 0;
  bottom: -2px;
  height: 1.5px;
  border-radius: 2px;
  background: var(--c-accent);
}

.u-header-spacer {
  flex: 1;
}

/* —— 搜索：全站唯一的玻璃元素之一（它确实浮在内容之上） —— */
.u-search {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 30px;
  padding: 0 13px 0 10px;
  border-radius: var(--r-pill);
  border: var(--glass-border);
  background: var(--glass-fill);
  box-shadow: var(--glass-shadow);
  backdrop-filter: var(--glass-blur);
  -webkit-backdrop-filter: var(--glass-blur);
  cursor: pointer;
  flex-shrink: 0;
  transition: box-shadow var(--dur-2) var(--ease-out);
}
.u-search span {
  font-size: 11.5px;
  color: var(--c-fg-3);
}
.u-search-txt {
  color: var(--c-fg-2) !important;
}
.u-search:hover {
  box-shadow:
    var(--glass-shadow),
    0 0 0 1px color-mix(in srgb, var(--c-accent) 40%, transparent);
}

/* —— 用户 —— */
.u-user {
  width: 26px;
  height: 26px;
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
.u-user-avatar {
  font-size: 12px;
  opacity: 0.85;
}

/* 窄窗：导航横向滚动而不是换行（换行会把顶栏撑成两行，吸顶高度不稳定） */
@media (max-width: 1080px) {
  .u-header {
    gap: 22px;
    padding: 0 var(--sp-5);
  }
  .u-nav {
    gap: 16px;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .u-nav::-webkit-scrollbar {
    display: none;
  }
}
@media (max-width: 760px) {
  .u-search-txt {
    display: none;
  }
  .u-search {
    padding: 0 10px;
  }
}
</style>
