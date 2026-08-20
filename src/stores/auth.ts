import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import type {
  LoginInput,
  RegisterInput,
  ResetPasswordInput,
  User,
} from "../types/auth";

/**
 * 用户认证全局状态（多用户登录）
 *
 * 对应后端 `SessionState`：当前登录用户保存在 Rust 侧会话中，
 * 前端通过 `get_current_user` 恢复会话（应用启动 / 路由守卫）。
 */
export const useAuthStore = defineStore("auth", {
  state: () => ({
    /** 当前登录用户，null 表示未登录 */
    user: null as User | null,
    /** 是否已完成一次会话检查（启动 / 守卫 / 登录后） */
    checked: false,
  }),

  actions: {
    /** 恢复会话：应用启动时调用，未登录返回 null */
    async checkSession(): Promise<User | null> {
      try {
        this.user = await invoke<User | null>("get_current_user");
      } catch {
        this.user = null;
      } finally {
        this.checked = true;
      }
      return this.user;
    },

    /** 登录（账户名/邮箱/手机号 + 密码），成功后写入当前用户 */
    async login(input: LoginInput): Promise<User> {
      this.user = await invoke<User>("login", { input });
      this.checked = true;
      return this.user;
    },

    /** 注册（注册成功自动登录） */
    async register(input: RegisterInput): Promise<User> {
      this.user = await invoke<User>("register", { input });
      this.checked = true;
      return this.user;
    },

    /** 退出登录 */
    async logout(): Promise<void> {
      await invoke("logout");
      this.user = null;
      this.checked = true;
    },

    /** 忘记密码重置（账户名 + 邮箱 + 手机号校验通过后重设密码） */
    async resetPassword(input: ResetPasswordInput): Promise<void> {
      await invoke("reset_password", { input });
    },
  },
});
