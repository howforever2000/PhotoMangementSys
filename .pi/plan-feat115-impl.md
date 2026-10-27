# 界面质感改造 · 执行方案（FEAT-115）

> 效果图：`design/preview/proposal/feat115-*.png`（7 个页面 × 深浅 2 套 = 14 张）
> 效果图源码：`design/preview/proposal/feat115-*.html` + `feat115.css` + `feat115-data.js`
> 复现：`node design/preview/proposal/shoot-feat115.cjs`
> **Home 不改**（用户 2026-10-26 明确要求）。
> 本文件是落地清单，每条都写了「改哪个文件、哪个选择器、从什么值改成什么值、为什么」。

---

## 〇、先读这一段：本方案的度量基准

效果图阶段用 `.visual/probe-feat115.cjs` 量过（口径见脚本注释）：

| 效果图页面 | 容器数 | 文字色档 | 说明 |
|---|---|---|---|
| 相册管理 | 4 | 4 | 3 个输入框/选中态 + 0 板 |
| 全局概览 | **1** | 4 | 只有那一块通栏板 |
| 分类·人物 | 0 | 3 | 全靠封面与文字 |
| 扫描入库 | 2 | 3 | 1 块板 + 1 个输入框 |
| 时间线 | 0 | 3 | 只有封面内描边 |

对照改造前的真实页面（`.visual/probe-containers.cjs`）：

| 页面 | 容器数（前） | 文字色档（前） | 目标 |
|---|---|---|---|
| AlbumList | 8 | **7** | ≤ 4 / ≤ 4 |
| SmartAlbum | **23** | 6 | ≤ 2 / ≤ 4 |
| ScanHub | 8 | 6 | ≤ 2 / ≤ 4 |
| Memories | 10 | 4 | ≤ 2 / ≤ 4 |
| Timeline | 2 | 4 | 保持 |
| SmartSearch | 12 | 2 | ≤ 2 / ≤ 4 |
| AlbumDetail | 13 | 6 | ≤ 3 / ≤ 4 |

> 「容器」的定义：有填充（alpha > 0.04）或有外投影的块级元素；封面 inset 1px 内描边、
> `border-top: 1px` 发丝线、`box-shadow: none` 都不算。所以它数的是「这块是不是一个独立的板/框」。

---

## 一、P0 · 顶栏从「一块浮起的玻璃板」改成「一条带」

**这是收益最大、风险也最集中的一项**：它一屏出现在所有 8 个页面，改完所有页面同时变干净。
也正因为如此，**建议放在最后单独一次提交**，不要和其他页面混在一起。

### 1.1 现状（`src/components/ui/AppHeader.vue`）

```
.mh          position: sticky; padding: 10px var(--sp-8) 0;     ← 板外留 10px 空气
.mh-inner    高 52px，圆角 var(--panel-glass-radius)
             background-color: var(--float-fill)
             backdrop-filter: url(#liquid-lens) blur(18px) saturate(...)
             border: 1px solid var(--float-border)
             box-shadow: inset 0 1px 0 rgba(255,255,255,.30)      ← 上缘镜面
                         inset 0 -1px 0 rgba(0,0,0,.34)           ← 下缘暗壁
                         0 10px 26px -16px rgba(0,0,0,.72)        ← 外投影
.mh-inner::before   135° 镜面高光渐变
```

板与内容之间有 10px 缝（`.mh` 的 padding-top），那条缝读起来像"两个窗口"。

### 1.2 改成什么

```
.mh          position: sticky; padding: 0; height: 54px;         ← 板外不留空气
             border-bottom: 1px solid var(--rule);               ← 只有下缘一根发丝线
             background: linear-gradient(180deg, var(--hover), transparent);
                                                                  ← 极淡一层，滚动时与内容分开
.mh-inner    高 52px；圆角 0
             background: transparent;  border: none;  box-shadow: none;
             backdrop-filter: none;                                ← 整条不再模糊
.mh-inner::before { display: none }                               ← 删镜面高光
```

### 1.3 具体编辑点（`src/components/ui/AppHeader.vue`）

| 位置 | 现状 | 改为 |
|---|---|---|
| `.mh`（约 176 行） | `padding: 10px var(--sp-8) 0; background: transparent;` | `padding: 0 var(--sp-8); height: 54px; border-bottom: 1px solid var(--rule); background: linear-gradient(180deg, var(--hover), transparent);` |
| `.mh-inner`（约 187 行起） | 高 52px + `border-radius: var(--panel-glass-radius)` + 填充/模糊/边/四层影 | 保留 `display:flex; align-items:center; gap:14px; height:52px; padding:0 16px 0 12px`，**删掉** `border-radius` / `background-color` / `backdrop-filter` / `border` / `box-shadow` / `position:relative` / `overflow:hidden` |
| `.mh-inner::before`（约 205 行） | 135° 高光 | 整段删除（连同其 CSS） |
| 浅色档 | — | 补 `body:not(.theme-dark) .mh { background: linear-gradient(180deg, rgba(255,255,255,.5), transparent); }` |

