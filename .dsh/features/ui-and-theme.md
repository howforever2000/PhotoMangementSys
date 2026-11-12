# UI 与主题（ui-and-theme）

> 设计令牌层 `tokens.css` + 液态玻璃材质 + 主题预设 / 深浅模式 / 对比度 / 组件透明度；全局视觉系统的持续重构。功能清单见 `features/index.md`。

## 核心能力（功能 ID）
- 登录页玻璃态（FEAT-059）、组件色调三选（FEAT-084）
- 界面质感改造 v1：深玻璃分层 + 收敛强调色 + 文字对比硬指标（FEAT-085）
- 主题弹窗重构：双页签（预定义 5 套三段渐变 / 自定义）+ 组件透明度 + 液态·磨砂材质（FEAT-086）
- 应用图标全套 + 登录/注册/忘记密码三页视觉统一（FEAT-093）、默认外观改「启动封面」壁纸 + 落日金（FEAT-094）
- impeccable 前端精修 v2（FEAT-100）、液态玻璃重做静态光学 + 实体壁厚（FEAT-101）
- Canvas 令牌层 + 数据带 + 瓦片 + 地点地图（FEAT-103）、模块页头 + 二级 tab + 纸片材质 + 预设 8 套（FEAT-105）、仪表墙 Rainmeter（FEAT-106）、无容器阅读层 + 搜索边界隔离（FEAT-107）、液态玻璃令牌冲突 + 真实边缘折射（FEAT-108）
- 智慧相册玻璃仪表墙 + 松绿混色 + 双材质 widget 库（FEAT-110）、深色线条看不见根因 + 边界可见性实测（FEAT-111）
- 预设收敛（FEAT-112 / 119）、卡片轻量化 + 瀑布流错落（FEAT-113）、顶栏带式化（FEAT-115）、图注回退（FEAT-117）、取色器改色盘 + 自定义预设命名（FEAT-118）、概览地点 Top5 排行（FEAT-120）、液态玻璃 v2 浅底可见性 + 滑块跟手（FEAT-121）
- 全局概览快照「数据变动才重算」（FEAT-123）、概览竖幅锚点布局 + 那年今日跳转（FEAT-161）

## 入口文件
- 样式：`src/assets/main.css`、`src/assets/tokens.css`、`src/assets/widget.css`、`src/assets/pm-dialog.css`、`src/assets/auth.css`、`src/assets/ai-tool.css`
- 状态/工具：`src/stores/theme.ts`、`src/stores/overview.ts`、`src/utils/presets.ts`、`src/utils/prefs.ts`、`src/utils/color.ts`、`src/utils/customPresets.ts`、`src/utils/loginTheme.ts`
- 组件：`src/components/ThemeDialog.vue`、`src/components/AuthShell.vue`、`src/components/OverviewPane.vue`、`src/components/ui/AppHeader.vue`、`src/components/ui/StatBand.vue`、`src/components/ui/PmColorPicker.vue`、`src/components/ui/widget/*`、`src/components/ui/LocationMap.vue`
- 设计源：`design/design-system.md`、`design/preview/`

## 关键约束与历史坑
- **文字色要感知「实际合成背景」**：`textColor` 跟模式（卡片语境）+ `onBgColor/onBgSubColor` 只对页面背景算对比度（WCAG 不足 4.5:1 自动翻转）。详见 `knowledge/ui-contrast-tokens.md`（BUG-2026-0919-002 / 004）。
- **新增文字令牌按预设渐变最亮/最暗端点标定**，不是纯黑纯白；透明度必须叠底色算合成色。详见 `knowledge/ui-contrast-tokens.md`（BUG-2026-1023-003 / 1024-007）。
- **对比度审计先自检假阳性**（底用元素自身背景、解析 `color(srgb 0~1 / a)`、豁免禁用态）。详见 `knowledge/ui-contrast-tokens.md`（BUG-2026-1024-004）。
- **深色 `.btn` 全局覆盖要排除实心/幽灵变体**；禁用态用实心灰不用整体 opacity。详见 `knowledge/ui-contrast-tokens.md`（BUG-2026-0925-002）。
- **`body.theme-dark` 特异性 / 伪元素须 content / `backdrop-filter` 层叠+fixed 包含块**：详见 `knowledge/frontend-style-pitfalls.md`（BUG-2026-1024-003 / 1027-001 / 0918-005）。
- **地图缩放四坑**：光点跟着放大 / 剪裁留白 / 拖拽快 k 倍 / 键盘收不到 —— 交互类改动必须写探针断言，不能只靠截图。来源：`bug-notes.json` BUG-2026-1024-008。
- **相册瓦片封面塌 0px**：`aspect-ratio` + flex 收缩 + `<img>` 循环依赖。来源：`bug-notes.json` BUG-2026-1023-006。
