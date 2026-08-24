import { defineStore } from "pinia";
import { ref } from "vue";

export type ToastType = "success" | "warning" | "error" | "info";

export interface ToastAction {
  label: string;
  onClick: () => void;
  style?: "primary" | "secondary" | "danger";
}

export interface Toast {
  id: number;
  type: ToastType;
  title: string;
  message?: string;
  duration?: number; // ms，0 = 持久化，需手动关闭
  actions?: ToastAction[];
  createdAt: number;
}

let toastIdCounter = 0;

/** 全局 Toast 通知状态管理：支持多实例堆叠、自动消失、手动关闭、动作按钮 */
export const useToastStore = defineStore("toast", () => {
  const toasts = ref<Toast[]>([]);
  const timers = new Map<number, ReturnType<typeof setTimeout>>();

  function add(
    type: ToastType,
    title: string,
    options?: { message?: string; duration?: number; actions?: ToastAction[] },
  ): number {
    const id = ++toastIdCounter;
    const toast: Toast = {
      id,
      type,
      title,
      message: options?.message,
      duration: options?.duration ?? defaultDuration(type),
      actions: options?.actions,
      createdAt: Date.now(),
    };
    toasts.value.push(toast);
    return id;
  }

  function remove(id: number) {
    const t = timers.get(id);
    if (t) {
      clearTimeout(t);
      timers.delete(id);
    }
    const idx = toasts.value.findIndex((t2) => t2.id === id);
    if (idx !== -1) toasts.value.splice(idx, 1);
  }

  /** 启动自动消失计时（duration > 0 才启用） */
  function scheduleDismiss(id: number, duration: number) {
    if (duration <= 0) return;
    const t = setTimeout(() => remove(id), duration);
    timers.set(id, t);
  }

  function clear() {
    for (const t of timers.values()) clearTimeout(t);
    timers.clear();
    toasts.value = [];
  }

  function success(title: string, message?: string, duration = 3000) {
    const id = add("success", title, { message, duration });
    scheduleDismiss(id, duration);
    return id;
  }
  function warning(title: string, message?: string, duration = 4000) {
    const id = add("warning", title, { message, duration });
    scheduleDismiss(id, duration);
    return id;
  }
  function error(title: string, message?: string, duration = 0) {
    const id = add("error", title, { message, duration });
    scheduleDismiss(id, duration);
    return id;
  }
  function info(title: string, message?: string, duration = 3000) {
    const id = add("info", title, { message, duration });
    scheduleDismiss(id, duration);
    return id;
  }
  function persistentError(title: string, message?: string) {
    const id = add("error", title, {
      message,
      duration: 0,
      actions: [{ label: "知道了", onClick: () => {}, style: "primary" }],
    });
    return id;
  }
  function action(
    type: ToastType,
    title: string,
    actions: ToastAction[],
    message?: string,
    duration = 0,
  ) {
    const id = add(type, title, { message, duration, actions });
    scheduleDismiss(id, duration);
    return id;
  }

  return { toasts, add, remove, clear, success, warning, error, info, persistentError, action };
});

/** 按类型返回默认显示时长（ms），0 表示不自动消失 */
function defaultDuration(type: ToastType): number {
  switch (type) {
    case "success":
      return 3000;
    case "warning":
      return 4000;
    case "error":
      return 0; // 错误默认持久化，避免用户错过关键信息
    case "info":
      return 3000;
  }
}
