<script setup lang="ts">
import { onBeforeUnmount, onMounted, reactive, ref, computed } from "vue";
import { useRouter } from "vue-router";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { useAuthStore } from "../stores/auth";
import { useThemeStore } from "../stores/theme";
import ThemeDialog from "../components/ThemeDialog.vue";

const router = useRouter();
const auth = useAuthStore();
const theme = useThemeStore();

/**
 * 弹窗面板主题样式（--pm-* 变量包）：由 theme store 统一下发（FEAT-086 上收），
 * Home 基本信息弹窗与设置菜单消费；颜色随组件色调/材质实时重推。
 */
const pmStyle = computed(() => theme.dialogVarStyle);

/** 当前登录账户名（未登录时显示默认文案） */
const username = () => auth.user?.username ?? "未登录";

/** 深色模式视觉（与背景样式自由搭配） */
const isDark = computed(() => theme.isDark);

const cardStyle = computed(() => theme.cardStyle);
const badgeStyle = computed(() =>
  isDark.value
    ? { color: "rgba(255,255,255,.85)", background: "rgba(120,120,130,.4)", border: "1px solid rgba(255,255,255,.18)" }
    : { color: "rgba(50,60,80,.85)", background: "rgba(0,0,0,.06)", border: "1px solid rgba(0,0,0,.08)" },
);

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
    desc: "勾选相册批量扫描入库（EXIF/影调/AI），或扫描文件夹按年·地点组织移动",
    icon: "🔍",
    path: "/scan",
    ready: true,
  },
  {
    id: "workshop",
    title: "创意工坊",
    desc: "图片编辑小组件：区域直方图均衡化（框选 / 画蒙版）等",
    icon: "🎨",
    path: "/workshop",
    ready: true,
  },
  {
    id: "smart",
    title: "智慧相册",
    desc: "照片时间线 · 智能搜索 · 批量整理，人物总览，聚合智慧功能（含回忆）",
    icon: "🧠",
    path: "/smart",
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
/** FEAT-045：头像资源 URL 时间戳（覆盖写同路径文件后破 webview 图片缓存） */
const avatarTs = ref(Date.now());

/** 头像 asset URL：附时间戳避免同名覆盖后缓存不刷新 */
function avatarUrl(p: string): string {
  return `${convertFileSrc(p)}?t=${avatarTs.value}`;
}

/** FEAT-045：选本地图片设为头像（后端中心方裁 256×256 落盘并写库） */
async function chooseAvatar() {
  profileError.value = "";
  try {
    const picked = await openFileDialog({
      multiple: false,
      directory: false,
      title: "选择头像图片",
      filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp", "gif"] }],
    });
    if (typeof picked !== "string") return;
    await auth.setAvatar(picked);
    avatarTs.value = Date.now();
    profileSuccess.value = "头像已更新";
    setTimeout(() => (profileSuccess.value = ""), 2000);
  } catch (e) {
    profileError.value = String(e);
  }
}

/** FEAT-045：移除头像（后端删文件 + 置空） */
async function removeAvatar() {
  profileError.value = "";
  try {
    await auth.clearAvatar();
    avatarTs.value = Date.now();
    profileSuccess.value = "已移除头像";
    setTimeout(() => (profileSuccess.value = ""), 2000);
  } catch (e) {
    profileError.value = String(e);
  }
}

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

/* ---------------- 主题/皮肤设置（FEAT-086：弹窗抽为独立组件 ThemeDialog） ---------------- */
const themeOpen = ref(false);

function openTheme() {
  themeOpen.value = true;
}

/* ---------------- 设置菜单（⚙） ---------------- */
const settingsOpen = ref(false);

/** 打开开发者视角日志窗口（后端保证单例：已开则聚焦） */
async function openDevLog() {
  settingsOpen.value = false;
  try {
    await invoke("open_dev_log_window");
  } catch (e) {
    console.error("打开开发者视角窗口失败:", e);
  }
}

/** 打开「数据与路径」副窗口（DB/缓存/模型位置 + 只读预览；后端保证单例） */
async function openDevData() {
  settingsOpen.value = false;
  try {
    await invoke("open_dev_data_window");
  } catch (e) {
    console.error("打开数据与路径窗口失败:", e);
  }
}

/* ---------------- Esc 关闭打开的弹窗（不依赖 mask focus） ---------------- */
function onGlobalKey(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  if (profileOpen.value) {
    e.preventDefault();
    e.stopPropagation();
    profileOpen.value = false;
  } else if (settingsOpen.value) {
    e.preventDefault();
    e.stopPropagation();
    settingsOpen.value = false;
  }
  // themeOpen 的 Esc 由 ThemeDialog 组件自持（capture 监听），这里不重复处理
}
onMounted(() => {
  document.addEventListener("keydown", onGlobalKey);
});
onBeforeUnmount(() => {
  document.removeEventListener("keydown", onGlobalKey);
});
</script>

<template>
  <div
    class="home-page"
    :style="{
      '--card-shadow': 'var(--shadow-2)',
    }"
  >
    <div class="home-content">
      <header class="home-header">
        <div class="header-text">
          <h1 class="app-title">本地相册管理</h1>
          <p class="app-subtitle">轻量级本地相册管理系统</p>
        </div>
        <div class="user-box glass-card" :style="cardStyle">
          <button class="user-chip" type="button" @click="openProfile" :title="'修改基本信息'">
            <img
              v-if="auth.user?.avatar"
              :src="avatarUrl(auth.user.avatar)"
              class="avatar avatar-img"
              alt=""
            />
            <span v-else class="avatar">👤</span>
            <span class="user-name">{{ username() }}</span>
          </button>
          <button
            class="icon-btn"
            type="button"
            title="主题 / 皮肤设置"
            @click="openTheme"
          >
            🎨
          </button>
          <div class="settings-wrap">
            <button
              class="icon-btn"
              type="button"
              title="设置"
              @click="settingsOpen = !settingsOpen"
            >
              ⚙️
            </button>
              <div v-if="settingsOpen" class="settings-menu" :style="pmStyle" role="menu">
                <button class="settings-item" type="button" role="menuitem" @click="openDevLog">
                  <span class="settings-item-icon">🧪</span>
                  <span>
                    <b>开发者视角</b>
                    <i>打开实时日志副窗口</i>
                  </span>
                </button>
                <button class="settings-item" type="button" role="menuitem" @click="openDevData">
                  <span class="settings-item-icon">🗂️</span>
                  <span>
                    <b>数据与路径</b>
                    <i>DB / 缓存 / 模型位置 · 只读预览</i>
                  </span>
                </button>
              </div>
          </div>
          <button class="logout-btn" type="button" @click="handleLogout">退出登录</button>
        </div>
      </header>

      <!--
        设置菜单的「点外部关闭」遮罩（BUG-2026-0918-005）。
        必须渲染在 user-box 之外：user-box 带 backdrop-filter（玻璃态），
        会把 position:fixed 的包含块变成它自己 —— 遮罩就只盖住头像那一小块，
        点页面其他地方关不掉菜单。
        层级：module-card(0) < 遮罩(8) < home-header(20)：既挡住下面的卡片，
        又不会盖住 header 里的菜单本身。
      -->
      <div v-if="settingsOpen" class="settings-mask" @click="settingsOpen = false"></div>

      <main class="module-grid">
        <article
          v-for="m in modules"
          :key="m.id"
          class="module-card glass-card"
          :class="{ 'module-ready': m.ready, 'module-pending': !m.ready }"
          :style="cardStyle"
          @click="openModule(m)"
        >
          <div class="module-icon">{{ m.icon }}</div>
          <div class="module-body">
            <h2 class="module-title">
              {{ m.title }}
              <span v-if="!m.ready" class="pending-badge" :style="badgeStyle">待开发</span>
            </h2>
            <p class="module-desc">{{ m.desc }}</p>
          </div>
          <div class="module-arrow">
            {{ m.ready ? "进入 →" : "🔒" }}
          </div>
        </article>
      </main>
    </div>

    <!-- 基本信息修改弹窗（需输入当前密码） -->
    <teleport to="body">
      <transition name="modal">
        <div v-if="profileOpen" class="pm-modal" @click.self="profileOpen = false">
          <div class="pm-dialog" role="dialog" aria-modal="true" :style="pmStyle">
            <div class="pm-dialog-head">
              <h3 :style="{ color: theme.textColor }">基本信息</h3>
              <span class="pm-hint">修改需输入当前密码</span>
            </div>
            <!-- FEAT-045：头像（选图/移除即时生效，无需密码） -->
            <div class="pm-field">
              <label>头像</label>
              <div class="avatar-edit">
                <img
                  v-if="auth.user?.avatar"
                  :key="auth.user.avatar + avatarTs"
                  :src="avatarUrl(auth.user.avatar)"
                  class="avatar-preview"
                  alt=""
                />
                <span v-else class="avatar-preview avatar-ph">👤</span>
                <button class="pm-btn" type="button" @click="chooseAvatar">选择图片</button>
                <button
                  v-if="auth.user?.avatar"
                  class="pm-btn"
                  type="button"
                  @click="removeAvatar"
                >
                  移除头像
                </button>
              </div>
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

      <!-- 主题 / 皮肤设置弹窗（FEAT-086：抽为独立组件，双页签） -->
      <ThemeDialog v-if="themeOpen" @close="themeOpen = false" />
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
  /* 抬到主内容之上（BUG-2026-0918-005）：user-box 带 backdrop-filter，
     自成层叠上下文；若 header 不抬层，⚙ 下拉菜单会被后面的 module-card
     （同样带 backdrop-filter、且 DOM 在后）盖住，看不见也点不到 */
  position: relative;
  z-index: 20;
}

