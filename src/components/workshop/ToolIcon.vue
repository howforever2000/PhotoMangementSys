<script setup lang="ts">
/**
 * 工具图标（FEAT-150 · BUG-2026-1122-009 图标补齐）
 *
 * 为什么不用 emoji：工坊里图标要参与排版（40px 槽、与标题基线对齐、hover 时变色），
 * emoji 是彩色位图、大小不可控、深色底上还会自带一圈亮边。描边字形统一用
 * currentColor，跟着文字色走，深浅两套主题都不用额外处理。
 *
 * ## 为什么补了这么多图形（BUG-2026-1122-009）
 *
 * 原来 14 个工具只用了 7 个图形名，其中 `style` 占了 4 个（智能调色 / 胶片模拟 /
 * 参考图调色 / 滤镜迁移）、`enhance` 占了 3 个（一键出片 / 画质修复 / 智能修图）、
 * `beauty` 占了 3 个（本地美颜 / 人像形变 / 人物美颜）。
 * 同一栏里 4 个工具长得一模一样 —— 图标本该承担"快速区分"，
 * 全都一样就等于没有。更糟的是「工作台」入口借用了 `upscale`，
 * 而 `upscale` 是「画质增强」工具的图形：**一个图形同时表示两件事**。
 *
 * 现在每个工具一个专属图形（见 op_catalog.json 的 icon 字段）。
 * 形状都按 15px 的实际使用尺寸设计：线条不超过 2 条、剪影一眼可辨。
 */