### 1.4 连带要查的两处

1. `.album-side { top: 150px }` 与 `.tl-rail { top: 68px }` 是**按"顶栏 56 + 数据带 96"算的吸顶位置**。
   顶栏从"52px 板 + 10px 空气 = 62px"变成 54px 后，这两个值都要重算：
   - `.tl-rail`：`top: 68px` → `top: 62px`（顶栏 54 + 8 呼吸）
   - `.album-side`：`top: 150px` → 按新的数据带高度重算（见 §2）
2. `--float-*` 令牌没有消费方了会不会变成死令牌：**不会**，搜索胶囊、弹窗、右键菜单还在用。
   但要在文件头注释里补一句"顶栏已改为带式，不再消费 --float-*"，否则下一个人会以为漏改了。

### 1.5 验收

```bash
node .visual/shoot-page.cjs AlbumList dark    # 需先起 dev server（端口见 §7）
```
- 顶栏与内容之间没有 10px 缝
- 内容从栏下穿过时，栏底有一根发丝线
- `.visual/audit-contrast.cjs` 仍 0 处不合格（顶栏文字底色从玻璃变成渐变，要重测）

---

## 二、P1 · 相册管理（AlbumList）

效果图：`feat115-albums-*.png`

### 2.1 数据带：金色预算（先纠正一个误判）

**先量再改**。我起初从代码读出"5 列里 4 条金色进度线"，实测（`.visual/_probe-gold.cjs`，
取 `getComputedStyle` 的真实值）是：

```
相册   | line=rgba(240,238,243,.6)  | bar.on=rgb(212,162,74) | aux=rgb(240,189,106)
照片   | line=rgba(240,238,243,.6)  | bar.on=-               | aux=-
已入库 | line=rgba(240,238,243,.6)  | bar.on=-               | aux=rgb(240,189,106)
待入库 | line=rgba(240,238,243,.6)  | bar.on=-               | aux=rgb(240,189,106)
占用   | line=rgba(240,238,243,.6)  | bar.on=-               | aux=-
```

**结论：占比线全部已经是弱色（`--fg-2`），没有一条金色。**
`progressTone` 五列里四条显式写了 `"muted"`，第五条（已入库）没写但默认就是非 accent。
所以 **AlbumList 数据带不需要改金色** —— 这里本来就是对的。

**真正超预算的是别处**，按实测排：

| 位置 | 金色出现数 | 判据 |
|---|---|---|
| AlbumList 数据带 | 2（年度柱阵的当月柱 + 3 个 `tone:"up"` 的 aux） | `bar.on=rgb(212,162,74)`、`aux=rgb(240,189,106)` |
| 左栏选中竖条 | 1（每项都有，但同屏只有 1 项选中） | `.side-item.on::before` |
| 新建相册主按钮 | 1 | `.btn-primary` |

同屏金色 = 2（柱阵当月 + 1 个 aux）+ 1（选中项）+ 1（主按钮）= **4 处**，仍超"≤2"。

**改法**（只动 aux，柱阵与主按钮保留）：

```ts
// src/views/AlbumList.vue · statItems —— 三处 tone: "up" 全部改掉
{ label: "已入库", value: scannedPhotos, delta: `${scannedAlbums} 个相册`,
  tone: "mute",                        // ← "up" → "mute"：这不是"变化值"，是静态计数
  progress: totalPhotos ? scannedPhotos / totalPhotos : 0 },
{ label: "待入库", value: pending, delta: pending ? "需要扫描" : "全部已扫",
  tone: pending ? "down" : "mute",     // ← "up" → "down"：待入库是"还差多少"，语义是欠缺不是增长
  progress: totalPhotos ? 1 - scannedPhotos / totalPhotos : 0, progressTone: "muted" },
```

> `tone: "up"` 会把 aux 染成 `--c-accent-bright`（截图里那三条暖金色）。
> 已入库/待入库都是**静态计数**，不是时间序列的变化值，本不该用 up。
> 这一处是 `tokens.css §2.7` 被违反的实证，而占比线不是。

### 2.2 相册墙：消灭"一张占满整行"

这是用户明确提的问题（"不要让一个相册占满整个屏幕"）。

