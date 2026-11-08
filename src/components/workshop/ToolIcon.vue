<script setup lang="ts">
/**
 * 工具图标（FEAT-150）
 *
 * 为什么不用 emoji：工坊里图标要参与排版（40px 槽、与标题基线对齐、hover 时变色），
 * emoji 是彩色位图、大小不可控、深色底上还会自带一圈亮边。描边字形统一用
 * currentColor，跟着文字色走，深浅两套主题都不用额外处理。
 */
withDefaults(
  defineProps<{
    name: "compress" | "text" | "enhance" | "journal" | "sparkle";
    size?: number;
  }>(),
  { size: 20 },
);
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.7"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <!-- 压缩：两只箭头往中线收 -->
    <template v-if="name === 'compress'">
      <path d="M12 3.2v6.2m0 0l2.9-2.9M12 9.4L9.1 6.5" />
      <path d="M12 20.8v-6.2m0 0l2.9 2.9M12 14.6l-2.9 2.9" />
    </template>

    <!-- 文字提取：框 + 三行递减文字 -->
    <template v-else-if="name === 'text'">
      <rect x="3" y="4.5" width="18" height="15" rx="2.5" />
      <path d="M6.8 9.4h6.4M6.8 12.6h10.4M6.8 15.8h5" />
    </template>

    <!-- 画质增强：主星 + 次星 -->
    <template v-else-if="name === 'enhance'">
      <path d="M11 3.5l1.7 4.4 4.4 1.7-4.4 1.7L11 15.7 9.3 11.3 4.9 9.6l4.4-1.7z" />
      <path d="M18 15.6l.85 2.05 2.05.85-2.05.85L18 21.4l-.85-2.05-2.05-.85 2.05-.85z" />
    </template>

    <!-- 主题手账：书脊 + 内页 -->
    <template v-else-if="name === 'journal'">
      <path d="M5.5 4.6A1.6 1.6 0 017.1 3H18a1 1 0 011 1v16a1 1 0 01-1 1H7.1a1.6 1.6 0 01-1.6-1.6z" />
      <path d="M9.4 3v18" opacity=".5" />
      <path d="M12.6 8h3.6M12.6 11.4h2.4" opacity=".7" />
    </template>

    <!-- AI 创作：一颗主星 + 一条小尾巴（与“画质增强”的星形区分开：这里是单星）+ -->
    <template v-else>
      <path d="M12 3.6l1.9 5.1 5.1 1.9-5.1 1.9L12 17.6l-1.9-5.1L5 10.6l5.1-1.9z" />
      <path d="M17.6 16.4l.7 1.7 1.7.7-1.7.7-.7 1.7-.7-1.7-1.7-.7 1.7-.7z" opacity=".55" />
    </template>
  </svg>
</template>
