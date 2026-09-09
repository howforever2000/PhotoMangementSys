<script setup lang="ts">
import { ref } from "vue";
import { useAuthStore } from "../stores/auth";

const auth = useAuthStore();

const username = ref("");
const email = ref("");
const phone = ref("");
const newPassword = ref("");
const confirmPassword = ref("");
const errorMsg = ref("");
const successMsg = ref("");
const isSubmitting = ref(false);

/** 客户端校验（与后端 auth.rs 规则一致） */
function validate(): string {
  const name = username.value.trim();
  if (name.length < 2 || name.length > 30) {
    return "账户名长度需为 2-30 个字符";
  }
  if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email.value.trim())) {
    return "邮箱格式不正确";
  }
  const digits = phone.value.replace(/\D/g, "");
  if (!/^1[3-9]\d{9}$/.test(digits)) {
    return "手机号格式不正确（需为 11 位大陆手机号）";
  }
  if (newPassword.value.length < 6 || newPassword.value.length > 64) {
    return "新密码长度需为 6-64 个字符";
  }
  if (newPassword.value !== confirmPassword.value) {
    return "两次输入的新密码不一致";
  }
  return "";
}

async function handleReset() {
  errorMsg.value = "";
  successMsg.value = "";
  const err = validate();
  if (err) {
    errorMsg.value = err;
    return;
  }

  isSubmitting.value = true;
  try {
    await auth.resetPassword({
      username: username.value.trim(),
      email: email.value.trim(),
      phone: phone.value.trim(),
      new_password: newPassword.value,
      confirm_password: confirmPassword.value,
    });
    successMsg.value = "密码重置成功，请使用新密码登录";
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
        <h1 class="auth-title">忘记密码</h1>
        <p class="auth-subtitle">填写账户名、邮箱、手机号校验通过后重设密码</p>
      </header>

      <form class="auth-form" @submit.prevent="handleReset">
        <label class="field">
          <span class="field-label">账户名</span>
          <input
            v-model="username"
            class="field-input"
            type="text"
            placeholder="注册时的账户名"
            autocomplete="username"
          />
        </label>

        <label class="field">
          <span class="field-label">邮箱</span>
          <input
            v-model="email"
            class="field-input"
            type="email"
            placeholder="注册时的邮箱"
            autocomplete="email"
          />
        </label>

        <label class="field">
          <span class="field-label">手机号</span>
          <input
            v-model="phone"
            class="field-input"
            type="tel"
            placeholder="注册时的手机号"
            autocomplete="tel"
          />
        </label>

        <label class="field">
          <span class="field-label">新密码</span>
          <input
            v-model="newPassword"
            class="field-input"
            type="password"
            placeholder="6-64 个字符"
            autocomplete="new-password"
          />
        </label>

        <label class="field">
          <span class="field-label">确认新密码</span>
          <input
            v-model="confirmPassword"
            class="field-input"
            type="password"
            placeholder="再次输入新密码"
            autocomplete="new-password"
          />
        </label>

        <p v-if="errorMsg" class="error-msg">{{ errorMsg }}</p>
        <p v-if="successMsg" class="success-msg">{{ successMsg }}</p>

        <button class="btn-primary" type="submit" :disabled="isSubmitting">
          {{ isSubmitting ? "提交中…" : "重置密码" }}
        </button>
      </form>

      <footer class="auth-footer">
        <span class="footer-text">想起密码了？</span>
        <router-link class="auth-link" to="/login">返回登录</router-link>
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
  max-width: 420px;
  background: #fff;
  border-radius: 16px;
  border: 1px solid #eceef2;
  box-shadow: 0 12px 40px rgba(30, 41, 59, 0.1);
  padding: 36px 32px 28px;
}

.auth-header {
  text-align: center;
  margin-bottom: 24px;
}

.auth-title {
  margin: 0 0 8px;
  font-size: 24px;
  font-weight: 700;
  color: var(--color-text);
  letter-spacing: 0.5px;
}

.auth-subtitle {
  margin: 0;
  font-size: 13px;
  color: var(--color-text-2);
}

.auth-form {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.field-label {
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text-2);
}

.field-input {
  height: 42px;
  padding: 0 12px;
  font-size: 14px;
  color: var(--color-text);
  border: 1px solid var(--color-border);
  border-radius: 8px;
  outline: none;
  transition: border-color 0.2s, box-shadow 0.2s;
  background: var(--color-surface);
}

.field-input::placeholder {
  color: #98a2b3;
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

.success-msg {
  margin: 0;
  font-size: 13px;
  color: #1a7f37;
  background: #eefaf1;
  border: 1px solid #cdeeda;
  border-radius: 8px;
  padding: 8px 12px;
}

.btn-primary {
  height: 44px;
  font-size: 15px;
  font-weight: 600;
  letter-spacing: 1px;
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
  margin-top: 18px;
  display: flex;
  justify-content: center;
  align-items: center;
  gap: 8px;
  font-size: 13px;
}

.footer-text {
  color: var(--color-text-2);
}

.auth-link {
  color: #396cd8;
  text-decoration: none;
  font-weight: 500;
}

.auth-link:hover {
  text-decoration: underline;
}
</style>
