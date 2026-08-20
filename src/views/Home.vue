<script setup lang="ts">
import { useRouter } from "vue-router";
import { useAuthStore } from "../stores/auth";

const router = useRouter();
const auth = useAuthStore();

/** 当前登录账户名（未登录时显示默认文案） */
const username = () => auth.user?.username ?? "未登录";

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
    id: "tags",
    title: "标签系统",
    desc: "为图片打标签、评分（待开发）",
    icon: "🏷️",
    ready: false,
  },
] as const;

function openModule(m: (typeof modules)[number]) {
  if (m.ready && m.path) {
    router.push(m.path);
  }
}
</script>

<template>
  <div class="home-page">
    <header class="home-header">
      <div class="header-text">
        <h1 class="app-title">本地相册管理</h1>
        <p class="app-subtitle">轻量级本地相册管理系统</p>
      </div>
      <div class="user-box">
        <span class="user-name">👤 {{ username() }}</span>
        <button class="logout-btn" type="button" @click="handleLogout">退出登录</button>
      </div>
    </header>

    <main class="module-grid">
      <article
        v-for="m in modules"
        :key="m.id"
        class="module-card"
        :class="{ 'module-ready': m.ready, 'module-pending': !m.ready }"
        @click="openModule(m)"
      >
        <div class="module-icon">{{ m.icon }}</div>
        <div class="module-body">
          <h2 class="module-title">
            {{ m.title }}
            <span v-if="!m.ready" class="pending-badge">待开发</span>
          </h2>
          <p class="module-desc">{{ m.desc }}</p>
        </div>
        <div class="module-arrow">
          {{ m.ready ? "进入 →" : "🔒" }}
        </div>
      </article>
    </main>
  </div>
</template>

<style scoped>
.home-page {
  max-width: 1000px;
  margin: 0 auto;
  padding: 40px 24px;
  min-height: 100vh;
}

.home-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 40px;
  text-align: left;
}

.header-text {
  text-align: left;
}

.user-box {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.user-name {
  font-size: 14px;
  color: #4a5568;
}

.logout-btn {
  height: 32px;
  padding: 0 14px;
  font-size: 13px;
  color: #4a5568;
  background: #fff;
  border: 1px solid #d8dce3;
  border-radius: 8px;
  cursor: pointer;
  transition: color 0.2s, border-color 0.2s;
}

.logout-btn:hover {
  color: #d64545;
  border-color: #d64545;
}

.app-title {
  font-size: 32px;
  margin: 0 0 8px;
  color: #2c3e50;
}

.app-subtitle {
  margin: 0;
  color: #888;
  font-size: 15px;
}

.module-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 20px;
}

.module-card {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 20px;
  background: #fff;
  border-radius: 12px;
  border: 1px solid #eee;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.05);
  transition: transform 0.2s, box-shadow 0.2s, border-color 0.2s;
  cursor: pointer;
}

.module-ready:hover {
  transform: translateY(-4px);
  box-shadow: 0 8px 20px rgba(0, 0, 0, 0.12);
  border-color: #396cd8;
}

.module-pending {
  opacity: 0.6;
  cursor: not-allowed;
}

.module-pending:hover {
  transform: none;
}

.module-icon {
  font-size: 32px;
  flex-shrink: 0;
}

.module-body {
  flex: 1;
}

.module-title {
  margin: 0 0 6px;
  font-size: 17px;
  color: #2c3e50;
}

.module-desc {
  margin: 0;
  font-size: 13px;
  color: #888;
  line-height: 1.5;
}

.pending-badge {
  display: inline-block;
  margin-left: 6px;
  padding: 1px 8px;
  font-size: 11px;
  color: #999;
  background: #f0f0f0;
  border-radius: 10px;
  vertical-align: middle;
}

.module-arrow {
  color: #396cd8;
  font-size: 14px;
  flex-shrink: 0;
}
</style>
