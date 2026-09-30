<script setup lang="ts">
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAuthStore } from "../stores/auth";
import AuthShell from "../components/AuthShell.vue";

// 登录页固定使用启动壁纸（covers/login-sunset.jpg），不随主题/皮肤变化。
// FEAT-093：壁纸与落日紫金配色的单一事实来源是
//   · src/assets/auth.css（.auth-cover 的 url）
//   · src/utils/loginTheme.ts（色值 + 对比度断言）
// 换图只需改上面两处，本组件不再持有图片资源。
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
    errorMsg.value = "请输入账户名 / 邮箱 / 手机号";
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
  <AuthShell title="本地相册搭子" subtitle="登录后管理你的相册空间">
    <form class="auth-form" @submit.prevent="handleLogin">
      <label class="field">
        <span class="field-label">账户名 / 邮箱 / 手机号</span>
        <input
          v-model="account"
          class="field-input"
          type="text"
          placeholder="输入账户名、邮箱或手机号"
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
