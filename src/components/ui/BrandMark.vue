<script setup lang="ts">
/**
 * BrandMark —— 品牌标识块（中文主名 + 拉丁辅助标识 + 双语标语）
 *
 * 为什么抽成组件（FEAT-189）：首页页头与登录页页头要出现同一块品牌信息，
 * 但两处的配色上下文完全不同 —— 首页坐在用户壁纸上、登录页坐在固定落日壁纸上，
 * 字号也要各自适配（首页 28px / 登录页 24px）。如果各写一份模板，
 * 「中英主次关系」「竖线分隔」「标语大小」这三件事会在两处各漂一次。
 *
 * 消费方只传一个 `size`，内部按尺寸档位下发字号；颜色**一律用 currentColor 派生**，
 * 所以组件不关心自己坐在什么背景上 —— 由外层容器给 color 即可。
 *
 * ⚠ 文案不在这里，来自 `utils/brand.ts`（单一事实源）。
 */
import { BRAND_CN, BRAND_EN, SLOGAN_CN, SLOGAN_EN } from "../../utils/brand.ts";

withDefaults(
  defineProps<{
    /** 尺寸档：home = 首页页头（大），auth = 登录页卡片（略小） */
    size?: "home" | "auth";
    /** 是否显示双语标语 */
    slogan?: boolean;
  }>(),
  { size: "home", slogan: true },
);
</script>

<template>
  <div class="brand" :class="`brand-${size}`">
    <h1 class="brand-name">
      <span class="brand-cn">{{ BRAND_CN }}</span>
      <span class="brand-en">{{ BRAND_EN }}</span>
    </h1>
    <p v-if="slogan" class="brand-slogan">
      <span class="brand-slogan-cn">{{ SLOGAN_CN }}</span>
      <span class="brand-sep" aria-hidden="true"></span>
      <span class="brand-slogan-en">{{ SLOGAN_EN }}</span>
    </p>
  </div>
</template>

<style scoped>
.brand {
  min-width: 0;
}

/* ── 品牌名：中文主 + 拉丁辅，基线对齐 ────────────────────────────────
   为什么不用两个 h1：读屏器会把两个标题念成两条；
   这里中文是主名、拉丁是补充标识，合成的是一件事。 */
.brand-name {
  display: flex;
  align-items: baseline;
  gap: 9px;
  margin: 0;
}

.brand-cn {
  font-weight: 700;
  letter-spacing: 0.01em;
  /* 颜色交给外层：首页是 on-bg 对比色，登录页是纯白。
     组件自身不写色值，免得两处各维护一套。 */
  color: inherit;
  white-space: nowrap;
}

/* 拉丁标识：小字号 + 开阔字距 + 微弱降浓。
   它不跟中文抢注意力，但远看要读成同一个标识块的两半。 */
.brand-en {
  font-family: "Segoe UI", "Helvetica Neue", Arial, sans-serif;
  font-weight: 500;
  letter-spacing: 0.14em;
  text-transform: uppercase;
  opacity: 0.78;
  white-space: nowrap;
}

/* ── 标语：中文 + 竖线 + 英文（A 方案，中英同行） ────────────────────── */
.brand-slogan {
  display: flex;
  align-items: center;
  margin: 0;
  white-space: nowrap;
}

.brand-slogan-cn {
  letter-spacing: 0.02em;
}

.brand-slogan-en {
  letter-spacing: 0.03em;
  opacity: 0.66;
}

/* 分隔线用 1px 竖线而不是「·」：圆点在两段外文里已经出现，竖线是结构性分隔，
   读成「双语的同一句」而不是「两个短语」 */
.brand-sep {
  display: inline-block;
  flex: none;
  width: 1px;
  vertical-align: middle;
  background: currentColor;
  opacity: 0.42;
}

/* ══ 尺寸档 ══════════════════════════════════════════════════════════ */
.brand-home .brand-cn {
  font-size: 28px;
}
.brand-home .brand-en {
  font-size: 12.5px;
}
.brand-home .brand-slogan {
  gap: 0;
  margin-top: 6px;
}
.brand-home .brand-slogan-cn {
  font-size: 13.5px;
}
.brand-home .brand-slogan-en {
  font-size: 11px;
}
.brand-home .brand-sep {
  height: 9px;
  margin: 0 9px;
}

.brand-auth .brand-name {
  justify-content: center;
  gap: 8px;
}
.brand-auth .brand-cn {
  font-size: 24px;
}
.brand-auth .brand-en {
  font-size: 11px;
}
.brand-auth .brand-slogan {
  justify-content: center;
  margin-top: 8px;
}
.brand-auth .brand-slogan-cn {
  font-size: 13px;
}
.brand-auth .brand-slogan-en {
  font-size: 10.5px;
}
.brand-auth .brand-sep {
  height: 9px;
  margin: 0 8px;
}

/* 窄窗：标语整串不换行会把页头撑宽，这里允许折行（品牌名仍保持一行） */
@media (max-width: 640px) {
  .brand-slogan {
    flex-wrap: wrap;
    white-space: normal;
  }
  .brand-home .brand-cn {
    font-size: 24px;
  }
}
</style>