.header-text {
  text-align: left;
}

.app-title {
  font-size: 32px;
  font-weight: 700;
  margin: 0 0 8px;
  letter-spacing: 0.5px;
  /* 不靠模糊/阴影"假装通透"，靠字重 + 字距 + 颜色对比 */
  /* BUG-2026-0919-004：标题直接落在页面背景上，用 on-bg 对比色（深色模式+浅背景也可读） */
  color: var(--color-on-bg, var(--color-text));
}

.app-subtitle {
  margin: 0;
  font-size: 15px;
  color: var(--color-on-bg-2, var(--color-text-2));
}

.user-box {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
  padding: 6px 12px;
  /* blur 半径过大（12-16px）会让文字下方的背景"糊一片"，反而发雾。
     改为 6px，仍保留玻璃质感但不再拖累文字。 */
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
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
  /* 黑色回退字形（无 emoji 字体时）在深色玻璃上不可读 */
  color: var(--color-text);
}

/* FEAT-045：用户头像（user-chip 圆形小图 + 弹窗预览） */
.avatar-img {
  width: 26px;
  height: 26px;
  border-radius: 50%;
  object-fit: cover;
  display: block;
}
.avatar-edit {
  display: flex;
  align-items: center;
  gap: 10px;
}
.avatar-preview {
  width: 64px;
  height: 64px;
  border-radius: 50%;
  object-fit: cover;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(127, 127, 127, 0.12);
}
.avatar-ph {
  font-size: 26px;
}

