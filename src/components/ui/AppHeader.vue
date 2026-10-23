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
 *   顶栏里唯一的玻璃是右侧搜索胶囊（它确实浮在内容之上）。
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

/** 主导航项：一级导航 = **产品级功能**，不是页面。
 *
 * FEAT-105 改动：旧 NAV 是 [相册, 时间线, 人物, 地点, 语义]，后四项其实全是
 * `/smart` 一个模块里的 tab —— 结果相册管理 / 创意工坊 / 扫描入库三个模块根本
 * 没有一级入口，而"时间线"却和"相册"平起平坐。这是层级错位。
 *
 * 现在四项 = Home 上那四张卡，一一对应，用户建立的空间感与入口处一致：
 *   相册管理 /albums · 智慧相册 /smart · 创意工坊 /workshop · 扫描入库 /scan
 *
 * 判据：一级项数 == 产品级功能数。凡"点进去只是同一页里的另一块"，一律下沉为二级。
 * 二级（人物/时间线/回忆/地点/语义/搜索）仍各自保留独立路由，只是不再占一级位。
 */
const NAV = [
  { label: "相册管理", path: "/albums" },
  { label: "智慧相册", path: "/smart" },
  { label: "创意工坊", path: "/workshop" },
  { label: "扫描入库", path: "/scan" },
] as const;

/** 归属映射：子页 → 它所属的一级模块。
 *
 * 为什么用映射而不是改路由：`/timeline`、`/memories`、`/search` 是平级路由
 * （各有自己的页面组件与刷新行为），把它们改成 `/smart/timeline` 会动到 URL、
 * 深链接与已存的浏览历史。层级归属本来就是"展示概念"，用一张表就够了。
 *
 * 效果：停在 /timeline 时一级高亮落在「智慧相册」，而不是五项都不亮。
 */
const BELONGS: Record<string, string> = {
  "/timeline": "/smart",
  "/memories": "/smart",
  "/search": "/smart",
  "/scan/test": "/scan",
  "/album/:id": "/albums",
};

/** 把当前路由归一化到一级模块路径（匹配不到就返回原路径） */
function moduleOf(path: string): string {
  if (BELONGS[path]) return BELONGS[path];
  // 前缀类：/album/12 → /albums，/scan/test → /scan
  if (path.startsWith("/album/")) return "/albums";
  if (path.startsWith("/scan/")) return "/scan";
  return path;
}

/** 导航项的完整 href（四项都是平路径，保留函数以便日后二级项复用） */
function hrefOf(item: (typeof NAV)[number]): string {
  return item.path;
}

/** 当前激活项：先归一到一级模块再比，避免子页让导航全灭 */
function isActive(item: (typeof NAV)[number]): boolean {
  return moduleOf(route.path) === item.path;
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
