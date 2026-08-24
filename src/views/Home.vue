<script setup lang="ts">
import { reactive, ref, computed } from "vue";
import { useRouter } from "vue-router";
import { useAuthStore } from "../stores/auth";
import { useThemeStore } from "../stores/theme";

const router = useRouter();
const auth = useAuthStore();
const theme = useThemeStore();

/** 当前登录账户名（未登录时显示默认文案） */
const username = () => auth.user?.username ?? "未登录";

/** 深色模式视觉（与背景样式自由搭配） */
const isDark = computed(() => theme.isDark);

const titleColor = computed(() => theme.textColor);
const subtitleColor = computed(() => theme.subTextColor);
const cardStyle = computed(() => theme.cardStyle);
const cardTitleColor = computed(() => theme.textColor);
const cardDescColor = computed(() => theme.subTextColor);
const badgeStyle = computed(() =>
  isDark.value
    ? { color: "rgba(255,255,255,.85)", background: "rgba(120,120,130,.4)", border: "1px solid rgba(255,255,255,.18)" }
    : { color: "rgba(50,60,80,.85)", background: "rgba(0,0,0,.06)", border: "1px solid rgba(0,0,0,.08)" },
);
const arrowColor = computed(() => (isDark.value ? "#bcd0ff" : "#3a6cf5"));

/** 退出登录并回到登录页 */
async function handleLogout() {
  try {
    await auth.logout();
  } catch (e) {
    console.error("退出登录失败:", e);
  }
  router.replace("/login");
}

/** 应用功能板块定义 */
const modules = [
  {
    id: "albums",
    title: "相册管理",
    desc: "创建相册、绑定本地文件夹、设置封面",
    icon: "📁",
    path: "/albums",
    ready: true,
  },
  {
    id: "scan",
    title: "图片扫描",
    desc: "扫描文件夹内图片，按时间/地点排序，按年·地点组织移动",
    icon: "🔍",
    path: "/scan",
    ready: true,
  },
  {
    id: "process",
    title: "图像处理",
    desc: "直方图均衡化、CLAHE 等算法（待开发）",
    icon: "🎨",
    ready: false,
  },
  {
    id: "smart",
    title: "智慧相册",
    desc: "人脸/人物识别结果总览：头像、命名与脸数一览",
    icon: "🧠",
    path: "/smart",
    ready: true,
  },
  {
    id: "timeline",
    title: "照片时间线",
    desc: "跨相册按拍摄时间聚合浏览，重现回忆旅程",
    icon: "📅",
    path: "/timeline",
    ready: true,
  },
  {
    id: "search",
    title: "智能搜索",
    desc: "自然语言 + 多维筛选，跨相册检索照片",
    icon: "🔎",
    path: "/search",
    ready: true,
  },
] as const;

function openModule(m: (typeof modules)[number]) {
  if (m.ready && m.path) {
    router.push(m.path);
  }
}

/* ---------------- 基本信息修改（需先验证当前密码） ---------------- */
const profileOpen = ref(false);
const profileForm = reactive({ email: "", phone: "", current_password: "" });
const profileError = ref("");
const profileSuccess = ref("");

function openProfile() {
  profileForm.email = auth.user?.email ?? "";
  profileForm.phone = auth.user?.phone ?? "";
  profileForm.current_password = "";
  profileError.value = "";
  profileSuccess.value = "";
  profileOpen.value = true;
}

async function submitProfile() {
  profileError.value = "";
  profileSuccess.value = "";
  if (!profileForm.email.trim()) return (profileError.value = "邮箱不能为空");
  if (!profileForm.current_password) return (profileError.value = "请输入当前密码以确认修改");
  try {
    const updated = await auth.updateProfile({
      email: profileForm.email.trim(),
      phone: profileForm.phone.trim(),
      current_password: profileForm.current_password,
    });
    profileSuccess.value = "已更新，新邮箱/手机号即时生效";
    setTimeout(() => {
      profileOpen.value = false;
      console.log("profile updated", updated);
    }, 900);
  } catch (e) {
    profileError.value = String(e);
  }
}

