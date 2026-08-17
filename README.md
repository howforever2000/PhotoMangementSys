# Tauri + Vue + TypeScript

This template should help get you started developing with Vue 3 and TypeScript in Vite. The template uses Vue 3 `<script setup>` SFCs, check out the [script setup docs](https://v3.vuejs.org/api/sfc-script-setup.html#sfc-script-setup) to learn more.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Vue - Official](https://marketplace.visualstudio.com/items?itemName=Vue.volar) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## 多用户登录（用户功能）

本应用支持同一台电脑上多个用户分别注册、登录，每个用户的相册空间互相隔离。

- **注册**：账户名、邮箱、手机号、密码、密码确认（邮箱/手机号/账户名均唯一）
- **登录**：账户名 / 邮箱 / 手机号 任一 + 密码
- **忘记密码**：填账户名 + 邮箱 + 手机号，三者校验通过后即可重设密码
- **相册空间隔离**：相册、分组、标签、搜索、排序均按当前登录用户隔离
- **密码安全**：密码使用 Argon2id 加盐哈希存储，不落明文

### 升级迁移（内置管理员账户）

多用户功能上线前已存在的相册/分组没有归属用户。升级时若检测到无主数据，
会自动创建内置管理员账户并接管旧数据（仅迁移时创建一次）：

- 账户名：`admin`
- 密码：`admin123`
- 邮箱：`admin@local.dev` / 手机号：`13800000000`（用于忘记密码校验）

登录后可在设置中重设密码（当前版本通过「忘记密码」流程重设）。

