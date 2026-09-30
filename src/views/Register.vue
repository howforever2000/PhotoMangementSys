<script setup lang="ts">
import { ref } from "vue";
import { useRouter } from "vue-router";
import { useAuthStore } from "../stores/auth";
import AuthShell from "../components/AuthShell.vue";

const auth = useAuthStore();
const router = useRouter();

const username = ref("");
const email = ref("");
const phone = ref("");
const password = ref("");
const confirmPassword = ref("");
const errorMsg = ref("");
const isSubmitting = ref(false);

/** 客户端校验（与后端 auth.rs 规则一致，后端仍会二次校验） */
function validate(): string {
  const name = username.value.trim();
  if (name.length < 2 || name.length > 30) {
    return "账户名长度需为 2-30 个字符";
  }
  if (!/^[A-Za-z0-9_\u4e00-\u9fa5]+$/.test(name)) {
    return "账户名只能包含字母、数字、下划线或中文";
  }
  if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email.value.trim())) {
    return "邮箱格式不正确";
  }
  const digits = phone.value.replace(/\D/g, "");
  if (!/^1[3-9]\d{9}$/.test(digits)) {
    return "手机号格式不正确（需为 11 位大陆手机号）";
  }
  if (password.value.length < 6 || password.value.length > 64) {
    return "密码长度需为 6-64 个字符";
  }
  if (password.value !== confirmPassword.value) {
    return "两次输入的密码不一致";
  }
  return "";
}

async function handleRegister() {
  errorMsg.value = validate();
  if (errorMsg.value) return;

  isSubmitting.value = true;
  try {
    // 注册成功自动登录并进入主页
    await auth.register({
      username: username.value.trim(),
      email: email.value.trim(),
      phone: phone.value.trim(),
      password: password.value,
      confirm_password: confirmPassword.value,
    });
    router.replace("/home");
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    isSubmitting.value = false;
  }
}
</script>

<template>
  <AuthShell title="注册新账户" subtitle="每个账户拥有独立的相册空间">
    <form class="auth-form" @submit.prevent="handleRegister">
      <label class="field">
        <span class="field-label">账户名</span>
        <input
          v-model="username"
          class="field-input"
          type="text"
          placeholder="2-30 位字母、数字、下划线或中文"
          autocomplete="username"
        />
      </label>

      <label class="field">
        <span class="field-label">邮箱</span>
        <input
          v-model="email"
          class="field-input"
          type="email"
          placeholder="example@mail.com"
          autocomplete="email"
        />
      </label>

      <label class="field">
        <span class="field-label">手机号</span>
        <input
          v-model="phone"
          class="field-input"
          type="tel"
          placeholder="11 位大陆手机号"
          autocomplete="tel"
        />
      </label>

      <label class="field">
        <span class="field-label">密码</span>
        <input
          v-model="password"
          class="field-input"
          type="password"
          placeholder="6-64 个字符"
          autocomplete="new-password"
        />
      </label>

      <label class="field">
        <span class="field-label">确认密码</span>
        <input
          v-model="confirmPassword"
          class="field-input"
          type="password"
          placeholder="再次输入密码"
          autocomplete="new-password"
        />
      </label>

      <p v-if="errorMsg" class="error-msg" role="alert">{{ errorMsg }}</p>

      <button class="btn-primary" type="submit" :disabled="isSubmitting">
        {{ isSubmitting ? "注册中…" : "注 册" }}
      </button>
    </form>

    <template #footer>
      <span class="footer-text">已有账户？</span>
      <router-link class="auth-link" to="/login">返回登录</router-link>
    </template>
  </AuthShell>
</template>
