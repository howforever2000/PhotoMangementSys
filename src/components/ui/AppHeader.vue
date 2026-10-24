<script setup lang="ts">
/**
 * AppHeader —— 大组件内部导航条（FEAT-105 定稿）
 *
 * ⚠️ 这条栏**不跨大组件**。它上面每一项都属于"当前这个大组件"内部的功能分区。
 *    跨大组件的入口只有一个地方：Home 的四张卡（点「← 主页」回去换）。
 *
 * 演进记录（很重要，避免再走回头路）：
 *   第 1 版 NAV = 相册/时间线/人物/地点/语义 —— 后四项是 /smart 内部的 tab，
 *            却和"相册"平起平坐，层级错位。
 *   第 2 版把导航整条删掉，只留「← 主页 + 模块名」—— 矫枉过正：
 *            大组件内部确实需要一条常驻的分区导航（截图里的"概览/相册管理/…"
 *            位置就是这个），只是它不能是别的模块。
 *   第 3 版（现在）栏内 = **本组件内部的分区**，由每个页面通过 `tabs` 传入。
 *
 * 结构（对齐效果图）：
 *   [← 主页] [组件名] │ 分区1 分区2 分区3 …                    [搜索] [头像]
 *   左下角另有一级���航的语义：不需要。回 Home 换组件只有一个出口。
 *
 * 玻璃：这条栏不是玻璃，是结构性元素（页面底色 + 底部发丝线）。
 *   栏里唯一的玻璃是右侧搜索胶囊（它确实浮在内容之上）。
 */
import { computed } from "vue";
import { useRouter } from "vue-router";
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
const auth = useAuthStore();
const username = computed(() => auth.user?.username ?? "");
</script>

<template>
  <header class="mh" :class="{ 'mh-compact': compact }">
    <!-- 唯一出口：回 Home 换大组件（这不是导航项，是"退出本组件"） -->
    <button class="mh-back" type="button" title="回到主页（切换其他组件）" @click="router.push('/home')">
      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" aria-hidden="true">
        <path d="M15 5l-7 7 7 7" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
      <span>主页</span>
    </button>

    <!-- 组件名 -->
    <div class="mh-name">
      <span v-if="icon" class="mh-emo" aria-hidden="true">{{ icon }}</span>
      <span class="mh-title">{{ title || "相册" }}</span>
      <span v-if="meta" class="mh-meta">{{ meta }}</span>
    </div>

    <!-- 组件内部分区导航：每一项都属于本组件 -->
    <nav v-if="tabs.length" class="mh-nav" aria-label="组件内分区">
      <template v-for="t in tabs" :key="t.key || t.label">
        <RouterLink v-if="t.to && !t.disabled" :to="t.to" class="mh-nav-item" :class="{ on: $route.path === t.to }">
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
      <span aria-hidden="true">🔍</span>
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
  </header>
</template>

<style scoped>
.mh {
  display: flex;
  align-items: center;
  gap: 16px;
  height: 56px;
  padding: 0 var(--sp-8);
  /* 结构层：页面底色 + 底部发丝线；吸顶时内容从下方滚过，实底遮住 */
  background: var(--c-bg);
  border-bottom: 1px solid var(--c-rule);
  position: sticky;
  top: 0;
  z-index: var(--z-sticky);
  backdrop-filter: none;
}

.mh-compact {
  height: 48px;
  gap: 12px;
}

/* —— 返回主页 —— */
.mh-back {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 26px;
  padding: 0 9px 0 6px;
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
  font-size: 16px;
  align-self: center;
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
  gap: 4px;
  min-width: 0;
  padding-left: 16px;
  border-left: 1px solid var(--c-rule);
  height: 22px;
}
.mh-nav-item {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 10px;
  border: none;
  background: none;
  border-radius: 6px;
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
  background: var(--hover, rgba(255, 255, 255, 0.045));
}
.mh-nav-item.on {
  color: var(--c-fg);
  font-weight: var(--fw-em);
  background: var(--hover, rgba(255, 255, 255, 0.045));
}
/* 选中下划线贴在文字下方，比"整块高亮"轻，与全站的 tab 语言一致 */
.mh-nav-item.on::after {
  content: "";
  position: absolute;
  left: 10px;
  right: 10px;
  bottom: 4px;
  height: 1.5px;
  border-radius: 2px;
  background: var(--c-accent);
}
.mh-nav-item.off {
  /* FEAT-105 实测：置灰项用 --c-fg-3 在浅色预设上仅 3.58:1。
     置灰的目的是“看似不可用”，不是“读不出来” —— 用户得先看清这个词
     才知道这块功能叫什么，再去 hover 读 why。
     改用 --c-fg-2 + opacity .75：既弱于可点项（--c-fg），又保底可读。 */
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
  color: var(--c-fg-3);
  background: var(--hover, rgba(255, 255, 255, 0.06));
  font-variant-numeric: tabular-nums;
}
.mh-nav-item.on .mh-nav-n {
  color: var(--c-accent);
  background: color-mix(in srgb, var(--c-accent) 14%, transparent);
}

.mh-sp {
  flex: 1;
}

/* —— 搜索胶囊：栏里唯一的玻璃 —— */
.mh-search {
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
.mh-search span {
  font-size: 11.5px;
  color: var(--c-fg-3);
}
.mh-search-txt {
  color: var(--c-fg-2) !important;
}
.mh-search:hover {
  box-shadow:
    var(--glass-shadow),
    0 0 0 1px color-mix(in srgb, var(--c-accent) 40%, transparent);
}

/* —— 用户 —— */
.mh-user {
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
.mh-user span {
  font-size: 12px;
  opacity: 0.85;
}

/* 窄窗：分区导航横向滚动而不是换行（换行会把顶栏撑成两行，吸顶高度不稳定） */
@media (max-width: 1180px) {
  .mh {
    gap: 10px;
    padding: 0 var(--sp-5);
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
