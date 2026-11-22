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

/** 同屏最多同时存在的 toast 数：超限丢最旧（add 内强制执行） */
const MAX_STACK = 6;

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
    // 堆叠上限：超过 MAX_STACK 时丢掉最旧的（连计时器一起清），
    // 防止连续失败时一层层摞满整屏（BUG-2026-1121-004：8 条地点识别失败堆屏不消失）
    if (toasts.value.length > MAX_STACK) {
      const oldest = toasts.value[0];
      remove(oldest.id);
    }
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

  /**
   * 暂停某个 toast 的自动消失（鼠标 hover 时调用）。
   * 仅取消计时器，不改变原 duration。
   */
  function pauseDismiss(id: number) {
    const t = timers.get(id);
    if (t) {
      clearTimeout(t);
      timers.delete(id);
    }
  }

  /**
   * 恢复某个 toast 的自动消失（鼠标离开时调用）。
   * 使用统一较短剩余时间（保证暂停后不会停留太久）。
   */
  function resumeDismiss(id: number) {
    const toast = toasts.value.find((t2) => t2.id === id);
    if (!toast || !toast.duration || toast.duration <= 0) return;
    // 重置为与原长成比例的“剩余时间” 走一个简化的统一策略：使用 1.6s 剩余时间
    // 避免 hover 来回切后 toasts 持续很久。
    const remaining = 1600;
    const t = setTimeout(() => remove(id), remaining);
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
  function error(title: string, message?: string, duration = 6000) {
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
      actions: [{ label: "知道了", onClick: () => remove(id), style: "primary" }],
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

  return { toasts, add, remove, clear, success, warning, error, info, persistentError, action, pauseDismiss, resumeDismiss };
});

/** 按类型返回默认显示时长（ms），0 表示不自动消失 */
function defaultDuration(type: ToastType): number {
  switch (type) {
    case "success":
      return 3000;
    case "warning":
      return 4000;
    case "error":
      return 6000; // 错误默认 6s 自动消失；确需用户必须看到的用 persistentError（带「知道了」按钮）
    case "info":
      return 3000;
  }
}
