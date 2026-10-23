# 执行计划 · Canvas 极简重设计

> 分支 `dev`（起点 `56000ed`）· 2026-10
> 设计规格见 `design/design-system.md`

---

## 阶段总览

| 阶段 | 内容 | 触碰文件 | 可回滚 |
|---|---|---|---|
| **P0** | 令牌层 + 交互手感基线 | `src/assets/tokens.css`(新) `main.css` | 是（不动组件） |
| **P1** | 核心组件：数据带 / 相册瓦片 / 行内统计 | `src/components/ui/*`(新) | 是（新组件） |
| **P2** | 页面落地：相册 + 时间线 | `AlbumList.vue` `Timeline.vue` | 是（逐页） |
| **P3** | 地点地图 | `LocationGallery.vue` + 前端底图生成 | 是（新增视图） |
| **P4** | 预设精简（10→6）+ 深浅模式 | `presets.ts` `prefs.ts` `ThemeDialog.vue` | 是 |
| **P5** | 其余页面套同一骨架 + 截图基线 | 其余 views | 是 |

---

## P0 · 令牌层

### P0-1 新建 `src/assets/tokens.css`
- [ ] 字阶 7 档 / 字重 3 档 / 间距 8 档 / 圆角 3 档 / 动效 3×2 / z-index 8 层 / 色彩预算
- [ ] 深色 `:root` + 浅色 `body.theme-light` 两套值
- [ ] `main.ts` 中在 `main.css` **之前**引入

### P0-2 `main.css` 收敛
- [ ] `--shadow-1`（液态）6 层 → 3 层；`::after` 棱环 1.5px → 1px
- [ ] 删除 `.candy-surface` 在卡片上的使用（保留类，仅登录页用）
- [ ] 全站 `transition` 时长映射到 `--dur-1/2/3`（139 处）
- [ ] `prefers-reduced-motion` 与 `prefers-contrast: more` 双降级

### P0-3 交互手感
- [ ] 所有按钮/卡片加 `:active { transform: scale(.985) }`（现仅 6 处）
- [ ] 统一 `:focus-visible` 焦点环
- [ ] 热区补足到 40×40（图标按钮）

**验收**：`npm run build` 通过；全站 `transition` 无硬编码 ms；截图与改造前对比仅颜色/时长变化，无布局位移。

---

## P1 · 核心组件

### P1-1 `StatBand.vue`
- [ ] props: `items: { label, value, unit?, delta?, deltaTone?, spark: number[] }[]`
- [ ] 可选 `inline` slot（右侧内联图表）
- [ ] 发丝竖线用 `::before` 实现，不含边框

### P1-2 `AlbumTile.vue`
- [ ] props: `cover`, `name`, `meta`, `badge?`
- [ ] 裸封面 + 图注两行；悬停亮度 + 内描边；按压 scale
- [ ] 封面图 `loading="lazy"` + 失败占位

### P1-3 `GroupStat.vue`
- [ ] props: `items: { label, value }[]`、`dist?: number[]`
- [ ] 与组标题同基线渲染

### P1-4 `Sparkline.vue`
- [ ] props: `data: number[]`、`w`、`h`、`highlight`
- [ ] 纯 CSS 柱（不引依赖）

**验收**：4 个组件单测（props 渲染 / 空数据 / 极值）；在 Storybook 式的临时页面对比三种数据形态。

---

## P2 · 页面落地

### P2-1 `AlbumList.vue`（2883 行，最大改造）
- [ ] 提取模板中的网格 → `AlbumTile`
- [ ] 顶部统计卡 → `StatBand`
- [ ] 移除 `.glass-card` / `theme.cardStyle` 在瓦片上的使用
- [ ] 导航：年份分组保留，但组头改用 `GroupStat`
- [ ] **不动**：多选、右键菜单、拖拽、批量导入等既有逻辑（只换样式层）

