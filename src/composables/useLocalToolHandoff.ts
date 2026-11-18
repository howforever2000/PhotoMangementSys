/**
 * 工具之间的「同类直达」交接（FEAT-169）
 *
 * 背景：同一个目标常有两条代价不同的路 —— 例如「美颜」可以走云端整幅重绘
 * （效果强、形状只有软约束），也可以走本机分区磨皮（形状数学恒等、免费离线）。
 * 用户在看到云端面板时，最自然的诉求是「那我要形状绝对不变的那条」，此时不该
 * 让他自己回左栏找。
 *
 * 做法：只把**目标工具 id + 当前素材**交给 `workshopUi` store（它本来就负责
 * 「用户在哪、带了哪些照片」，且离开页面不重置），由工坊视图完成实际跳转。
 * 这样面板组件不需要知道路由，也不产生第二份「当前工具」状态。
 */
import { primaryTool, toolById } from "../utils/toolCatalog";
import { isWorkshopTool, useWorkshopUiStore } from "../stores/workshopUi";

/** 把素材与目标工具一起交给工坊视图；返回是否已成功交接。 */
export function handOffToTool(toolId: string, paths: readonly string[]): boolean {
  if (!isWorkshopTool(toolId)) return false;
  const ui = useWorkshopUiStore();
  if (paths.length) ui.setTray([...new Set([...ui.tray, ...paths])]);
  ui.setActive(toolId);
  return true;
}

/** 「本地分区磨皮」这一路的工具 id；清单一改这里自动跟随，不手抄字符串。 */
export function localBeautyTarget(): string {
  return toolById("local_beauty") ? "local_beauty" : primaryTool().id;
}

/** 组合式包装：组件里 `const go = routeToLocalTool()` 再 `go(paths)`。 */
export function routeToLocalTool(toolId = localBeautyTarget()) {
  return (paths: readonly string[]): boolean => handOffToTool(toolId, paths);
}