> **修正（执行阶段实测）**：下面这段初稿的归因是错的。
> `grep breakPoints` 的结果是 **只有 `Timeline.vue` 传了 `breakPoints`，AlbumList 没传**。
> 所以 AlbumList 走的是"枚举行数 + 打分"那条主分支，它本来就有 `k===1 → score += 6` 的重罚，
> 不会出单块成行。效果图里那条"落日山峦独占整行"是**我手写效果图时的分行错误**，不是产品缺陷。
>
> 但真正的缺陷在**更下游、且两个分支共用**的那一步 —— 见 §2.2-b。下面 §2.2-a 对 Timeline 仍然成立。

**§2.2-a「逐段贪心」分支的隐患（只影响 Timeline）**

`Timeline.vue:267` 传了 `breakPoints: monthStarts.value`（不跨月），
走的是 `if (breaks.size)` 那条**逐段贪心**分支：

```ts
while (i < to) {
  const row = [];
  let sumA = 0;
  while (i < to) {
    const k = row.length + 1;
    const hAfter = (W - gap * (k - 1)) / (sumA + aspects[i]);
    if (row.length > 0 && hAfter < H0) break;   // ← 只看高度，不看块数
    row.push(i); sumA += aspects[i]; i += 1;
  }
  if (row.length) finalGroups.push(row);
}
```

一个月只剩 1 张照片时，`row.length > 0` 永远不成立，那张就单独成行。
**但"单独成行"本身不是问题** —— 问题是下一步怎么给它算宽度。

**§2.2-b 真正的根因：高度被夹住，宽度却仍拉满（两个分支共用）**

```ts
// justifiedRows.ts §2 生成行（改前）
const coverH = Math.round(clamp(avail / sumA, minCoverH, maxCoverH));  // ← 夹住了
const cells = row.map((idx) => ({
  coverH,
  width: Math.round((aspects[idx] / sumA) * avail),   // ← 宽度仍然精确等于 W
}));
```

高度被夹到 `maxCoverH`，宽度却按"铺满 W"分配 —— **两者独立，宽高比就失真了**。
单块成行时最极端：宽 = 整个容器 W、高被压到 260 → 一条**占满全屏的横幅**。
这跟断点无关，`n=1`（库里只有一个相册）时同样会出。

**改法**（`src/utils/justifiedRows.ts` §2，已落地）：

```ts
while (i < to) {
  const row = [];
  let sumA = 0;
  while (i < to) {
    const k = row.length + 1;
    const hAfter = (W - gap * (k - 1)) / (sumA + aspects[i]);
    if (row.length > 0 && hAfter < H0) break;
    /* 兜底：段内只剩 1 块时不允许它独占整行。
       块数 > 1 却只剩 1 块可放，说明这一行已经是本段最后一行 ——
       此时应把它并到上一行（接受高度被夹），而不是给一个 100% 宽的横幅。
       效果图里用户明确否掉了"一个相册占满整个屏幕"。 */
    if (row.length === 0 && to - i === 1 && finalGroups.length > 0) break;
    row.push(i); sumA += aspects[i]; i += 1;
  }
  if (row.length) finalGroups.push(row);
  /* 上一行的兜底：把落单的最后一块并进去，行高会被夹到上限，但不是横幅 */
  if (!row.length && finalGroups.length) {
    const prev = finalGroups[finalGroups.length - 1];
    if (prev.length) { prev.push(i); i += 1; continue; }
    row.push(i); i += 1;   // 整段就只有一块：仍然单独成行（无解）
  }
}
```

> 单测要补：`justifiedRows.test.ts` 加一条「段内只剩 1 块时不成横幅」——
> 传入 `breakPoints` 使某段只剩 1 项，断言该行宽度 < 容器宽的 60%。

**同时**（`src/views/AlbumList.vue` 的 `wallRows` 调用）把宽幅因子从 1.8 降到 1.5：

```ts
justifyRows({
  ...
  wideEvery: 5,
  wideFactor: 1.5,   // ← 1.8 → 1.5。1.8 倍宽在 3 列屏上会占掉大半个视口
  nominalHeight: 232,
  baseAspect: 1.28,
})
```

### 2.3 图注：从"压封面的两行落款"改成"封面下方一行"

**现状**（`src/components/AlbumCard.vue`）：`.tile-seal` 绝对定位在封面内左下角，
两行（名字 13.5px + 元信息 10.5px），配 `.tile-cover:has(img)::after` 的 54px 深渐变。
一屏 12 张，每张被盖掉左下角约 15%。

**改法**：