/* ---------------- 主题/皮肤设置 ---------------- */
const themeOpen = ref(false);
const bgFileInput = ref<HTMLInputElement | null>(null);

function openTheme() {
  themeOpen.value = true;
}

function chooseBgImage() {
  bgFileInput.value?.click();
}

/** 压缩选中的图片：最长边限制 1920px、JPEG 0.85，避免 data URL 撑爆 localStorage */
function compressImage(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const url = URL.createObjectURL(file);
    const img = new Image();
    img.onload = () => {
      const max = 1920;
      const scale = Math.min(1, max / Math.max(img.width, img.height));
      const w = Math.max(1, Math.round(img.width * scale));
      const h = Math.max(1, Math.round(img.height * scale));
      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      canvas.getContext("2d")!.drawImage(img, 0, 0, w, h);
      URL.revokeObjectURL(url);
      resolve(canvas.toDataURL("image/jpeg", 0.85));
    };
    img.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("图片读取失败"));
    };
    img.src = url;
  });
}

async function onBgFile(e: Event) {
  const input = e.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  try {
    const data = await compressImage(file);
    theme.saveImage(data);
    theme.bgStyle = "image";
    theme.persist();
  } catch (err) {
    console.error("背景图设置失败:", err);
  }
  input.value = "";
}

function clearBgImage() {
  theme.saveImage("");
  if (theme.bgStyle === "image") {
    theme.bgStyle = "color";
    theme.persist();
  }
}

const themeOptions = [
  { id: "image", label: "背景图" },
  { id: "gradient", label: "渐变色" },
  { id: "color", label: "纯色" },
] as const;

function setBgStyle(v: string) {
  theme.bgStyle = v as "image" | "gradient" | "color";
  theme.persist();
}
</script>

