// 用户认证相关类型定义
//
// 严格对应 Rust 侧 `src-tauri/src/auth.rs` 中的结构体字段，
// 保证前端 invoke 调用的参数与返回值类型安全。

/** 用户实体 —— 对应 Rust `auth::User`（不含密码哈希） */
export interface User {
  id: number;
  /** 账户名（唯一） */
  username: string;
  /** 邮箱（唯一，小写） */
  email: string;
  /** 手机号（唯一） */
  phone: string;
  /** 注册时间戳（Unix 秒） */
  created_at: number;
}

/** 注册输入 —— 对应 Rust `auth::RegisterInput`（需求：账户名、邮箱、手机号、密码、密码确认） */
export interface RegisterInput {
  username: string;
  email: string;
  phone: string;
  password: string;
  /** 密码确认（必须与密码一致） */
  confirm_password: string;
}

/** 登录输入 —— 对应 Rust `auth::LoginInput`（account 为账户名/邮箱/手机号任一） */
export interface LoginInput {
  account: string;
  password: string;
}

/** 忘记密码重置输入 —— 对应 Rust `auth::ResetPasswordInput` */
export interface ResetPasswordInput {
  username: string;
  email: string;
  phone: string;
  new_password: string;
  confirm_password: string;
}

/** 修改基本信息输入 —— 对应 Rust `auth::UpdateProfileInput`（需先验证当前密码） */
export interface UpdateProfileInput {
  email: string;
  phone: string;
  current_password: string;
}
