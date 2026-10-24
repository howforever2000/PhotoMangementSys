<script lang="ts">
export interface ModuleTab {
  /** 稳定 key（状态型用它比 active；路由型可不填） */
  key?: string;
  label: string;
  /** 计数徽标（"170"，可以不传） */
  count?: number | string;
  /** 跳转路由；不传则为本页内状态切换 */
  to?: string;
  /** 置灰但可见（表示"有这块功能，当前条件不可用"） */
  disabled?: boolean;
  /** 置灰原因，写进 title —— 灰掉必须讲原因，否则用户以为坏了 */
  why?: string;
}
</script>

<script setup lang="ts">
/**
 * ModuleTabs —— 模块内部的二级导航（FEAT-105）
 *
 * 只做一件事：在当前大模块内部切换小功能。它**不是**一级导航，
 * 一级入口只有 Home 的四张卡；所以这里每一项都必须属于同一个模块。
 *
 * 为什么不用顶栏承载：模块内的小功能有 6~7 个（人物/时间线/回忆/地点/
 * 语义分类/智能搜索/以图搜图），放进 56px 顶栏会把"我在哪个模块"和
 * "我在模块的哪块"混成一行，层级就塌了。
 *
 * 形态：34px 文字 tab + 选中项 1.5px 金色下划线，贴在页头正下方。
 *   下划线 bottom:-1px 压在容器的 1px 分隔线上，读作"这一项连着下面的内容"；
 *   若贴在元素盒子底部（-6px）会浮在半空，看起来像按钮而不是 tab。
 *
 * 路由型与状态型混用：`to` 有值走 RouterLink（跨路由，如 /timeline），
 * 无值走本地 @click（同页切 tab，如 人物/内容分类/地点）。
 */
/**
 * ModuleTabs —— 模块内部的二级导航（FEAT-105）
 *
 * 只做一件事：在当前大模块内部切换小功能。它**不是**一级导航，
 * 一级入口只有 Home 的四张卡；所以这里每一项都必须属于同一个模块。
 *
 * 为什么不用顶栏承载：模块内的小功能有 6~7 个（人物/时间线/回忆/地点/
 * 语义分类/智能搜索/以图搜图），放进 56px 顶栏会把"我在哪个模块"和
 * "我在模块的哪块"混成一行，层级就塌了。
 *
 * 形态：34px 文字 tab + 选中项 1.5px 金色下划线，贴在页头正下方。
 *   下划线 bottom:-1px 压在容器的 1px 分隔线上，读作"这一项连着下面的内容"；
 *   若贴在元素盒子底部（-6px）会浮在半空，看起来像按钮而不是 tab。
 *
 * 路由型与状态型混用：`to` 有值走 RouterLink（跨路由，如 /timeline），
 * 无值走本地 @click（同页切 tab，如 人物/内容分类/地点）。
 */
import { useRoute } from "vue-router";

export interface ModuleTab {
  /** 稳定 key（状态型用它比 active；路由型可不填） */
  key?: string;
  label: string;
  /** 计数徽标（"170"，可以不传） */
  count?: number | string;
  /** 跳转路由；不传则为本页内状态切换 */
  to?: string;
  /** 置灰但可见（表示"有这块功能，当前条件不可用"） */
  disabled?: boolean;
  /** 置灰原因，写进 title —— 灰掉必须讲原因，否则用户以为坏了 */
  why?: string;
}

const emit = defineEmits<{ pick: [key: string] }>();
const props = defineProps<{
  tabs: ModuleTab[];
  /** 当前激活的 key（状态型用）或路径（路由型用） */
  active?: string;
  /** 右侧动作区（"新建相册"这类本模块的操作，不是导航） */
  ariaLabel?: string;
}>();

const route = useRoute();

