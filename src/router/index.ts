import { createRouter, createWebHistory } from "vue-router";
import Home from "../views/Home.vue";
import AlbumList from "../views/AlbumList.vue";
import Login from "../views/Login.vue";
import Register from "../views/Register.vue";
import ForgotPassword from "../views/ForgotPassword.vue";
import { useAuthStore } from "../stores/auth";

/**
 * 路由配置 —— 对应需求 §5.1 页面层级
 *
 * App
 * ├── /login           登录页（无需登录）
 * ├── /register        注册页（无需登录）
 * ├── /forgot-password 忘记密码页（无需登录）
 * ├── /home      主页（功能板块导航）
 * │   └── /albums    相册列表页（相册管理板块）
 * │        └── /album/:id  相册详情页
 *
 * 多用户登录守卫：除登录/注册/忘记密码外，其余页面均需登录后才能访问，
 * 未登录跳转 /login 并携带 redirect 参数，登录后回跳。
 */
const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: "/",
      redirect: "/home",
    },
    {
      path: "/login",
      name: "login",
      component: Login,
      meta: { public: true },
    },
    {
      path: "/register",
      name: "register",
      component: Register,
      meta: { public: true },
    },
    {
      path: "/forgot-password",
      name: "forgot-password",
      component: ForgotPassword,
      meta: { public: true },
    },
    {
      path: "/home",
      name: "home",
      component: Home,
    },
    {
      path: "/albums",
      name: "album-list",
      component: AlbumList,
    },
    {
      // 懒加载图片扫描测试页
      path: "/scan",
      name: "test-scan",
      component: () => import("../views/TestScan.vue"),
    },
    {
      // 智慧相册：人脸/人物识别结果总览（FEAT-030）
      path: "/smart",
      name: "smart-album",
      component: () => import("../views/SmartAlbum.vue"),
    },
    {
      // 跨相册照片时间线（FEAT-033）
      path: "/timeline",
      name: "timeline",
      component: () => import("../views/Timeline.vue"),
    },
    {
      // 懒加载详情页
      path: "/album/:id",
      name: "album-detail",
      component: () => import("../views/AlbumDetail.vue"),
    },
  ],
});

/** 登录守卫：未登录访问受保护页面 → 跳转登录页；已登录访问登录页 → 跳转主页 */
router.beforeEach(async (to) => {
  const auth = useAuthStore();
  if (!auth.checked) {
    await auth.checkSession();
  }
  if (to.meta.public) {
    // 已登录用户再访问登录页 → 直接进主页
    if (auth.user && to.name === "login") {
      return { path: "/home" };
    }
    return true;
  }
  if (!auth.user) {
    return {
      path: "/login",
      query: { redirect: to.fullPath },
    };
  }
  return true;
});

export default router;
