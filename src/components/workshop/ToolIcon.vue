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
    name: "compress" | "text" | "enhance" | "journal" | "beauty" | "style" | "upscale";
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

    <!-- 人物美颜：脸 + 亮点星辰 -->
    <template v-else-if="name === 'beauty'">
      <path d="M12 3.6c3.8 0 6.4 2.9 6.4 6.7 0 3.9-2.8 7.1-6.4 7.1S5.6 14.2 5.6 10.3C5.6 6.5 8.2 3.6 12 3.6Z" />
      <path d="M9.2 10.1h.02M14.8 10.1h.02" />
      <path d="M9.6 13c.8.6 1.5.9 2.4.9s1.6-.3 2.4-.9" opacity=".7" />
      <path d="M18.4 3.1l.55 1.4 1.4.55-1.4.55-.55 1.4-.55-1.4-1.4-.55 1.4-.55z" />
    </template>

    <!-- 滤镜迁移：两层叠放的色块 -->
    <template v-else-if="name === 'style'">
      <rect x="3.6" y="3.6" width="9.6" height="9.6" rx="2" />
      <rect x="10.8" y="10.8" width="9.6" height="9.6" rx="2" opacity=".55" />
    </template>

    <!-- 画质增强：四角外扩 -->
    <template v-else>
      <path d="M4 8.2V4h4.2M20 8.2V4h-4.2M4 15.8V20h4.2M20 15.8V20h-4.2" />
      <rect x="8.4" y="8.4" width="7.2" height="7.2" rx="1.4" opacity=".5" />
    </template>
  </svg>
</template>
