<script setup lang="ts">
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useAuthStore } from "../stores/auth";
// 登录页固定使用设计封面（不随主题/皮肤变化）
import coverImg from "../../covers/cover.png";

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
    <div class="auth-cover" :style="{ backgroundImage: `url(${coverImg})` }"></div>
    <div class="auth-overlay"></div>
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
  position: relative;
  min-height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  overflow: hidden;
}

/* 固定设计封面：登录页专属，不受主题设置影响 */
.auth-cover {
  position: absolute;
  inset: 0;
  z-index: 0;
  background-size: cover;
  background-position: center;
}

.auth-overlay {
  position: absolute;
  inset: 0;
  z-index: 1;
  background:
    radial-gradient(120% 120% at 20% 0%, rgba(20, 28, 64, 0.55) 0%, rgba(8, 12, 28, 0.86) 100%),
    linear-gradient(180deg, rgba(8, 12, 28, 0.78) 0%, rgba(8, 12, 28, 0.9) 100%);
}

.auth-card {
  position: relative;
  z-index: 2;
  width: 100%;
  max-width: 400px;
  /* backdrop-filter 半径过大（16px）会让覆盖层下文字发糊，这里降到 8px */
  background: rgba(255, 255, 255, 0.1);
  border: 1px solid rgba(255, 255, 255, 0.18);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  border-radius: 16px;
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  padding: 36px 32px 28px;
}

.auth-header {
  text-align: center;
  margin-bottom: 28px;
}

.auth-title {
  margin: 0 0 8px;
  font-size: 24px;
  font-weight: 700;
  color: #f5f7ff;
  /* 替换原本"靠阴影发光"的做法：用 text-stroke + 微阴影，笔画边缘清晰。
     注意：-webkit-text-stroke 在 Win/Chrome 上对中文支持有限，阴影作为兜底。 */
  -webkit-text-stroke: 0.3px rgba(255, 255, 255, 0.4);
  text-shadow:
    0 0 14px rgba(120, 160, 255, 0.35),
    0 1px 2px rgba(0, 0, 0, 0.4);
  letter-spacing: 0.5px;
}

.auth-subtitle {
  margin: 0;
  font-size: 13px;
  color: rgba(225, 232, 255, 0.82);
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
  color: #f0f3fb;
  font-weight: 500;
}

.field-input {
  height: 42px;
  padding: 0 12px;
  font-size: 14px;
  /* 输入框背景由灰白改为更纯的白，避免在玻璃面板上发灰 */
  color: #1f2733;
  border: 1px solid rgba(255, 255, 255, 0.32);
  border-radius: 8px;
  outline: none;
  transition: border-color 0.2s, box-shadow 0.2s, background 0.2s;
  background: #ffffff;
}

.field-input::placeholder {
  color: #98a2b3;
}

.field-input:focus {
  border-color: #6ea8ff;
  box-shadow: 0 0 0 3px rgba(110, 168, 255, 0.28);
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
  font-weight: 600;
  color: #fff;
  background: #396cd8;
  border: none;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.2s, transform 0.1s;
  letter-spacing: 1px;
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
  color: #aacbff;
  text-decoration: none;
  font-weight: 500;
}

.auth-link:hover {
  text-decoration: underline;
}

.auth-divider {
  color: rgba(214, 221, 240, 0.5);
}
</style>