<template>
  <div class="home-page">
    <div class="home-content">
      <header class="home-header">
        <div class="header-text">
          <h1 class="app-title" :style="{ color: titleColor }">本地相册管理</h1>
          <p class="app-subtitle" :style="{ color: subtitleColor }">轻量级本地相册管理系统</p>
        </div>
        <div class="user-box" :style="cardStyle">
          <button class="user-chip" type="button" @click="openProfile" :title="'修改基本信息'">
            <span class="avatar">👤</span>
            <span class="user-name" :style="{ color: cardTitleColor }">{{ username() }}</span>
          </button>
          <button
            class="icon-btn"
            type="button"
            title="主题 / 皮肤设置"
            :style="{ color: cardTitleColor }"
            @click="openTheme"
          >
            🎨
          </button>
          <button class="logout-btn" type="button" @click="handleLogout">退出登录</button>
        </div>
      </header>

      <main class="module-grid">
        <article
          v-for="m in modules"
          :key="m.id"
          class="module-card"
          :class="{ 'module-ready': m.ready, 'module-pending': !m.ready }"
          :style="cardStyle"
          @click="openModule(m)"
        >
          <div class="module-icon">{{ m.icon }}</div>
          <div class="module-body">
            <h2 class="module-title" :style="{ color: cardTitleColor }">
              {{ m.title }}
              <span v-if="!m.ready" class="pending-badge" :style="badgeStyle">待开发</span>
            </h2>
            <p class="module-desc" :style="{ color: cardDescColor }">{{ m.desc }}</p>
          </div>
          <div class="module-arrow" :style="{ color: arrowColor }">
            {{ m.ready ? "进入 →" : "🔒" }}
          </div>
        </article>
      </main>
    </div>

    <!-- 基本信息修改弹窗（需输入当前密码） -->
    <teleport to="body">
      <transition name="modal">
        <div v-if="profileOpen" class="pm-modal" @click.self="profileOpen = false">
          <div class="pm-dialog" role="dialog" aria-modal="true">
            <div class="pm-dialog-head">
              <h3>基本信息</h3>
              <span class="pm-hint">修改需输入当前密码</span>
            </div>
            <div class="pm-field">
              <label>用户名</label>
              <input :value="username()" disabled />
            </div>
            <div class="pm-field">
              <label>邮箱</label>
              <input v-model="profileForm.email" type="email" placeholder="用于登录 / 找回密码" />
            </div>
            <div class="pm-field">
              <label>手机号</label>
              <input v-model="profileForm.phone" placeholder="用于找回密码" />
            </div>
            <div class="pm-field">
              <label>当前密码</label>
              <input v-model="profileForm.current_password" type="password" placeholder="验证身份后才能修改" />
            </div>
            <p v-if="profileError" class="pm-error">{{ profileError }}</p>
            <p v-if="profileSuccess" class="pm-ok">{{ profileSuccess }}</p>
            <div class="pm-actions">
              <button class="pm-btn" type="button" @click="profileOpen = false">取消</button>
              <button class="pm-btn pm-btn-primary" type="button" @click="submitProfile">保存修改</button>
            </div>
          </div>
        </div>
      </transition>

      <!-- 主题 / 皮肤设置弹窗 -->
      <transition name="modal">
        <div v-if="themeOpen" class="pm-modal" @click.self="themeOpen = false">
          <div class="pm-dialog" role="dialog" aria-modal="true">
            <div class="pm-dialog-head">
              <h3>主题 / 皮肤</h3>
              <span class="pm-hint">主页与各页共用，设置自动保存</span>
            </div>

            <div class="pm-section">
              <div class="pm-section-title">基础色调</div>
              <div class="pm-seg">
                <button
                  type="button"
                  :class="{ on: theme.mode === 'light' }"
                  @click="theme.setMode('light')"
                >
                  浅色
                </button>
                <button
                  type="button"
                  :class="{ on: theme.mode === 'dark' }"
                  @click="theme.setMode('dark')"
                >
                  深色
                </button>
              </div>
            </div>

            <div class="pm-section">
              <div class="pm-section-title">背景样式</div>
              <div class="pm-seg">
                <button
                  v-for="opt in themeOptions"
                  :key="opt.id"
                  type="button"
                  :class="{ on: theme.bgStyle === opt.id }"
                  @click="setBgStyle(opt.id)"
                >
                  {{ opt.label }}
                </button>
              </div>
            </div>

            <div v-if="theme.bgStyle === 'color'" class="pm-section">
              <label class="pm-section-title">选择颜色</label>
              <div class="pm-color-row">
                <input type="color" v-model="theme.bgColor" @change="theme.persist()" />
                <span class="pm-mono">{{ theme.bgColor }}</span>
              </div>
            </div>

            <div v-else-if="theme.bgStyle === 'gradient'" class="pm-section">
              <div class="pm-section-title">渐变配色</div>
              <div class="pm-grade">
                <input type="color" v-model="theme.gradFrom" @change="theme.persist()" />
                <input type="color" v-model="theme.gradTo" @change="theme.persist()" />
              </div>
              <label class="pm-range">
                <span>角度</span>
                <input type="range" v-model.number="theme.gradAngle" min="0" max="360" @change="theme.persist()" />
                <b>{{ theme.gradAngle }}°</b>
              </label>
            </div>

            <div v-else class="pm-section">
              <div class="pm-section-title">背景图与透明度</div>
              <button class="pm-btn" type="button" @click="chooseBgImage">选择本地图片</button>
              <input ref="bgFileInput" type="file" accept="image/*" class="pm-hidden-input" @change="onBgFile" />
              <button
                v-if="theme.bgImage"
                class="pm-btn pm-btn-clear"
                type="button"
                @click="clearBgImage"
              >
                清除背景图
              </button>
              <label class="pm-range">
                <span>透明度</span>
                <input type="range" v-model.number="theme.bgOpacity" min="0.05" max="1" step="0.05" @change="theme.persist()" />
                <b>{{ Math.round(theme.bgOpacity * 100) }}%</b>
              </label>
              <div
                v-if="theme.bgImage"
                class="pm-preview"
                :style="{ backgroundImage: `url(${theme.bgImage})` }"
              ></div>
            </div>

            <div class="pm-actions">
              <button class="pm-btn" type="button" @click="theme.reset()">恢复默认</button>
              <button class="pm-btn pm-btn-primary" type="button" @click="themeOpen = false">完成</button>
            </div>
          </div>
        </div>
      </transition>
    </teleport>
  </div>
