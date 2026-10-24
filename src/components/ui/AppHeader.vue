<script setup lang="ts">
/**
 * AppHeader —— 模块页头（FEAT-105 重做）
 *
 * ⚠️ 这里**没有全站一级导航**。上一版把「相册管理 / 智慧相册 / 创意工坊 / 扫描入库」
 * 放在这条栏里，等于在模块页里又摆了一条全局导航 —— 与「每个大模块独立、顶栏只导航
 * 本模块内部」的目标直接冲突。一级入口只存在一个地方：Home 的四张卡。
 *
 * 所以这条栏只做三件事：
 *   ① 退出本模块（← 主页）
 *   ② 说明你在哪个模块（emoji + 模块名 + 一行读数）
 *   ③ 全局动作（搜索、头像）—— 它们不是导航，不构成层级
 *
 * 模块内部的二级导航（人物/时间线/回忆/…）由 ModuleTabs 承担，紧贴这条栏下方。
 *
 * 玻璃：这条栏不是玻璃，是结构性元素 —— 纯页面底色 + 底部发丝线。滚动时也不变。
 *   栏里唯一的玻璃是搜索胶囊（它确实浮在内容之上）。
 */
import { computed } from "vue";
import { useRouter } from "vue-router";
import { useAuthStore } from "../../stores/auth";

withDefaults(
  defineProps<{
    /** 模块名（如「智慧相册」） */
    title?: string;
    /** 模块 emoji（Home 四张卡上的同一个） */
    icon?: string;
    /** 一行读数，例如「170 位人物 · 21 类语义」；不传则整块不渲染 */
    meta?: string;
    /** 是否显示「← 主页」（灯箱/独立壳里关掉） */
    showHome?: boolean;
    /** 紧凑高度 */
    compact?: boolean;
  }>(),
  { title: "", icon: "", meta: "", showHome: true, compact: false },
);

const router = useRouter();
const auth = useAuthStore();
const username = computed(() => auth.user?.username ?? "");
</script>

<template>
  <header class="mh" :class="{ 'mh-compact': compact }">
    <!-- ① 退出本模块：唯一的"返回"入口。不是一级导航，只是一个出口。 -->
    <button v-if="showHome" class="mh-back" type="button" title="回到主页（切换其他模块）" @click="router.push('/home')">
      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" aria-hidden="true">
        <path d="M15 5l-7 7 7 7" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
      <span>主页</span>
    </button>

    <!-- ② 你在哪个模块 -->
    <div class="mh-name">
      <span v-if="icon" class="mh-emo" aria-hidden="true">{{ icon }}</span>
      <h1 class="mh-title">{{ title || "相册" }}</h1>
      <span v-if="meta" class="mh-meta">{{ meta }}</span>
    </div>

    <div class="mh-sp"></div>

    <!-- ③ 全局动作（非导航） -->
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
  gap: 14px;
  height: 56px;
  padding: 0 var(--sp-8);
  /* 结构层：页面底色 + 底部发丝线。吸顶时内容从下方滚过，
     用与页面同色的实底遮住，避免文字在半透明导航里糊在一起。 */
  background: transparent;
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

/* —— ① 返回主页 —— */
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

/* —— ② 模块名 + 读数 —— */
.mh-name {
  display: flex;
  align-items: baseline;
  gap: 9px;
  min-width: 0;
}
.mh-emo {
  font-size: 17px;
  line-height: 1;
  align-self: center;
}
.mh-title {
  margin: 0;
  font-size: var(--fs-head);
  font-weight: var(--fw-strong);
  letter-spacing: -0.018em;
  line-height: 1.15;
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

.mh-sp {
  flex: 1;
}

/* —— ③ 搜索胶囊：栏里唯一的玻璃 —— */
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

@media (max-width: 1080px) {
  .mh {
    gap: 10px;
    padding: 0 var(--sp-5);
  }
  .mh-meta {
    display: none;
  }
}
@media (max-width: 760px) {
  .mh-search-txt {
    display: none;
  }
  .mh-search {
    padding: 0 10px;
  }
  .mh-title {
    font-size: var(--fs-title);
  }
}
</style>