| 项 | 现状 | 改为 |
|---|---|---|
| 图注位置 | `position:absolute; left:10px; right:10px; bottom:9px`（封面内） | 封面外下方：`.album-tile.tile-wall` 下 `margin-top: 8px` 的普通流内元素 |
| 名字 | 13.5px / `--fw-strong`(650) / 白字 + 双投影 | 13px / 600 / `var(--fg)`（封面外来文字，不用白字） |
| 元信息 | 10.5px / `--fg-2` / 白字 | 10.5px / `var(--fg-3)` |
| 布局 | 两行竖排 | **一行横排**：名字左、张数右，同基线 |
| 渐变兜底 | `::after` 高 54px、最深 0.42 | 降到高 34px、最深 0.30（只为封面内的日期/未入库角标兜底） |
| 行高 | `captionH: 0`（图注不占行高） | `captionH: 18`（图注回到流里，参与行高计算） |

**为什么张数不要 `margin-left:auto` 推到最右**：瓦片宽度不同（宽幅 1.5 倍），
推到右边会让每一行的张数落在不同 x 位置，读起来像两栏表格而不是一句描述。
用 `gap: 7px` 连成一串即可。

**连带**：`AlbumCard.vue` 的 `wall` 分支模板要把 `.tile-seal` 移出 `.tile-cover`：

```html
<div class="tile-cover"> ... 角标/勾选/日期 ... </div>
<div class="tile-seal">            <!-- ← 移到封面外 -->
  <b class="tile-name">{{ album.name }}</b>
  <span class="tile-meta">{{ tileMeta }}</span>
</div>
```

### 2.4 左栏：已经是文字列，只需与数据带对齐

`.side-item` 的选中态（2px 金条 + 文字提亮）**已经是对的**，不用改。
要改的是 `.side-h` 与 `.u-stat-label` 的字阶一致性 —— 两处已经是同一套
（9.5px / .15em / uppercase / `--fg-2`），确认即可。

`.album-side { top: 150px }` 需按新数据带高度重算（数据带现在 ~112px 高）：

```
top = 顶栏 54 + 数据带 112 + 间距 16 = 182px  →  实际取 178px（留 4px 呼吸）
```

### 2.5 文字色档从 7 档收到 4 档

实测 7 档的来源是三种灰混用。全局替换（`src/views/AlbumList.vue`）：

| 现状 | 改为 | 出现处 |
|---|---|---|
| `var(--c-fg-2)` | `var(--fg-2)` | `.main-sub` `.search-icon` `.search-scope` `.search-elsewhere` 等 |
| `var(--color-text-2)` | `var(--fg-2)` | 同上一批 |
| `opacity: .7` / `opacity: .65` | `var(--fg-3)` | `.tl-year-count` 之类（AlbumList 内搜 `opacity`） |
| `var(--c-fg)` | `var(--fg)` | 同上 |

> `--c-fg*` 是给"压在承载面上"的文字标定的（深一档），`--fg*` 是给"直接压在壁纸上"的。
> 数据带/左栏/图注都在壁纸上，统一用 `--fg*`。**组件内部**（弹窗、卡片内）才用 `--c-fg*`。

---

## 三、P2 · 智慧相册 · 全局概览（23 → 1 个容器）

效果图：`feat115-smart-overview-*.png`
源码：`src/components/OverviewPane.vue`（505 行）+ `src/components/ui/widget/*`

### 3.1 这是本方案最大的一处改动，先说清结构

**现状**：11 块 `<UiWidget>`，每块有自己的 `background-color` + `border` + `box-shadow`，
块间 `gap: 14px`，缝里露出壁纸。

**改法**：把 `WidgetGrid` 从"11 块独立 widget"改成"**一块板内的 12 栅格**"。

```
改前                           改后
┌────┐ ┌────┐ ┌────┐          ┌────────────────────────────────┐
│w1  │ │w2  │ │w3  │          │ 照片库总览      │ 今年月度节奏  │  ← 区1（span 7 / 5）
└────┘ └────┘ └────┘          ├──────────────────┼─────────────┤
┌────┐ ┌────┐ ┌────┐          │ 语义索引 │ 那年今日 │ 相册封面   │  ← 区2（4/4/4）
│w4  │ │w5  │ │w6  │          ├──────────┼──────────┼───────────┤
└────┘ └────┘ └────┘          │ 人物(7)          │ 地点(5)    │  ← 区3（7/5）
┌────┐ └────┘ └────┘          ├──────────────────┼─────────────┤
│w7  │                        │ 分类 │ 覆盖环 │ Top城市       │  ← 区4（4/4/4）
└────┘                        ├──────┴─────────┴───────────────┤
                              │ 最近导入(8)      │ 那年今日(4)  │  ← 区5（8/4）
                              └────────────────────────────────┘
```

### 3.2 具体编辑点

#### `src/components/ui/widget/UiWidget.vue`

把材质从"每块独立玻璃"改成"板内无底"：