.user-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--color-text);
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
  color: var(--color-text);
}

.icon-btn:hover {
  background: rgba(120, 120, 130, 0.18);
}

/* 设置下拉菜单（⚙）：菜单锚定 user-box，遮罩负责点击外部关闭 */
.settings-wrap {
  position: relative;
  display: inline-flex;
}

/* 遮罩在模板里渲染于 header 之外（那里没有 backdrop-filter 祖先，
   position:fixed 才真正按视口铺满）；z-index 8 = 卡片(0) 之上、header(20) 之下 */
.settings-mask {
  position: fixed;
  inset: 0;
  z-index: 8;
}

.settings-menu {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  z-index: 901;
  min-width: 220px;
  padding: 6px;
  border-radius: 12px;
  box-shadow: 0 14px 40px rgba(0, 0, 0, 0.28);
}

.settings-item {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 9px 10px;
  text-align: left;
  background: transparent;
  border: none;
  border-radius: 9px;
  cursor: pointer;
  transition: background 0.15s;
}

.settings-item:hover {
  background: var(--pm-btn-hover);
}

.settings-item-icon {
  font-size: 17px;
}

.settings-item b {
  display: block;
  font-size: 13.5px;
  font-weight: 600;
  color: var(--pm-text);
}

.settings-item i {
  display: block;
  font-style: normal;
  font-size: 11.5px;
  color: var(--pm-hint);
  margin-top: 1px;
}

.logout-btn {
  height: 30px;
  padding: 0 14px;
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text);
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
  /* 玻璃材质（模糊/高光/阴影）由 :style="cardStyle" 下发，不再写死 blur */
  border-radius: var(--radius-card);
  transition: transform 0.2s, box-shadow 0.2s, border-color 0.2s, background 0.2s;
  cursor: pointer;
}

.module-card:hover {
  box-shadow: var(--card-shadow);
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
  font-weight: 600;
  color: var(--color-text);
}

.module-desc {
  margin: 0;
  font-size: 13px;
  line-height: 1.5;
  color: var(--color-text-2);
}

.pending-badge {
  display: inline-block;
  margin-left: 8px;
  padding: 1px 8px;
  font-size: 11px;
  font-weight: 600;
  border-radius: 10px;
  vertical-align: middle;
}

.module-arrow {
  font-size: 14px;
  flex-shrink: 0;
  white-space: nowrap;
  /* 链接色 + 加重：灰色箭头在深绿玻璃上存在感太弱，行动入口应该一眼可见 */
  color: var(--color-link);
  font-weight: 600;
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
