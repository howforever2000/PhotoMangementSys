import { createRouter, createWebHistory } from "vue-router";
import Home from "../views/Home.vue";
import AlbumList from "../views/AlbumList.vue";

/**
 * 路由配置 —— 对应需求 §5.1 页面层级
 *
 * App
 * ├── /home      主页（功能板块导航）
 * │   └── /albums    相册列表页（相册管理板块）
 * │        └── /album/:id  相册详情页
 */
const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: "/",
      redirect: "/home",
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
      // 懒加载详情页
      path: "/album/:id",
      name: "album-detail",
      component: () => import("../views/AlbumDetail.vue"),
    },
  ],
});

export default router;
