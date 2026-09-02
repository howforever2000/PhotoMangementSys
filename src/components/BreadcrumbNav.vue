<script setup lang="ts">
import { useRouter } from "vue-router";

interface Crumb {
  label: string;
  /** 未定义则表示当前页（不可点击） */
  to?: string | undefined;
}

defineProps<{
  /** 面包屑路径，数组最后一项为当前页 */
  crumbs: Crumb[];
}>();

const router = useRouter();

function navigate(to?: string) {
  if (!to) return;
  router.push(to);
}
</script>

<template>
  <nav class="breadcrumb-nav" aria-label="面包屑导航">
    <ol class="breadcrumb-list">
      <li
        v-for="(crumb, i) in crumbs"
        :key="i"
        class="breadcrumb-item"
      >
        <!-- 非最后一项：可点击 -->
        <template v-if="i < crumbs.length - 1 && crumb.to">
          <a
            class="breadcrumb-link"
            :href="'#' + crumb.to"
            @click.prevent="navigate(crumb.to)"
          >{{ crumb.label }}</a>
          <span class="breadcrumb-sep" aria-hidden="true">›</span>
        </template>
        <!-- 最后一项：当前页，不可点击 -->
        <span v-else class="breadcrumb-current" aria-current="page">{{ crumb.label }}</span>
      </li>
    </ol>
  </nav>
</template>

<style scoped>
.breadcrumb-nav {
  display: flex;
  align-items: center;
  gap: 4px;
}

.breadcrumb-list {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 2px;
  list-style: none;
  margin: 0;
  padding: 0;
}

.breadcrumb-item {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 13px;
}

.breadcrumb-link {
  color: #396cd8;
  text-decoration: none;
  cursor: pointer;
  padding: 1px 3px;
  border-radius: 4px;
  transition: background 0.12s, color 0.12s;
}

.breadcrumb-link:hover {
  background: rgba(57, 108, 216, 0.1);
  text-decoration: underline;
}

.breadcrumb-sep {
  color: #b0bac8;
  font-size: 14px;
  user-select: none;
}

.breadcrumb-current {
  color: var(--color-text, #2c3e50);
  font-weight: 500;
  cursor: default;
}
</style>