### P2-2 `Timeline.vue`
- [ ] 顶部统计带换成时间线专属四项（含"无拍摄时间占比""GPS 覆盖"）
- [ ] 年组头加 `5 个月 · 388 张 · 平均 78/月` + 月度迷你柱
- [ ] 月行右侧加 `N 张`

**验收**：Playwright 截图（1440×900 / 1280×800 / 1024×768 × 深浅共 6 张/页），与改造前逐像素 diff，位移只允许出现在预期区块。

---

## P3 · 地点地图

### P3-1 前端底图（一次性脚本）
- [ ] `python/gen_map_lite.py`：读 `china_geo.json` → DP 容差 0.02° 简化省级轮廓 → 输出 `public/china-provinces-lite.json`（目标 ≤ 300KB）
- [ ] 保留省市名 + bbox，用于点位落图与标签

### P3-2 后端补字段
- [ ] `LocationGroupRow` 增加 `city_center: Option<(f64, f64)>`（该聚合点所有照片 lat/lon 的均值）
- [ ] `list_photo_locations` 聚合时累加 lat/lon（用现有 `lat`/`lon` 列，**无需迁移**）
- [ ] 保持向后兼容：新字段为 `null` 时前端降级为纯卡片视图

### P3-3 `LocationMap.vue`（新组件）
- [ ] SVG 等距圆柱投影，省级轮廓 `stroke: rgba(255,255,255,.04)`
- [ ] 金色光点：`r = 3 + 2.2·ln(count)`，`box-shadow` 光晕，`prefers-reduced-motion` 下不闪
- [ ] 悬停/点击 → 复用现有 `openGroup(g)` 打开照片网格
- [ ] 空态：无点位时地图像素级留空，仅底部显示 `GPS 覆盖 0% · 31 张无定位`
- [ ] `LocationGallery.vue` 顶部加视图切换：`地图 / 卡片`

**验收**：653 张测试数据（当前 0 张有 GPS）验证空态；构造 20 个点位验证点位半径/标签避让；地图懒加载不拖慢首屏（`IntersectionObserver`）。

---

## P4 · 预设精简

- [ ] `presets.ts`：10 套 → 6 套，新增 `mode` 字段
- [ ] `prefs.ts`：`normalizePrefs` 对已删除预设色值按自定义处理（不回落默认、不报错）
- [ ] `ThemeDialog.vue`：分组改「深色 / 浅色」，加模式切换按钮
- [ ] 测试：`presets.test.ts` / `prefs.test.ts` 同步；新增「已删除 preset 不崩溃」用例

**验收**：`npm test` 全绿；老用户 localStorage（含已删预设）加载后外观不变。

---

## P5 · 其余页面 + 基线

- [ ] 人物 / 语义 / 搜索 / 扫描 / 相册详情 / 回忆 套同一顶栏 + 数据带
- [ ] 建立 `scripts/visual-baseline.cjs`（Playwright 截图基线，6 组合/页）
- [ ] 删除 11 个页面里的自制「返回」按钮（导航常驻后不需要）

---

## 编码纪律（本计划强制）

- **CS1 分层**：`statband` / `album-tile` / `group-stat` / `sparkline` / `location-map` 各自独立组件；页面只做数据装配，不写样式细节
- **CS2 日志**：新增前端 `utils/trace.ts`（统一 `traceStart/traceEnd`，含参数、耗时、错误），用于 P3 的 `list_photo_locations` 调用与地图底图加载
- 每阶段独立 commit，message 前缀 `feat(ui):` / `refactor(ui):` / `fix(ui):`

---

## 待确认

- [ ] Q1. P0 是否要**保留旧令牌别名**（如 `--radius-card` 映射到 `--r-ui`）以降低一次性改动风险？建议保留一版。
- [ ] Q2. P3 地图是否限定**中国境内**（现有 `china_geo.json` 只有中国数据）？境外照片如何处理 —— 归入「境外/无定位」还是引入世界底图？
- [ ] Q3. P2 的 `AlbumList.vue` 改造是否允许"先新增组件、页面切换分两次提交"？2883 行一次改完风险高。
