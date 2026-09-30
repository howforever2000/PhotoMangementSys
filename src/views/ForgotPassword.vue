<script setup lang="ts">
import { ref } from "vue";
import { useAuthStore } from "../stores/auth";
import AuthShell from "../components/AuthShell.vue";

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
  <AuthShell title="忘记密码" subtitle="填写账户名、邮箱、手机号校验通过后重设密码">
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

      <p v-if="errorMsg" class="error-msg" role="alert">{{ errorMsg }}</p>
      <p v-if="successMsg" class="success-msg" role="status">{{ successMsg }}</p>

      <button class="btn-primary" type="submit" :disabled="isSubmitting">
        {{ isSubmitting ? "提交中…" : "重置密码" }}
      </button>
    </form>

    <template #footer>
      <span class="footer-text">想起密码了？</span>
      <router-link class="auth-link" to="/login">返回登录</router-link>
    </template>
  </AuthShell>
</template>