</template>

<style scoped>
.home-page {
  position: relative;
  min-height: 100vh;
}

/* 主页不再自备封面：背景由 App.vue 全局主题层提供（纯色/渐变/背景图+透明度） */

.home-content {
  position: relative;
  z-index: 2;
  max-width: 1000px;
  margin: 0 auto;
  padding: 48px 24px 64px;
  min-height: 100vh;
}

.home-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 48px;
}

.header-text {
  text-align: left;
}

.app-title {
  font-size: 32px;
  margin: 0 0 8px;
  letter-spacing: 0.5px;
}

.app-subtitle {
  margin: 0;
  font-size: 15px;
}

.user-box {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
  padding: 6px 12px;
  backdrop-filter: blur(12px);
  border-radius: 999px;
}

.user-chip {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 4px 6px;
  background: transparent;
  border: none;
  border-radius: 8px;
  cursor: pointer;
}

.avatar {
  font-size: 16px;
}

.user-name {
  font-size: 14px;
  font-weight: 600;
}

.icon-btn {
  width: 32px;
  height: 32px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 15px;
  background: transparent;
  border: none;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.2s;
}

.icon-btn:hover {
  background: rgba(120, 120, 130, 0.18);
}

.logout-btn {
  height: 30px;
  padding: 0 14px;
  font-size: 13px;
  color: inherit;
  background: rgba(120, 120, 130, 0.12);
  border: 1px solid rgba(120, 120, 130, 0.22);
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.2s, border-color 0.2s;
}

.logout-btn:hover {
  background: rgba(255, 80, 80, 0.18);
  border-color: rgba(255, 120, 120, 0.5);
}

.module-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 20px;
}

.module-card {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 22px 20px;
  backdrop-filter: blur(14px);
  border-radius: var(--radius-lg);
  transition: transform 0.2s, box-shadow 0.2s, border-color 0.2s, background 0.2s;
  cursor: pointer;
}

.module-card:hover {
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.4);
  border-color: rgba(140, 180, 255, 0.45);
}

.module-ready:hover {
  transform: translateY(-4px);
}

.module-pending {
  opacity: 0.62;
  cursor: not-allowed;
}

.module-pending:hover {
  transform: none;
}

.module-icon {
  font-size: 34px;
  flex-shrink: 0;
  filter: drop-shadow(0 2px 6px rgba(0, 0, 0, 0.4));
}

.module-body {
  flex: 1;
}

.module-title {
  margin: 0 0 6px;
  font-size: 18px;
}

.module-desc {
  margin: 0;
  font-size: 13px;
  line-height: 1.5;
}

.pending-badge {
  display: inline-block;
  margin-left: 8px;
  padding: 1px 8px;
  font-size: 11px;
  border-radius: 10px;
  vertical-align: middle;
}

.module-arrow {
  font-size: 14px;
  flex-shrink: 0;
  white-space: nowrap;
}

@media (max-width: 640px) {
  .module-grid {
    grid-template-columns: 1fr;
  }
  .home-content {
    padding: 32px 16px 48px;
  }
  .home-header {
    flex-direction: column;
    align-items: flex-start;
    gap: 16px;
  }
}
</style>

