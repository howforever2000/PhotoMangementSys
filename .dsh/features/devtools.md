# 开发者工具（devtools）

> 开发者视角三入口：实时日志副窗口（cmd 风，tail `app.log`）、数据与路径副窗口（运行态路径清单 + 只读库表 + SQL 只读沙箱）。功能清单见 `features/index.md`。

## 核心能力（功能 ID）
- 设置入口 + cmd 风实时日志副窗口（只读 `app.log` 文件 tail，无事件流改造，FEAT-054）
- 数据与路径副窗口：运行态路径清单（体量 / 口径 / 定位）+ 只读库表浏览（FEAT-058）
- SQL 查询只读沙箱：表结构 DDL + 用户信息默认打码（FEAT-060）
- 已废弃：主页「数据库查看」独立入口并入「数据与路径」（FEAT-064）

## 入口文件
- 前端：`src/dev/DataWindowApp.vue`、`src/components/settings/*`（开发者视角入口）、`src/views/Home.vue`
- Rust：`src-tauri/src/devtools.rs`、`devdata.rs`、`devpath.rs`、`lib.rs`
- 模板/权限：`src-tauri/capabilities/dev-data.json`（副窗口 capability）

## 关键约束与历史坑
- **建窗 / 日志读取绝不能在主线程同步执行**：`open_dev_log_window` 曾用同步命令在主线程建窗 → 窗口停在 `about:blank`（空白无标题）+ 卡死事件循环（全应用卡顿）。改为异步命令 + 限流读取。来源：`bug-notes.json` BUG-2026-0910-005、BUG-2026-0910-001（洪峰读取放大）。
- **路径清单不能用构建机绝对路径**：`CARGO_MANIFEST_DIR` 被烘进 release 二进制 → 「数据与路径」暴露本机用户名。详见 `knowledge/data-path-and-packaging.md`（BUG-2026-1013-001）。
- **SQL 沙箱只读**：表结构展示 + 用户信息默认打码；不得开放写操作。
- 服务端成本结构日志（`[vcr.stat] classify_batch …`）曾未进 `app.log`（open）。来源：`bug-notes.json` BUG-2026-1027-010。
