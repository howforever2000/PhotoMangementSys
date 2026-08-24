import { useToastStore } from "../stores/toast";

/**
 * 统一的 Toast 提示组合式函数
 * 替代原生 alert/confirm，提供更好的用户体验。用法：
 *
 *   import { useNotify } from "@/composables/useNotify"; // 注意：当前项目未配置 @ 别名，请用相对路径
 *   const { success, error, warning, info, confirm } = useNotify();
 *   success("操作成功", "相册已创建");
 *   error("创建失败", "该路径已被占用");
 *   const ok = await confirm("确定删除吗？", "此操作不可撤销");
 */
export function useNotify() {
  const toast = useToastStore();

  const success = (title: string, message?: string, duration = 3000) => toast.success(title, message, duration);
  const warning = (title: string, message?: string, duration = 4000) => toast.warning(title, message, duration);
  /** 错误提示（默认持久化，需手动关闭/点"知道了"） */
  const error = (title: string, message?: string, duration = 0) => toast.error(title, message, duration);
  const info = (title: string, message?: string, duration = 3000) => toast.info(title, message, duration);
  const persistentError = (title: string, message?: string) => toast.persistentError(title, message);

  /** 确认对话框 - 返回 Promise<boolean>，替代原生 confirm() */
  const confirm = (
    title: string,
    message?: string,
    options?: { confirmText?: string; cancelText?: string; type?: "danger" | "primary" },
  ): Promise<boolean> =>
    new Promise((resolve) => {
      toast.action(
        options?.type === "danger" ? "error" : "info",
        title,
        [
          {
            label: options?.cancelText || "取消",
            style: "secondary",
            onClick: () => resolve(false),
          },
          {
            label: options?.confirmText || "确定",
            style: options?.type === "danger" ? "danger" : "primary",
            onClick: () => resolve(true),
          },
        ],
        message,
        0,
      );
    });

  const action = (
    type: "success" | "warning" | "error" | "info",
    title: string,
    actions: { label: string; onClick: () => void; style?: "primary" | "secondary" | "danger" }[],
    message?: string,
    duration = 0,
  ) => toast.action(type, title, actions, message, duration);

  const clear = () => toast.clear();

  return { success, warning, error, info, persistentError, confirm, action, clear, _store: toast };
}

/**
 * 便捷导出：在非组件上下文（store、工具函数）中直接 `notify.success(...)`
 */
export const notify = {
  success: (title: string, message?: string, duration = 3000) => useToastStore().success(title, message, duration),
  warning: (title: string, message?: string, duration = 4000) => useToastStore().warning(title, message, duration),
  error: (title: string, message?: string, duration = 0) => useToastStore().error(title, message, duration),
  info: (title: string, message?: string, duration = 3000) => useToastStore().info(title, message, duration),
  persistentError: (title: string, message?: string) => useToastStore().persistentError(title, message),
  confirm: (
    title: string,
    message?: string,
    options?: { confirmText?: string; cancelText?: string; type?: "danger" | "primary" },
  ): Promise<boolean> =>
    new Promise((resolve) => {
      useToastStore().action(
        options?.type === "danger" ? "error" : "info",
        title,
        [
          { label: options?.cancelText || "取消", style: "secondary", onClick: () => resolve(false) },
          {
            label: options?.confirmText || "确定",
            style: options?.type === "danger" ? "danger" : "primary",
            onClick: () => resolve(true),
          },
        ],
        message,
        0,
      );
    }),
  clear: () => useToastStore().clear(),
};
