/**
 * 「扫描入库」模块的分区定义（唯一真值）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么要抽出来（FEAT-151）
 * ──────────────────────────────────────────────────────────────────────────
 * 这个模块有两条呈现路径：
 *   · `/scan`（Hub）：页头分区 + 在页内渲染各分区
 *   · `/scan/test`（相册扫描分组工具）：独立子页，页头同样要显示这一排分区
 *
 * 抽出来之前，两份定义各写一遍，很快就跑偏了：分组工具页上「重复清理」还写着
 * "功能待开发"（实际早已上线），点进去也没有任何高亮，用户读到的是一份过期说明书。
 *
 * 现在：**分区表只有这一份**。两份页头都调 `scanModuleTabs()`，跳转方式也统一 ——
 * 页内分区是状态型 tab，子页分区是路由型 tab（AppHeader 内部渲染成 RouterLink）。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 「按年·地点浏览」为什么被换成了「入库体检」
 * ──────────────────────────────────────────────────────────────────────────
 * 那个占位是早期规划：扫描入库后按年份 / 地点翻看结果。但「按年浏览」与
 * 「按地点浏览」在智慧相册里已经由时间线（/timeline）与地点（/smart?tab=location）
 * 承担，在这里再做一遍就是同一件事的第三个入口，且与"扫描入库"的职责无关。
 *
 * 「入库体检」才是这个模块自己的问题：扫完之后，数据到底干不干净 ——
 * 哪些相册没入库、哪些照片缺缩略图 / 缺向量、哪些记录指向的文件已经不存在。
 * 它与智慧相册（人脸 / 地点 / 内容分类 / 语义搜索）零重叠。
 */
import type { HeaderTab } from "../types/ui";

/** 页内可切换的分区（状态型 tab） */
export type ScanSectionKey = "global" | "dedupe" | "health";

export interface ScanSection {
  key: ScanSectionKey;
  /** 页头导航里的短名（栏内空间紧张，长名会挤掉页面读数） */
  navLabel: string;
  /** 内容区标题用的全名 */
  title: string;
  icon: string;
  /** 一句话说清这块干什么（空态与页头副标题复用） */
  blurb: string;
}

export const SCAN_SECTIONS: ScanSection[] = [
  {
    key: "global",
    navLabel: "全局扫描",
    title: "全局照片扫描入库",
    icon: "🗂️",
    blurb: "批量扫描相册目录，把 EXIF / 影调 / 内容 / 向量写进库",
  },
  {
    key: "dedupe",
    navLabel: "重复清理",
    title: "重复扫描清理",
    icon: "🧹",
    blurb: "全库找重复：内容哈希一致的字节副本 + 画面相似的尺寸/格式副本",
  },
  {
    key: "health",
    navLabel: "入库体检",
    title: "入库体检",
    icon: "🩺",
    blurb: "扫完之后数据干不干净：入库率 / 缩略图 / 向量 / 失效文件与缓存体积",
  },
];

/** 独立子页分区（路由型 tab）：相册扫描分组工具 */
export const SCAN_TOOL_TAB: HeaderTab = { label: "扫描分组工具", to: "/scan/test" };

/** 默认分区（进 Hub 落在哪一块） */
export const DEFAULT_SCAN_SECTION: ScanSectionKey = "global";

/** 某分区的定义 */
export function scanSection(key: string): ScanSection | undefined {
  return SCAN_SECTIONS.find((s) => s.key === key);
}

/**
 * 生成页头分区导航
 *
 * 不接收「当前在哪一块」——高亮由 `AppHeader` 用 `active` / 当前路由自己判定，
 * 分区表保持成一份静态真值（同一个函数结果可以安全地塞进 computed）。
 */
export function scanModuleTabs(): HeaderTab[] {
  return [
    ...SCAN_SECTIONS.map((s) => ({ key: s.key, label: s.navLabel })),
    { ...SCAN_TOOL_TAB },
  ];
}