| 项 | 现状 | 改为 |
|---|---|---|
| `data-mat="glass"` 的 `background-color` | `rgba(var(--tint), .38)` | `transparent` |
| `backdrop-filter` | `blur(24px) saturate(1.32)` | `none` |
| `border` | `1px solid rgba(255,255,255,.11)` | `none` |
| `box-shadow` | 棱边 + 接触影 | `none` |
| `border-radius` | `18px` | `0`（板内不需要圆角；圆角由外层板给） |
| `data-mat="plain"` | `var(--color-surface-2)` 近实心 | 同改 `transparent` |

**新增一个 prop** `bare?: boolean`（或直接改 `material` 语义），
让 OverviewPane 传"板内模式"，其他潜在消费方不受影响。

#### `src/components/ui/widget/WidgetGrid.vue`

```css
/* 现状 */
.grid { display: grid; grid-template-columns: repeat(12, 1fr);
        grid-auto-rows: 86px; gap: 14px; }

/* 改为：板内容器 —— 只负责分区，不产生板 */
.ui-widget-grid { display: flex; flex-direction: column; }   /* 5 个区 */
.zone { padding: 18px 22px; }
.zone + .zone { border-top: 1px solid var(--rule); }        /* 区间发丝线 */
.zone-inner { display: grid; grid-template-columns: repeat(12, 1fr); gap: 14px; }
```

#### `src/components/OverviewPane.vue`

结构改成 5 个 `<section class="zone">`，每个里面一个 `.zone-inner` 12 栅格。
**widget 的 span 分配**（效果图用的是这套）：

| 区 | widget | span | rows |
|---|---|---|---|
| 1 | 照片库总览 | 7 | 2 |
| 1 | 今年月度节奏 | 5 | 2 |
| 2 | 语义索引 | 4 | 1 |
| 2 | 那年今日 | 4 | 1 |
| 2 | 相册封面 | 4 | 1 |
| 3 | 人物 | 7 | 2 |
| 3 | 地点 | 5 | 2 |
| 4 | 内容分类 | 4 | 1 |
| 4 | 人脸照片覆盖 | 4 | 1 |
| 4 | Top 城市 | 4 | 1 |
| 5 | 最近导入 | 8 | 2 |
| 5 | 那年今日（大卡） | 4 | 2 |

**列间隔线**：`.zone-inner > * + *` 之间那根竖线不能每格都画（那又是表格）。
效果图的做法是只给"跨区分界的那一格"加 `border-left` + `padding-left: 22px`：
即 span 5 / span 4 的第二、三格。实现上给这些 widget 加一个 class（如 `zone-cut`）。

#### `src/components/ui/widget/StatCore.vue` 等 8 个 Core

内容不用改（数据与展示都对），**只确认它们没有自己的背景/边框**。
如果某个 Core 里有 `background` / `box-shadow`，一并去掉。

### 3.3 金色预算

效果图里这块板上只有 **1 处金色**（入库进度条）。检查：
- `GaugeCore`（语义索引环）现在是金色 → 改成 `var(--fg-3)`，因为索引未建立时不该用强调色
- `RankBarsCore` 的 Top1 → 保留金色（"第一名"是允许的第三处）
- `BarsCore` 的"当月"柱 → 保留金色

如果超了，优先降 `GaugeCore`。

---

## 四、P3 · 内容分类 + 人物（去掉玻璃底）

效果图：`feat115-smart-cards-*.png`

### 4.1 分类卡（`src/components/CategoryGallery.vue`）

```html
<!-- 现状 -->
<article class="cat-card glass-card" :style="theme.cardStyle" ...>

<!-- 改为 -->
<article class="cat-card" ...>          <!-- 去掉 glass-card 与 cardStyle -->
```

对应 CSS（约 764 行）：

| 现状 | 改为 |
|---|---|
| `border-radius: 14px` | 保留 |
| `box-shadow: 0 4px 14px rgba(0,0,0,.1)` | `box-shadow: inset 0 0 0 1px var(--cover-line)`（只留封面内描边） |
| `transition: transform..., box-shadow...` | 只留 `filter`（hover 提亮度，不位移） |
| `:hover { transform: translateY(-3px); box-shadow: 0 10px 24px ... }` | `:hover { filter: brightness(1.06) }` |

渐变兜底从 76px/0.52 降到与相册墙一致的 34px/0.30（只为角标兜底，落款自己带 text-shadow）。

### 4.2 人物卡（`src/components/PersonGallery.vue`）

```html
<!-- 现状 -->
<article class="person-card glass-card" :style="surfaceStyle" ...>
<!-- 改为 -->
<article class="person-card" ...>
```

