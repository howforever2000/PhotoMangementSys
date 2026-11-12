# 前端 / CSS 避坑

> Vue3 `<script setup>` + scoped CSS + 液态玻璃主题下高频踩的坑。来源均为 `bug-notes.json`。

## 新建公共组件前先查是否已存在（勿覆盖 AOP trace.ts）

- **触发条件**：需求说「要一个公共日志组件」。
- **先做**：先 grep 文件是否已存在；项目原本就有 `src/utils/trace.ts`（AOP 装饰器 `trace(name, fn)`），扩展而非覆盖。
- **为什么**：误以为缺失，按「新建文件」写入另一个 API（`traceStart/traceEnd`），把已有 AOP 日志组件覆盖掉。
- **违规症状**：build 立刻 4 处 `import { trace }` 找不到导出。
- **来源**：`bug-notes.json` BUG-2026-1023-005（`src/utils/trace.ts`）。
- **记录日期**：2026-11-12

## scoped CSS 追加必须落在 `</style>` 之内

- **触发条件**：用 `cat >>` 往 `.vue` 末尾追加样式。
- **先做**：定位最后一个 `</style>` 之前插入（用 edit 而非盲追）。
- **为什么**：文件末尾已有 `</style>`，追加内容落在它之后成为 SFC 之外的普通文本，Vue 编译器直接丢弃且**不报错**。
- **违规症状**：构建通过、元素存在 class 正确但样式零命中；`getComputedStyle` padding=0；三步排查：diag 读 computedStyle → 遍历 styleSheets → curl 编译产物 grep。
- **来源**：`bug-notes.json` BUG-2026-1023-007（`src/components/AlbumCard.vue`）。
- **记录日期**：2026-11-12

## 主题覆盖 `body.xxx` 特异性更高，组件覆盖要提级或排除

- **触发条件**：组件内 `.gs-btn-primary` / `.gs-area.glass-surface` 覆盖，但 `main.css` 的 `body.theme-dark .btn` / `body.theme-dark .glass-surface` 生效。
- **先做**：`body` 主题规则用 `:not(.btn-primary/.btn-danger/.btn-ghost/...)` 排除变体；或组件层提一级特异性；改完必须读 `getComputedStyle` 确认真值，别凭截图猜「是不是没热更」。
- **为什么**：`body.theme-dark .btn` 特异性 (0,2,1) 高于组件 scoped (0,2,0)，把实心主色按钮一并刷成半透明白。
- **违规症状**：样式改了但颜色没变；主色按钮变透明；白底白钮。
- **来源**：`bug-notes.json` BUG-2026-1024-003、BUG-2026-0925-002（`src/assets/main.css`）。
- **记录日期**：2026-11-12

## 伪元素「写了 background 却没 content」静默失效

- **触发条件**：液态玻璃棱环 `::after` 只写 `background` 不写 `content`。
- **先做**：伪元素必须给 `content:''/position/inset`；判据看 `getComputedStyle(el,'::after').content`，不是源码里有没有这段；同一材质在深 / 浅两套底色上各验一次。
- **为什么**：没有 `content` 的伪元素不会被生成，`background` 声明完全空转。
- **违规症状**：样式面板里看得到声明，页面上什么都没有。
- **来源**：`bug-notes.json` BUG-2026-1027-001（`src/assets/main.css`）。
- **记录日期**：2026-11-12

## `<script setup>` 顶层调用要放最后一行，防 TDZ

- **触发条件**：在 `<script setup>` 中段调用 `void initForCurrentPhoto()` 等同步副作用函数。
- **先做**：顶层副作用调用放 `<script setup>` **最后一行**，并在上方注释「必须放最后」及 TDZ 成因；局部变量别与外层 computed 同名（避免遮蔽）。
- **为什么**：同步走到 `ref/const` 声明之前即触发 TDZ；异常被 `void` / 同步链吞掉，后续 EXIF 补全与单张扫描整体不执行也不重试。
- **违规症状**：信息面板恒显示「未记录拍摄时间/地点」、翻一张再翻回来才正常；TDZ 错误指向外层声明行看起来像「定义顺序问题」（实为内层遮蔽）。
- **来源**：`bug-notes.json` BUG-2026-1030-001、BUG-2026-1023-008（`src/views/Timeline.vue`）。
- **记录日期**：2026-11-12

## `backdrop-filter` 同时产生层叠上下文与 `position:fixed` 包含块

- **触发条件**：玻璃态 UI 里做浮动下拉 / 遮罩。
- **先做**：header 提 `position:relative; z-index:20`；遮罩移出 `backdrop-filter` 祖先（或 `<teleport to="body">`）；z-index 分层要「卡片之上、header 之下」。
- **为什么**：`backdrop-filter`（同 `filter`/`transform`）既产生新层叠上下文，又把 `position:fixed` 的包含块从视口改成该元素自身。
- **违规症状**：菜单被卡片盖住只剩上半截、遮罩只盖头像一小块、点页面其余处关不掉菜单。
- **来源**：`bug-notes.json` BUG-2026-0918-005（`src/views/Home.vue`）。
- **记录日期**：2026-11-12

## 事件传播类「点了没反应」先查遮罩 / `@click.stop`

- **触发条件**：右键菜单 / 弹窗打开后点不动、点别处不消失。
- **先做**：检查全屏遮罩是否 `@click.stop` 掐断 document 关闭分支（`@contextmenu.prevent.stop` 同理）；`onItemClick` 抛错前先 `emit('close')`。
- **为什么**：这类 bug 表现为「点了没反应」，真因在事件传播而非点击逻辑本身。
- **违规症状**：右键别处既不关闭也不放行原生菜单。
- **来源**：`bug-notes.json` BUG-2026-1007-001（`src/components/ContextMenu.vue`）。
- **记录日期**：2026-11-12

## 「安全阀」用途变更后必须复查

- **触发条件**：组件透明度滑块在液态下 0~62% 被统一夹到 0.58。
- **先做**：用途一变（材质变成用户可选卡面）就复查三重夹紧（`Math.max(0.58, base-0.04)` + 对比安全阀）；验收滑块看**下发的值**而不只是画面。
- **为什么**：安全阀是为特定用途写的，用途变了会静默吃掉用户操作。
- **违规症状**：滑块拖了没反应，用户只说「滑块坏了」。
- **来源**：`bug-notes.json` BUG-2026-1027-002。
- **记录日期**：2026-11-12
