/**
 * FEAT-SEM：扫描方式确认对话框文案（覆盖 / 增量）
 *
 * 三个扫描入口（相册扫描面板 / 全局扫描 / 批量管理）共用同一份说明文案，
 * 避免各写一份导致的措辞漂移。
 * ConfirmDialog 的 `.confirm-msg` 已是 `white-space: pre-line`，`\n` 会渲染为换行。
 */
export const SCAN_MODE_TITLE = "选择扫描方式";

export const SCAN_MODE_TIP =
  "覆盖：重新识别全部照片并更新结果（耗时较长，AI 字段以最新结果为准）\n" +
  "增量：跳过已入库的照片，只处理新增照片（推荐，速度快）";