| 项 | 现状 | 改为 |
|---|---|---|
| `padding` | `20px 18px 16px` | `0`（没有卡就不需要内边距，头像自己说话） |
| 布局 | 竖排居中 | 保持 |
| `border-radius` | 14px | 删（没有底了） |
| `:hover { transform: translateY(-2px) }` | 删 |
| 重命名按钮 | 常驻 | 悬停才现（`opacity: 0` → `:hover` 时 1） |
| 名字 | `line-clamp: 2` 已对 | 保持（这是 FEAT-113 修好的，别动） |
| 张数徽标 | 压头像下沿 | 保持 |

`.person-grid` 的 `minmax(196px, 1fr)` → `minmax(150px, 1fr)`（没有卡了，可以更密）。

---

## 五、P4 · 扫描入库（材质归位 + 一块板）

效果图：`feat115-scan-*.png`
源码：`src/components/GlobalScanPanel.vue`（875 行）

### 5.1 问题：面板已经是墙板玻璃，但**内部又套了第二层板**

```css
.gs-area { border: 1px solid var(--panel-glass-edge); border-radius: 12px; }
body .gs-area.glass-surface { background-color: var(--panel-glass-fill); ... }

.gs-select-block {                                  /* ← 第二层 */
  margin: 0 14px;
  border: 1px solid var(--c-rule);
  border-radius: 8px;
  background: color-mix(in srgb, var(--c-fg) 4%, transparent);
}
```

### 5.2 改法：外层板保留，内层板删掉，改左右分栏

```
┌──────────────────── .gs-area（一块墙板玻璃）────────────────────┐
│ 左：选择区（无内衬板）        │ 右：任务区                        │
│  · 全选/反选/已选计数/过滤    │  · 空态说明 或 运行进度            │
│  · 相册行列表                 │                                  │
│  · 扫描内容 chips             │                                  │
│  · 底部 CTA                   │                                  │
└─────────────────────────────────────────────────────────────────┘
```

| 位置 | 现状 | 改为 |
|---|---|---|
| `.gs-select-block` | 有边框 + 底色 + 8px 圆角 | `border: none; background: none; border-radius: 0; margin: 0;` |
| `.gs-album-row:hover` / `.checked` | `background: var(--color-soft-accent)`（整块底色） | `background: var(--hover)`（5% 微光）；勾选态靠左侧 2px 竖条表达 |
| `.gs-controls`（chips） | `flex-wrap` 独占一块 | 保留，但去掉外层容器底（如果有） |
| `.gs-progress-block` | `padding: 10px 14px` | 保留，右侧栏内 |

新增左右分栏外壳（`.gs-area` 内）：

```css
.gs-cols { display: grid; grid-template-columns: minmax(0, 1.15fr) minmax(0, 1fr); }
.gs-cols > .gs-l { padding: 20px 22px; }
.gs-cols > .gs-r { padding: 20px 22px; border-left: 1px solid var(--rule); }
@media (max-width: 1100px) { .gs-cols { grid-template-columns: 1fr; } .gs-cols > .gs-r { border-left: none; border-top: 1px solid var(--rule); } }
```

**注意**：`.gs-area` 现在**已经是**墙板玻璃了（`body .gs-area.glass-surface` 那段注释写得很清楚）。
所以 §5.2 不需要动 `.gs-area` 本身，只删内层。这与其说是"改材质"，不如说是"去掉双重容器"。

### 5.3 ScanHub 数据带（`src/views/ScanHub.vue`）

`statItems` 里同样查一遍金色 `progressTone`，只留"已入库相册"一条。

---

## 六、P5 · 时间线（基准页，只做两处微调）

效果图：`feat115-timeline-*.png`（**只渲染 2026 一年**，避免 24000px 高的图）

时间线已经是全站最干净的一页（容器 2），**不要大改**。只改两处：

### 6.1 页头三行合一

**现状**（`src/views/Timeline.vue`）：

```html
<header class="tl-header"><h1 class="tl-title">照片时间线</h1><p class="tl-subtitle">跨相册按拍摄时间聚合浏览</p></header>
<div class="tl-toolbar">
  <label class="tl-count">共 {{ totalFiltered }} 张</label>
  <select v-model="yearFilter" class="tl-year-select">...</select>
</div>
```

标题 → 工具条 → 数据带 = 三行约 200px 才看到第一张照片。

**改为一行**（`page-head` 形态，效果图里已有）：

```html
<div class="tl-pagehead">
  <h1 class="tl-title">照片时间线</h1>
  <span class="tl-sub">跨相册按拍摄时间聚合浏览 · 共 {{ totalFiltered }} 张</span>
  <span class="tl-right">
    <button class="btn" @click="...">全部年份 ▾</button>   <!-- 自绘，见 6.2 -->
  </span>
</div>
```

`.tl-title` 字号 `var(--fs-head)`(19px) → `17px`（小标题档，顶栏已经有"智慧相册"）。

### 6.2 原生 `<select>` 换自绘下拉

