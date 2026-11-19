/**
 * 用系统默认浏览器打开外部链接（全仓唯一封装点）
 *
 * 为什么单独一个模块：`utils/modelOfflinePackage.ts` 里原本内联了同一段
 * 「动态 import opener → openUrl」；打赏入口是第二个消费方，再抄一遍就是第三份实现
 * （本项目已有先例：`PhotoActionMenu` 被四个画廊各写一份，条目与行为都漂了）。
 *
 * 为什么是**动态** import：只有用户真的点「打开」时才加载 opener 插件，
 * 首页首屏不背这个模块（与 `modelOfflinePackage` 原写法一致）。
 *
 * 返回值只表达「尝试过打开」，失败由调用方决定怎么提示 —— 各视图的 notify 不同。
 */
export async function openExternalUrl(url: string): Promise<void> {
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}