/** 路由型 tab 的激活判定。
 *
 * 必须同时比 path **和** to 里写的 query —— 因为 /smart、/smart?tab=category、
 * /smart?tab=location 的 path 完全一样，只比 path 会让三个 tab 同时高亮。
 *
 * 父路径（/albums）不吃子路径（/album/3）：子页的高亮由面包屑承担。
 *
 * 「/smart」这类不带 query 的自指项特别处理：它只在 path 匹配**且完全没有 query 指向
 * 本模块其他 tab** 时亮。否则当用户从 /search 回到 /smart 时，若残留 ?tab=xxx
 * 会出现两项同时亮（渲染实测见过）。 */
function isOn(t: ModuleTab): boolean {
  if (!t.to) return !!t.key && t.key === (props.active || "");

  const [path, search = ""] = t.to.split("?");
  if (route.path !== path) return false;
  if (!search) {
    // 不带 query 的项：只有当当前路由也没带 tab 参数时才亮（tab 参数代表另一个同级 tab）
    return !route.query.tab;
  }
  const want = new URLSearchParams(search);
  for (const [k, v] of want.entries()) {
    if (String(route.query[k] ?? "") !== v) return false;
  }
  return true;
}
</script>

<template>
  <nav class="mtabs" :aria-label="ariaLabel">
    <template v-for="t in tabs" :key="t.key || t.label">
      <RouterLink
        v-if="t.to && !t.disabled"
        :to="t.to"
        class="mtab"
        :class="{ on: isOn(t) }"
      >
        {{ t.label }}
        <span v-if="t.count !== undefined && t.count !== ''" class="mtab-n">{{ t.count }}</span>
      </RouterLink>
      <button
        v-else
        type="button"
        class="mtab"
        :class="{ on: isOn(t), off: t.disabled }"
        :disabled="t.disabled"
        :title="t.disabled ? t.why : undefined"
        @click="!t.disabled && t.key && emit('pick', t.key)"
      >
        {{ t.label }}
        <span v-if="t.count !== undefined && t.count !== ''" class="mtab-n">{{ t.count }}</span>
      </button>
    </template>

    <div class="mtabs-sp"></div>
    <slot name="actions" />
  </nav>
</template>

<style scoped>
.mtabs {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 0 var(--sp-8);
  border-bottom: 1px solid var(--c-rule);
}

.mtab {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 34px;
  padding: 0 12px;
  border: none;
  background: none;
  font-size: var(--fs-body);
  letter-spacing: var(--ls-body);
  color: var(--c-fg-2);
  text-decoration: none;
  white-space: nowrap;
  cursor: pointer;
  transition: color var(--dur-2) var(--ease-out);
}
.mtab:hover {
  color: var(--c-fg);
}
.mtab.on {
  color: var(--c-fg);
  font-weight: var(--fw-em);
}
.mtab.on::after {
  content: "";
  position: absolute;
  left: 10px;
  right: 10px;
  bottom: -1px;
  height: 1.5px;
  border-radius: 2px;
  background: var(--c-accent);
}

/* 计数徽标：把"有多少"贴在名字后面，省掉一行解释 */
.mtab-n {
  font-size: 10px;
  line-height: 1;
  padding: 2px 5px;
  border-radius: 4px;
  color: var(--c-fg-3);
  background: var(--hover, rgba(255, 255, 255, 0.06));
  font-variant-numeric: tabular-nums;
}
.mtab.on .mtab-n {
  color: var(--c-accent);
  background: color-mix(in srgb, var(--c-accent) 14%, transparent);
}

/* 置灰：仍然可读（不是 hidden），只是不可点 */
.mtab.off {
  color: var(--c-fg-3);
  cursor: not-allowed;
}

.mtabs-sp {
  flex: 1;
}

@media (max-width: 900px) {
  .mtabs {
    padding: 0 var(--sp-5);
    overflow-x: auto;
    scrollbar-width: none;
  }
  .mtabs::-webkit-scrollbar {
    display: none;
  }
}
</style>