**根因**（已在 `BUG-2026-1024-*` 修过一次类名碰撞，但**白底 option 无解**）：
`<select>` 的 `option` 列表由操作系统绘制，CSS 控制不了。
`.tl-year-select option { background: #1e1b22; color: #f2f0f4 }` 在部分平台仍被忽略。

**做法**：换成与 `.btn` 同规格的按钮 + 自绘浮层（复用 `ContextMenu` 或新写一个 `PmSelect`）。
这是**独立小组件**，可复用（SmartSearch 的 6 个 `<select>` 同样受益）。

> 如果本轮不想做自绘下拉，**最低限度**是把 `.tl-year-select` 的样式做到与 `.btn` 一致
> （高 28px、`box-shadow: inset 0 0 0 1px var(--ctrl-line)`、无边框），
> 至少视觉上不再是一块白底黑框的外来物。

### 6.3 吸顶位置

`.tl-rail { top: 68px }` → `top: 62px`（顶栏 54px + 8px 呼吸，见 §1.4）。

### 6.4 数据带 5 列 → 4 列

`无拍摄时间` 与 `地点覆盖` 这两个诊断读数在任何一年都读同一个值，各占一列是浪费。
合并成一列：

```ts
{ label: "诊断", value: `${noTime}`, unit: "张无拍摄时间",
  note: `地点覆盖 ${locPct}% · ${noLoc} 张无定位` }
```

---

## 七、P6 · 回忆 / 智能搜索 / 相册详情（对齐收敛）

效果图：`feat115-memories-search-detail-*.png`（三屏合一）

### 7.1 Memories（`src/views/Memories.vue`）

| 项 | 现状 | 改为 |
|---|---|---|
| `.mem-story` | `flex: 0 0 220px; height: 280px; border-radius: 16px; box-shadow: var(--shadow-1)` | 保留尺寸与圆角，**删 `box-shadow`**，改 `box-shadow: inset 0 0 0 1px var(--cover-line)` |
| `.mem-story::before/::after` | `candy-surface` 颗粒层（`z-index: 1`） | 删除（糖果釉面是上一代方向） |
| `.mem-story:hover` | `translateY(-3px) + shadow-2` | `filter: brightness(1.06)` |
| `.mem-section-head h2` | `20px / 700` | `22px / 620`（与相册墙的 `.group-year` 同一档） |
| `.mem-section-sub` | `var(--color-text-2)` | `var(--fg-3)` |
| `.mem-person` | 有自己的卡底（若有） | 去掉，头像 + 名字 + 次数三行文字 |
| `.mem-photo` / `.mem-cap` | 网格卡 | 保留网格，`.mem-cap` 从压封面改到封面下方一行 |

### 7.2 SmartSearch（`src/views/SmartSearch.vue`）

| 项 | 现状 | 改为 |
|---|---|---|
| `.ss-title` | `var(--fs-head)`(19px) | 17px（与 Timeline 一致） |
| `.ss-subtitle` | `opacity: .7` | `var(--fg-3)` |
| `.ss-filters` | `background: var(--card-bg); padding: 12px; border-radius: 12px` | `background: none; padding: 0; border-radius: 0` —— 6 组筛选不该是一块底板 |
| `.ss-input` | `font-size: 15px` | `13px`（15px 在这套字阶里没有对应档） |
| `.ss-empty` | `padding: 70px 20px` + 大图标 + 两行字 | `padding: 52px 0 30px`，图标缩小或去掉，补一行可点示例 chips |
| 6 个 `<select>` | 原生 | 同 §6.2（换自绘） |
| `.ss-warn` | — | 保留（缺模型提示是功能性的） |

### 7.3 AlbumDetail（`src/views/AlbumDetail.vue` + `AlbumMeta.vue`）

| 项 | 现状 | 改为 |
|---|---|---|
| `.detail-page` | `max-width: 1200px; margin: 0 auto; padding: 24px` | `padding: 0 var(--gutter) 48px`，删 `max-width`/`margin`/`padding: 24px` |
| `.detail-nav` | 独占一行（面包屑 + 删除按钮） | 面包屑并入 AppHeader（顶栏 meta 已是相册名）；删除按钮移到信息行右侧 |
| `AlbumMeta` | 一张大卡（封面 88px + 名字 + 路径 + 说明 + 5 个 meta 胶囊 + 标签） | **信息一行**：封面 + 标题/路径/说明 + `meta-row`（张数/大小/时间/地点/入库）+ 标签 chips，全部无底无框 |
| `.collapse-section` | `background: var(--color-surface); border: 1px solid var(--color-border); border-radius: var(--radius-card); box-shadow: var(--shadow-1)` | **折纸行**：`background: none; border: none; box-shadow: none; border-radius: 0; border-top: 1px solid var(--rule)` |
| `.cs-head:hover` | `background: var(--color-primary-soft)` | `background: var(--hover)` |
| `.cs-title` | `15px / 600` | `12.5px / 560` |
| `.cs-action-hint` | `点击展开` | 保留（文案是对的，说明当前状态） |