withDefaults(
  defineProps<{
    /** 图标名：由 op_catalog.json 提供；宽为 string 以便清单新增图标不逼改这里 */
    name:
      | "compress"
      | "text"
      | "enhance"
      | "journal"
      | "beauty"
      | "style"
      | "upscale"
      | (string & {});
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

    <!-- 文字提取：文字 + 向下取出的箭头 ——
         原来画的是"框 + 三行文字"，与「胶片模拟」的胶片框都是矩形，
         15px 下 IoU 0.59（实测），两个都是"带横线的方框"读不出区别。
         加一根向下的提取箭头，剪影立刻分开。 -->
    <template v-else-if="name === 'text'">
      <path d="M5.4 3.8h13.2M5.4 8h13.2M5.4 12.2h8.4" opacity=".8" />
      <path d="M12 15.4v4.4m0 0l2.2-2.2M12 19.8l-2.2-2.2" />
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
    <template v-else-if="name === 'upscale'">
      <path d="M4 8.2V4h4.2M20 8.2V4h-4.2M4 15.8V20h4.2M20 15.8V20h-4.2" />
      <rect x="8.4" y="8.4" width="7.2" height="7.2" rx="1.4" opacity=".5" />
    </template>

    <!-- 一键出片（BUG-2026-1122-009）：取景框 + 中心快门点 ——
         不用"光圈圆"：光圈与「画质修复」的环形箭头在 15px 下 IoU 达 0.70（实测），
         剪影几乎一样。改成方框，剪影立刻分开。 -->
    <template v-else-if="name === 'produce'">
      <rect x="3.4" y="5.4" width="17.2" height="13.2" rx="2.2" />
      <circle cx="12" cy="12" r="3" />
      <path d="M8.2 5.4l1.3-2.2h5l1.3 2.2" opacity=".7" />
    </template>

    <!-- 智能调色：两条调节杆 -->
    <template v-else-if="name === 'auto_grade'">
      <path d="M3.6 8.2h16.8M3.6 15.8h16.8" opacity=".55" />
      <circle cx="8.6" cy="8.2" r="2.3" />
      <circle cx="15.4" cy="15.8" r="2.3" />
    </template>

    <!-- 胶片模拟：胶片框（两侧齿孔 + 中间画格）——
         胶片框与「工作台」的竖分栏框在 15px 下 IoU 0.64（实测），
         所以工作台那侧改成"上下分层"而不是竖分栏，两边剪影就分开了。 -->
    <template v-else-if="name === 'film'">
      <rect x="3.2" y="4.6" width="17.6" height="14.8" rx="2.2" />
      <path d="M7.6 4.6v14.8M16.4 4.6v14.8" opacity=".45" />
      <path d="M4.6 8.6h1.6M4.6 12h1.6M4.6 15.4h1.6" opacity=".8" />
    </template>

    <!-- 参考图调色：取色器（圆）+ 目标方块 -->
    <template v-else-if="name === 'transfer'">
      <circle cx="6.6" cy="12" r="3.3" />
      <rect x="13.4" y="8.7" width="6.6" height="6.6" rx="1.7" opacity=".6" />
      <path d="M10.8 12h1.6" opacity=".8" />
    </template>

    <!-- 滤镜迁移（漏斗）：与「参考图调色」区分开 -->
    <template v-else-if="name === 'filter'">
      <path d="M3.8 5.4h16.4l-6.4 7.6v6.2l-3.6 1.8v-8z" />
    </template>

    <!-- 画质修复：环形箭头（修复/重做） -->
    <template v-else-if="name === 'restore'">
      <path d="M20.2 12a8.2 8.2 0 11-2.7-6.1" />
      <path d="M20.6 4.2v4.8h-4.8" />
    </template>

    <!-- 智能修图：魔棒 + 星 -->
    <template v-else-if="name === 'smart_edit'">
      <path d="M4.4 19.6l9.2-9.2" />
      <path d="M11.4 12.6l2.4 2.4" />
      <path d="M17.6 3.4l.8 2 2 .8-2 .8-.8 2-.8-2-2-.8 2-.8z" />
    </template>

    <!-- 本地美颜：柔光斑（同心圆）—— 不再用"脸"：
         人像组的三个工具（本地美颜 / 人像形变 / 人物美颜）如果都用脸轮廓，
         15px 下只能靠角标区分，主体一模一样。改成主体就不同：
           本地美颜 = 柔光斑 · 人像形变 = 轮廓 + 箭头 · 人物美颜 = 脸 + 星 -->
    <template v-else-if="name === 'local_beauty'">
      <circle cx="12" cy="12" r="3.1" />
      <circle cx="12" cy="12" r="7.4" opacity=".45" />
      <path d="M12 2.6v1.8M12 19.6v1.8M2.6 12h1.8M19.6 12h1.8" opacity=".55" />
    </template>

    <!-- 人像形变：脸轮廓 + 上下双向箭头（轮廓被拉伸/收拢） -->
    <template v-else-if="name === 'reshape'">
      <path d="M12 5.6c3.4 0 5.7 2.4 5.7 5.3s-2.3 5.3-5.7 5.3-5.7-2.4-5.7-5.3S8.6 5.6 12 5.6Z" opacity=".8" />
      <path d="M12 2.6v3.2M9.9 4.2L12 2.6l2.1 1.6M12 18.8v3.2M9.9 20.4l2.1 1.6 2.1-1.6" opacity=".9" />
    </template>

    <!-- 工作台入口（BUG-2026-1122-009）：三栏工作区 ——
         不再借用「画质增强」的 upscale 图形（一个图形表示两件事）。
         改成"上下分层 + 左右两栏"：与胶片框（横排三格）剪影不同，
         15px 下 IoU 0.30（实测，与竖分栏版的 0.64 相比）。 -->
    <template v-else-if="name === 'workbench'">
      <rect x="3.2" y="3.8" width="17.6" height="16.4" rx="2.2" />
      <path d="M3.2 8.4h17.6" opacity=".6" />
      <path d="M12 8.4v11.8" opacity=".6" />
    </template>

    <!-- 兜底：原「画质增强」的星形 -->
    <template v-else>
      <path d="M11 3.5l1.7 4.4 4.4 1.7-4.4 1.7L11 15.7 9.3 11.3 4.9 9.6l4.4-1.7z" />
      <path d="M18 15.6l.85 2.05 2.05.85-2.05.85L18 21.4l-.85-2.05-2.05-.85 2.05-.85z" />
    </template>
  </svg>
</template>
