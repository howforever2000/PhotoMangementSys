<script setup lang="ts">
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAuthStore } from "../stores/auth";

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
  <div class="auth-page">
    <div class="auth-card">
      <header class="auth-header">
        <h1 class="auth-title">本地相册管理</h1>
        <p class="auth-subtitle">登录后管理你的相册空间</p>
      </header>

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

        <p v-if="errorMsg" class="error-msg">{{ errorMsg }}</p>

        <button class="btn-primary" type="submit" :disabled="isSubmitting">
          {{ isSubmitting ? "登录中…" : "登 录" }}
        </button>
      </form>

      <footer class="auth-footer">
        <router-link class="auth-link" to="/forgot-password">忘记密码？</router-link>
        <span class="auth-divider">|</span>
        <router-link class="auth-link" to="/register">注册新账户</router-link>
      </footer>
    </div>
  </div>
</template>

<style scoped>
.auth-page {
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: linear-gradient(160deg, #f5f7fb 0%, #eef1f6 100%);
}

.auth-card {
  width: 100%;
  max-width: 400px;
  background: #fff;
  border-radius: 16px;
  border: 1px solid #eceef2;
  box-shadow: 0 12px 40px rgba(30, 41, 59, 0.1);
  padding: 36px 32px 28px;
}

.auth-header {
  text-align: center;
  margin-bottom: 28px;
}

.auth-title {
  margin: 0 0 8px;
  font-size: 24px;
  color: #2c3e50;
}

.auth-subtitle {
  margin: 0;
  font-size: 13px;
  color: #8892a6;
}

.auth-form {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.field-label {
  font-size: 13px;
  color: #4a5568;
}

.field-input {
  height: 42px;
  padding: 0 12px;
  font-size: 14px;
  color: #2c3e50;
  border: 1px solid #d8dce3;
  border-radius: 8px;
  outline: none;
  transition: border-color 0.2s, box-shadow 0.2s;
  background: #fafbfc;
}

.field-input:focus {
  border-color: #396cd8;
  box-shadow: 0 0 0 3px rgba(57, 108, 216, 0.12);
  background: #fff;
}

.error-msg {
  margin: 0;
  font-size: 13px;
  color: #d64545;
  background: #fdf0f0;
  border: 1px solid #f5d4d4;
  border-radius: 8px;
  padding: 8px 12px;
}

.btn-primary {
  height: 44px;
  font-size: 15px;
  color: #fff;
  background: #396cd8;
  border: none;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.2s, transform 0.1s;
}

.btn-primary:hover {
  background: #2f5bc0;
}

.btn-primary:active {
  transform: translateY(1px);
}

.btn-primary:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.auth-footer {
  margin-top: 20px;
  display: flex;
  justify-content: center;
  align-items: center;
  gap: 10px;
  font-size: 13px;
}

.auth-link {
  color: #396cd8;
  text-decoration: none;
}

.auth-link:hover {
  text-decoration: underline;
}

.auth-divider {
  color: #c3c9d4;
}
</style>
