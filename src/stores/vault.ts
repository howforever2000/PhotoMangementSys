import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAuthStore } from "./auth";

/**
 * FEAT-095：隐藏相册解锁状态（会话内存态）
 *
 * 需求：隐藏相册在相册管理中默认不可见，需在「个人信息」界面输入**登录密码**
 * 解锁后才出现（决策 D3：解锁后本次会话一直可见，关闭弹窗不失效）。
 *
 * 为什么只放内存不落 localStorage：
 * 密码门禁的意义是「每次冷启动都要重新证明身份」。若把解锁态持久化，
 * 任何能打开应用的人都能绕过密码看到隐藏相册，门禁形同虚设。
 *
 * 登出 / 切换用户时由 `reset()` 显式清空（`App.vue` 监听登出），
 * 保证隐藏相册不会跨账号泄漏。
 */
export const useVaultStore = defineStore("vault", () => {
  /** 隐藏相册是否已解锁（本次会话内一直为 true 直到登出） */
  const hiddenUnlocked = ref(false);
  /** 解锁请求进行中（防重复提交） */
  const unlocking = ref(false);

  /** 验证登录密码并解锁隐藏相册；密码错误时返回 false 且不改变解锁态 */
  async function unlock(password: string): Promise<boolean> {
    if (unlocking.value) return hiddenUnlocked.value;
    unlocking.value = true;
    try {
      // 后端只比对密码，不回传任何密码信息（argon2 校验，与登录同一套逻辑）
      const ok = await invoke<boolean>("verify_current_password", { password });
      if (ok) hiddenUnlocked.value = true;
      return ok;
    } finally {
      unlocking.value = false;
    }
  }

  /** 主动上锁（用户想重新隐藏时可用） */
  function lock() {
    hiddenUnlocked.value = false;
  }

  /** 登出 / 切换用户时清空解锁态 */
  function reset() {
    hiddenUnlocked.value = false;
  }

  return { hiddenUnlocked, unlocking, unlock, lock, reset };
});

/**
 * 登出时联动清空隐藏相册解锁态。
 *
 * 单独提供这个函数而不是让调用方手动 `useVaultStore().reset()`，
 * 是为了让「谁在登出」这件事只有一个入口 —— 未来再加账号切换也不会漏。
 */
export async function logoutAndResetVault(): Promise<void> {
  const auth = useAuthStore();
  const vault = useVaultStore();
  try {
    await auth.logout();
  } finally {
    // 无论登出成功与否都清锁：宁可多锁一次，也不让解锁态跨会话残留
    vault.reset();
  }
}
