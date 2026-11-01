/**
 * 通用 UI 契约类型（与具体组件解耦）
 *
 * 为什么单独放一份：`HeaderTab` 原本长在 `AppHeader.vue` 的 `<script setup>` 里，
 * 于是 `.ts` 文件（如 `utils/scanModuleTabs.ts`）想复用这个类型就必须从 `.vue`
 * 里 import 具名类型 —— 而 `vite-env.d.ts` 的 `*.vue` 声明只给默认导出，
 * 在纯 TS 模块里拿到的是 `DefineComponent`，具名类型并不稳定。
 *
 * 把契约挪进 `types/` 后：组件与纯逻辑模块都从这里 import，方向单一（都指向 types）。
 */

/** 大组件内部的分区导航项（`AppHeader` 的 tabs 契约） */
export interface HeaderTab {
  /** 稳定 key（状态型分区用） */
  key?: string;
  label: string;
  /** 计数徽标 */
  count?: number | string;
  /** 跳转路由（路由型分区用，如 /scan/test） */
  to?: string;
  /** 置灰但可见（"有这块功能，当前条件不可用"） */
  disabled?: boolean;
  /** 置灰原因，写进 title —— 灰掉必须讲原因 */
  why?: string;
}
