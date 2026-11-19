<script setup lang="ts">
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAuthStore } from "../stores/auth";
import AuthShell from "../components/AuthShell.vue";
import BrandMark from "../components/ui/BrandMark.vue";

// 登录页固定使用启动壁纸（covers/login-starfield.jpg），不随主题/皮肤变化。
// FEAT-093：壁纸与星空蓝白配色的单一事实来源是
//   · src/assets/auth.css（.auth-cover 的 url）
//   · src/utils/loginTheme.ts（色值 + 对比度断言）
// 换图只需改上面两处（外加 theme.ts 的 DEFAULT_WALLPAPER），本组件不再持有图片资源。
const auth = useAuthStore();
const router = useRouter();
const route = useRoute();

const account = ref("");
const password = ref("");
const errorMsg = ref("");
const isSubmitting = ref(false);

async function handleLogin() {
  errorMsg.value = "";
  if (!account.value.trim()) {
    errorMsg.value = "请输入账户名 / 邮箱";
    return;
  }
  if (!password.value) {
    errorMsg.value = "请输入密码";
    return;
  }
  isSubmitting.value = true;
  try {
    await auth.login({
      account: account.value.trim(),
      password: password.value,
    });
    // 登录成功：跳回来源页（默认主页）
    const redirect = typeof route.query.redirect === "string" ? route.query.redirect : "/home";
    router.replace(redirect);
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    isSubmitting.value = false;
  }
}
</script>

<template>
  <AuthShell>
    <!-- 品牌标识（FEAT-189）：与首页页头同一组件，尺寸档换成 auth -->
    <template #header>
      <BrandMark size="auth" />
    </template>
    <form class="auth-form" @submit.prevent="handleLogin">
      <label class="field">
        <span class="field-label">账户名 / 邮箱</span>
        <input
          v-model="account"
          class="field-input"
          type="text"
          placeholder="输入账户名或邮箱"
          autocomplete="username"
        />
      </label>

      <label class="field">
        <span class="field-label">密码</span>
        <input
          v-model="password"
          class="field-input"
          type="password"
          placeholder="输入密码"
          autocomplete="current-password"
        />
      </label>

      <p v-if="errorMsg" class="error-msg" role="alert">{{ errorMsg }}</p>

      <button class="btn-primary" type="submit" :disabled="isSubmitting">
        {{ isSubmitting ? "登录中…" : "登 录" }}
      </button>
    </form>

    <template #footer>
      <router-link class="auth-link" to="/forgot-password">忘记密码？</router-link>
      <span class="auth-divider">|</span>
      <router-link class="auth-link" to="/register">注册新账户</router-link>
    </template>
  </AuthShell>
</template>
