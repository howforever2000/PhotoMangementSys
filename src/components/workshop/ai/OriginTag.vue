<script setup lang="ts">
/**
 * 模型来源角标（FEAT-199）
 *
 * 一枚弱色小字，回答「这一步是本地模型，还是要调 API？」。判据全部来自
 * `utils/modelOrigin.ts`（纯函数，派生自 catalog + 已绑定的供应商地址），
 * 本组件只负责呈现，不含任何判定。
 *
 * 为什么不做成彩色徽标：工坊定位是「懒人快速出图」，角标只是让用户不必去翻
 * 「模型设置」就能分辨花费与隐私；它不该抢走工具名的注意力
 * （见 `.dsh/knowledge/workshop-positioning.md` 与左栏 `.side-tag` 的既有取色）。
 * 只有「未配置模型」用危险色 —— 那是会让整条链跑不起来的真实状态。
 */
import { computed } from "vue";
import type { OriginInfo } from "../../../utils/modelOrigin";

const props = withDefaults(
  defineProps<{
    info: OriginInfo;
    /** short = 单字形态（横向工具条 / 极窄空间） */
    variant?: "full" | "short";
    /** 只显示标签，不挂 hover 说明（外层已有自己的 title 时用，避免双 tooltip） */
    noTitle?: boolean;
  }>(),
  { variant: "full", noTitle: false },
);

const text = computed(() => (props.variant === "short" ? props.info.short : props.info.label));
const title = computed(() => (props.noTitle ? undefined : props.info.detail));
</script>

<template>
  <span class="origin-tag" :class="info.origin" :title="title">{{ text }}</span>
</template>

<style scoped>
.origin-tag {
  flex: none;
  font-size: 10.5px;
  line-height: 1.4;
  white-space: nowrap;
  color: var(--fg-3, var(--fg-2));
  font-variant-numeric: tabular-nums;
}

/* 云端：这是唯一「照片出门 + 花钱」的一档，值得稍微提一点亮度 */
.origin-tag.cloud-api {
  color: var(--c-accent);
}

/* 未配置：会让整条链跑不起来，按危险色提示 */
.origin-tag.unconfigured {
  color: var(--c-danger);
}

/* 本机两档保持弱色：免费离线是默认预期，不需要每次都被强调 */
.origin-tag.local-algo,
.origin-tag.local-model {
  color: var(--fg-3, var(--fg-2));
}
</style>
