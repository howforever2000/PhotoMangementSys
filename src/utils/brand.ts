/**
 * 品牌文案单一事实源（FEAT-189）
 *
 * 为什么单独一个模块：品牌名与标语要在 4 处出现 —— 首页页头、登录页页头、
 * 浏览器 `<title>`、桌面窗口标题。散着写必然漂移（本项目已有先例：
 * `PhotoActionMenu.vue` 四个画廊各写一份右键菜单导致条目与行为都不一样）。
 * 前两处由 `BrandMark.vue` 消费这里的常量；后两处在 `index.html` /
 * `src-tauri/tauri.conf.json` 里是静态文本，改文案时**必须同步这两处**。
 *
 * 中英关系：中文是主名（「栖影」），拉丁是辅助标识（「LumaPerch」）——
 * 不并列成两个名字，所以 BRAND_FULL 用空格连接、且只在需要单行文本的地方
 * （窗口标题 / 浏览器标签）使用；页头由 BrandMark 做「中文大 + 拉丁小」的主次版式。
 */

/** 中文主名 */
export const BRAND_CN = "栖影";

/** 拉丁辅助标识（页头里按 uppercase + 字距 排版，源码保持原大小写便于阅读） */
export const BRAND_EN = "LumaPerch";

/** 单行全名：只给需要纯文本的场合（<title>、窗口标题、无障碍标签） */
export const BRAND_FULL = `${BRAND_CN} ${BRAND_EN}`;

/** 中文标语 */
export const SLOGAN_CN = "影栖本机，静享私藏。";

/** 英文标语（与中文同义，不是直译腔） */
export const SLOGAN_EN = "Your memories perch locally.";
