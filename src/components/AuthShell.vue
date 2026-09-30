<script setup lang="ts">
/**
 * 三张鉴权页（登录 / 注册 / 忘记密码）共用的外壳（FEAT-093）
 *
 * 收口三件事，避免三页各写一份：
 *   1. 启动壁纸 + 遮罩 + 居中玻璃卡片的结构；
 *   2. 落日紫金配色令牌的内联下发（色值来自 utils/loginTheme.ts，单一事实来源）；
 *   3. 「默认效果」材质接法：卡片挂 .glass-surface，模糊/高光/棱环由 main.css 的
 *      body.mat-* 提供，用户在主题弹窗换材质时登录页跟着变。
 *
 * 表单内容与页脚由使用方通过默认插槽 / footer 插槽传入（样式在 assets/auth.css）。
 */
import { authCssVars } from "../utils/loginTheme.ts";

withDefaults(defineProps<{ title: string; subtitle?: string }>(), { subtitle: "" });

const authVars = authCssVars();
</script>

<template>
  <div class="auth-page" :style="authVars">
    <div class="auth-cover" aria-hidden="true"></div>
    <div class="auth-overlay" aria-hidden="true"></div>

    <div class="auth-card glass-surface">
      <header class="auth-header">
        <h1 class="auth-title">{{ title }}</h1>
        <p v-if="subtitle" class="auth-subtitle">{{ subtitle }}</p>
      </header>

      <slot />

      <footer class="auth-footer">
        <slot name="footer" />
      </footer>
    </div>
  </div>
</template>