---

## 八、执行顺序（按可独立回滚排列）

| 阶段 | 内容 | 触碰文件 | 风险 | 可回滚 |
|---|---|---|---|---|
| **S1** | 令牌与文案色统一（§2.5） | `AlbumList.vue` `Timeline.vue` `Memories.vue` `SmartSearch.vue` `AlbumDetail.vue` | 低（纯替换） | 是 |
| **S2** | 分类卡/人物卡去玻璃底（§4） | `CategoryGallery.vue` `PersonGallery.vue` | 低 | 是 |
| **S3** | 相册图注移到封面下方 + 金色收敛（§2.1 §2.3） | `AlbumCard.vue` `AlbumList.vue` | 中（改变首屏观感） | 是 |
| **S4** | 消灭独占行 + 宽幅 1.8→1.5（§2.2） | `justifiedRows.ts` + 单测 | 中（动算法） | 是 |
| **S5** | 扫描面板去内层板（§5） | `GlobalScanPanel.vue` | 中 | 是 |
| **S6** | Timeline / SmartSearch / AlbumDetail 对齐收敛（§6 §7） | 三个 view + `CollapseSection.vue` | 中 | 是 |
| **S7** | **概览一面墙一块板**（§3） | `OverviewPane.vue` + `ui/widget/*` | **高**（改动最大） | 是 |
| **S8** | **顶栏改带式**（§1） | `AppHeader.vue` | **高**（影响全部 8 页） | 是 |

**建议先做 S1 + S2 + S3**：只碰 4 个文件，就能在相册管理与分类/人物两页看到整体方向是否正确。
确认后再推 S4–S6，最后才是 S7 与 S8。

---

## 九、验证（每阶段都跑，不要攒到最后）

```bash
# 1. 类型 / 测试 / 构建
npx vue-tsc --noEmit
npm test                                   # 现 137/137；S4 后应变 138+
npx vite build

# 2. 容器与文字色档（本方案的度量基准）
node .visual/probe-containers.cjs          # 目标见 §〇 表格
node .visual/probe-containers.cjs SmartAlbum

# 3. 对比度（深浅 × 全部页面）
node .visual/audit-contrast.cjs

# 4. 截图对照
npx vite --port 1519 --strictPort &        # ⚠️ 不要占 1420
VITE_PORT=1519 node .visual/shoot-page.cjs AlbumList dark light
VITE_PORT=1519 node .visual/shoot-current.cjs    # 新增：拍全部 10 页当前状态
```

### 各阶段的量化验收

| 阶段 | 指标 | 目标 |
|---|---|---|
| S1 | AlbumList 文字色档 | 7 → 4 |
| S2 | 分类/人物页容器数 | — → 0 |
| S3 | 相册墙图注位置 | 封面内 → 封面下方；行高含 18px 图注 |
| S4 | 独占行宽度 | 100% → < 60% 容器宽 |
| S5 | ScanHub 容器数 | 8 → 2 |
| S6 | 三页留白 | 统一 `--sp-8`（52px） |
| S7 | SmartAlbum 容器数 | 23 → 1~2 |
| S8 | 全站容器数 | 每页再 −1 |

---

## 十、明确不做的事

- **Home 不改**（用户 2026-10-26 明确要求）
- 不改任何 URL 路由
- 不动 `src-tauri` 后端
- 不引 UI 框架 / 图表库
- 不动 `justifiedRows.ts` 的**核心算法**（只加 §2.2 那条兜底）
- 不改 `StatBand.vue` 的 props（6 个消费页零改动）
- 不删任何统计字段（信息不删，只去壳）
- 不恢复渐变 hero / 彩虹入口卡

---

## 十一、遗留事项（本轮不做，但记在这里）

1. **自绘下拉 `PmSelect`**：§6.2 与 §7.2 都依赖它。原生 `<select>` 的 option 白底
   在 CSS 里无解，只有自绘能根治。建议单独立项（FEAT-116）。
2. **搜索范围隔离**：`AlbumList` 的搜索已经只走 `search_albums`（FEAT-107 做过），
   但顶栏搜索胶囊的范围标注（`全库` / `仅名称`）还没实现 —— 效果图里有，代码里没有。
3. **`--float-*` 令牌**：S8 之后顶栏不再消费它，但搜索胶囊/弹窗/菜单仍在用，不会变死令牌。
   要在 `AppHeader.vue` 文件头注明，否则后人会以为漏改。
