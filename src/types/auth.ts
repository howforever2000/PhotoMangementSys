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
  /** 注册时间戳（Unix 秒） */
  created_at: number;
  /** 头像文件绝对路径（FEAT-045；null = 未设置，前端回退占位图标） */
  avatar: string | null;
}

/** 注册输入 —— 对应 Rust `auth::RegisterInput`（FEAT-153：账户名、邮箱、密码、密码确认，手机号已去除） */
export interface RegisterInput {
  username: string;
  email: string;
  password: string;
  /** 密码确认（必须与密码一致） */
  confirm_password: string;
}

/** 登录输入 —— 对应 Rust `auth::LoginInput`（account 为账户名/邮箱任一） */
export interface LoginInput {
  account: string;
  password: string;
}

/** 忘记密码重置输入 —— 对应 Rust `auth::ResetPasswordInput`（账户名+邮箱双匹配） */
export interface ResetPasswordInput {
  username: string;
  email: string;
  new_password: string;
  confirm_password: string;
}

/** 修改基本信息输入 —— 对应 Rust `auth::UpdateProfileInput`（需先验证当前密码） */
export interface UpdateProfileInput {
  email: string;
  current_password: string;
}