<style>
/* 弹窗样式（非 scoped，便于 teleport 到 body 后生效） */
.pm-modal {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
  background: rgba(10, 14, 28, 0.45);
  backdrop-filter: blur(4px);
}
.pm-dialog {
  width: min(440px, 100%);
  max-height: 86vh;
  overflow-y: auto;
  background: #fff;
  border-radius: 18px;
  padding: 24px 24px 20px;
  box-shadow: 0 24px 64px rgba(0, 0, 0, 0.35);
}
.pm-dialog-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 18px;
}
.pm-dialog-head h3 {
  margin: 0;
  font-size: 18px;
  color: #1f2733;
}
.pm-hint {
  font-size: 12px;
  color: #8a93a6;
}
.pm-field {
  margin-bottom: 14px;
}
.pm-field label {
  display: block;
  font-size: 13px;
  color: #5a6474;
  margin-bottom: 6px;
}
.pm-field input {
  width: 100%;
  height: 40px;
  padding: 0 12px;
  font-size: 14px;
  color: #1f2733;
  background: #f4f6f9;
  border: 1px solid #e2e6ee;
  border-radius: 10px;
  outline: none;
  transition: border-color 0.2s, box-shadow 0.2s;
}
.pm-field input:focus {
  border-color: #5a8bf7;
  box-shadow: 0 0 0 3px rgba(90, 139, 247, 0.15);
}
.pm-field input:disabled {
  color: #98a0b0;
  background: #eef1f5;
}
.pm-error {
  margin: 4px 0 10px;
  font-size: 13px;
  color: #e5484d;
}
.pm-ok {
  margin: 4px 0 10px;
  font-size: 13px;
  color: #2f9e44;
}
.pm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 8px;
}
.pm-btn {
  height: 38px;
  padding: 0 18px;
  font-size: 14px;
  color: #4a5568;
  background: #f0f2f6;
  border: 1px solid #e2e6ee;
  border-radius: 10px;
  cursor: pointer;
  transition: background 0.2s;
}
.pm-btn:hover {
  background: #e6e9f0;
}
.pm-btn-primary {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}
.pm-btn-primary:hover {
  background: #2f5de0;
}
.pm-btn-clear {
  margin-left: 8px;
  color: #e5484d;
}
.pm-btn-clear:hover {
  background: #fdf0f0;
}
.pm-section {
  margin-bottom: 16px;
}
.pm-section-title {
  display: block;
  font-size: 13px;
  font-weight: 600;
  color: #5a6474;
  margin-bottom: 8px;
}
.pm-seg {
  display: flex;
  gap: 8px;
}
.pm-seg button {
  flex: 1;
  height: 34px;
  font-size: 13px;
  color: #5a6474;
  background: #f4f6f9;
  border: 1px solid #e2e6ee;
  border-radius: 10px;
  cursor: pointer;
  transition: all 0.2s;
}
.pm-seg button.on {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}
.pm-color-row {
  display: flex;
  align-items: center;
  gap: 10px;
}
.pm-color-row input[type="color"] {
  width: 52px;
  height: 40px;
  padding: 2px;
  border: 1px solid #e2e6ee;
  border-radius: 10px;
  background: #fff;
  cursor: pointer;
}
.pm-mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 13px;
  color: #5a6474;
}
.pm-grade {
  display: flex;
  gap: 12px;
}
.pm-grade input[type="color"] {
  width: 52px;
  height: 40px;
  padding: 2px;
  border: 1px solid #e2e6ee;
  border-radius: 10px;
  background: #fff;
  cursor: pointer;
}
.pm-range {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 13px;
  color: #5a6474;
  margin-top: 12px;
}
.pm-range input[type="range"] {
  flex: 1;
}
.pm-range b {
  min-width: 40px;
  text-align: right;
  color: #1f2733;
}
.pm-hidden-input {
  display: none;
}
.pm-preview {
  height: 110px;
  border-radius: 12px;
  background-size: cover;
  background-position: center;
  margin-top: 12px;
  border: 1px solid #e2e6ee;
}

.modal-enter-active,
.modal-leave-active {
  transition: opacity 0.22s ease;
}
.modal-enter-active .pm-dialog,
.modal-leave-active .pm-dialog {
  transition: transform 0.22s ease, opacity 0.22s ease;
}
.modal-enter-from,
.modal-leave-to {
  opacity: 0;
}
.modal-enter-from .pm-dialog,
.modal-leave-to .pm-dialog {
  transform: translateY(14px) scale(0.98);
  opacity: 0;
}
</style>
