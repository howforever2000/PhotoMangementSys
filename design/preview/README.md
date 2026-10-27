# 界面质感改造 · 效果图与落地记录（FEAT-106）

> 定位：在 FEAT-105（Rainmeter 皮肤化）之上做**重做**，而不是继续打补丁。
> 触发原因：FEAT-105 版本在真实数据下出现三类问题（见第一节），
> 且视觉仍带有"通用卡片套件"的味道，没有真正落到参考图的**仪表墙**形态。
>
> `all-01 ~ all-16.png` 是 FEAT-103 时期的对照图，仍可作为版式参考；
> 本轮的真实状态以 `.visual/shoot-page.cjs` 现拍的 `shot-*.png` 为准。

---

## 一、本轮修的三个硬问题（都是实测复现的）

### 1. 数据带与左栏**重叠**

**现象**：相册管理页左上角，左栏的玻璃板上沿压住了数据带面板的下沿。

**成因**：左栏玻璃板是一个 `::before` 伪元素，`inset: -10px -12px -12px -14px`，
向上溢出了 10px；而数据带就在它正上方 4px 处 —— 两个盒子在视觉上咬在一起。

**修法**（不是调 z-index，而是改结构）：

| 改动 | 说明 |
|---|---|
| `inset: -10px …` → `inset: 0 -10px -10px -12px` | 板不再向上溢出，只在左/下扩一点余量 |
| 数据带 `margin-bottom: 4px → 16px` | 板的上沿与数据带之间留出真实间距 |
| 左栏 `top: 72px → 168px` | 吸顶位置避开"顶栏 56 + 数据带 96 + 间距" |

### 2. 密排墙的**空洞**

**现象**：`grid-auto-flow: dense` + 大瓦片跨 2×4 时，行尾出现一整块无法回填的空白。

**成因**：大瓦片横跨两列后，它"横跨"的那两列在上方已被占满，剩下的空位是奇数格；
dense 回填需要**连续**空位，于是填不上。另外 dense 的回填顺序依赖元素流，
筛选/滚动后同一个相册会"跳位置"。

**修法**：整块换成**行式密排**（justified rows），算法见 `src/utils/justifiedRows.ts`。

```
每一行是一个 flex 容器：
  行宽 = Σ瓦片宽 + gap×(n−1)  ≡  容器宽   ← 精确铺满，没有空洞
  行内所有瓦片等高（封面高 + 34px 图注）    ← 图注条严格对齐
  宽幅瓦片（每 5 块一张）宽 1.8 倍         ← 错落，但周期可预期
```

行数/行高由**枚举 + 打分**决定（不是单条公式）：

```
对 R = 1 … n/2：
   把 n 块瓦片按宽高比分给 R 行（每行预算 = 总宽高比 / R）
   算每行封面高 h = (W − g·(k−1)) / Σa_row
   打分：h 偏离名义高度 → 扣分；h 被夹到上下限 → 重罚；单块成行 → 重罚
取分数最低者
```

为什么必须枚举：单条公式 `R = round((H₀·A + g·n)/(W + g))` 在离散切行时会舍入到
"末行只剩一块"，而末行一块的宽度就是整个容器 —— 被高度上限夹住后就是一条扁片
（实测出现过 1120×260 的巨幅横幅）。枚举 + 打分把这种形状直接排除掉。

14 条单测覆盖：每行精确铺满、同行等高、无巨幅、封面高在区间内、宽幅周期、
容器宽度自适应、count=0、极小容器。`node --experimental-strip-types --test`

### 3. 数据带的**数字压在环上**

**现象**：读数旁的环形仪表与 delta 文本（如"129 个相册"）叠在一起。

**成因**：环是**绝对定位**在面板右下角、宽度固定 20px；delta 换行到第二行、
或面板被压窄时，读数就与环重叠。

**修法**：环 → **底部进度条**（在 flex 流里独占一行）。

- 布局上：进度条自己占一行，**结构上不可能**与文字重叠（不是靠调层级掩盖）；
- 语义上：进度条读"占比/完成度"比读角度更直接，也更容易在窄面板上表达。

---

## 二、视觉：从"卡片套件"到"仪表墙"

参考图（Rainmeter 桌面皮肤）的可迁移特征是三条，本轮逐条落地：

| 参考图特征 | 落地方式 |
|---|---|
| 一面墙 = 若干**独立磨砂玻璃板**，板间露出壁纸 | `--panel-glass-*` 令牌 + `.panel-glass` / `StatBand` / 侧栏 / 入口卡全部消费同一套 |
| 每块板里有**大读数 + 微标签 + 微型图表** | 数据带：微标签（9.5px 大写）+ 30px 读数 + 进度条；相册格：大数字/张数 + 年度柱阵 |
| 板的大小**错落** | 相册墙：宽幅瓦片 ×1.8；主页入口卡：12 栅格里 7/5 与 5/7 交替 |

### 统一材质：三种玻璃各司其职（不再混用）

| 令牌 | 用途 | 特征 |
|---|---|---|
| `--panel-glass-*` | 墙板（数据带 / 侧栏 / 入口卡 / 扫描面板） | 填充更透、模糊 26px、饱和 1.25 |
| `--glass-*` | 浮层（顶栏 / 搜索胶囊 / 弹窗 / 右键菜单） | 填充更实（下限 .58）、模糊 16px |
| `--slip-*` | 纸片（组级小面板） | 不模糊、只靠明度差读厚度 |

⚠️ 本轮踩到的坑：`body.theme-dark .glass-surface`（特异性 0,2,1）会盖掉组件里的
`.gs-area.glass-surface`（0,2,0）—— 表现为"代码改了但面板还是旧的紫调玻璃"。
组件要覆盖它必须提到 `body .gs-area.glass-surface`（0,3,1）+ scoped 属性选择器。

### 顶栏：从"页面底色 + 一条线"改成"一块浮起的板"

```
padding: 10px var(--sp-8) 0     ← 板外面留空气，内容从板下穿过
.mh-inner: 玻璃板（52px 高，圆角 12）
  [← 主页] [图标][组件名 · 读数] │ 分区1 分区2 …        [搜索胶囊] [头像]
```

分区导航的顺序全局统一为：**内容分类 / 人物 / 地点 / 回忆 / 时间线 / 智能搜索**，
四个页面（SmartAlbum / Memories / Timeline / SmartSearch）共用同一份顺序 ——
否则页间跳转时同一排 tab 会"跳位"。

高亮判定也从 `$route.path === t.to` 改成**分别比较 path 与 tab query**：
`/smart?tab=category` 的 path 永远只是 `/smart`，旧写法导致这两个 tab 永不高亮。

### 智慧相册：默认落"内容分类"

进入智慧相册时最想看到的是"按内容找照片"，而不是一堆还没命名的人脸聚类
（P001 / P094）。人物的可用性依赖逐个改名，不该占主入口。
切 tab 会同步写进 URL（`router.replace`，不污染历史），刷新/分享能回到同一块。

### 相册格：图注改成压在封面上的胶囊

旧版图注是封面下方一条**贯穿整格宽**的玻璃条 —— 一墙的面板就被切成一道道横带
（那是"表格"，不是"墙"）。现在：胶囊浮在封面左下角，宽度随内容变化，
时间另起一个更小的胶囊压在上面一行，两者垂直错开。

---

## 三、其余界面的对齐

| 页面 | 本轮改动 |
|---|---|
| Home | 从"大标题 + 2×2 等宽卡"改成**控制台**：仪表数据带（照片/相册/占用/已入库）+ 12 栅格宽窄交替的入口卡；去掉 1000px 居中，与其余页同一套留白 |
| Workshop | 删掉与顶栏重复的大标题（同一信息说两遍，还把首屏内容推下 36px） |
| Timeline / SmartSearch | 去掉 `max-width: 1200 + padding: 20` 的居中缩窄，改用 `0 var(--sp-8)`，与全宽顶栏对齐；页内标题降一档 |
| Timeline | 修一个**类名碰撞**：下拉框与年份卡片都叫 `.tl-year`，下拉吃到了卡片的 `background: var(--card-bg)`，浅色预设下变成浅底浅字（实测 1.33:1） |
| ScanHub / GlobalScanPanel | 主面板从"组件色调玻璃"改为墙板玻璃；按钮从硬编码 `#fff / #1f2733` 改为令牌；徽标改"实底 + 白字" |
| CategoryGallery | 警告行从 `opacity: .7` 改为 `--c-fg-2`（opacity 会把已弱化的文字再乘一次） |

---

## 四、验证（量化，不是目测）

### 1. 对比度审计

`.visual/audit-contrast.cjs` 本轮被重写了两处方法学缺陷 —— 否则结果不可信：

1. **按元素自身的背景链算**，而不是只用页面底颜色：
   旧版会把"金底墨字"的主按钮算成 1.2:1（假阳性 6/10 条）。
2. **能解析 `color(srgb …)`**：Chromium 对 `color-mix()` 的计算值输出是 0~1 分量，
   旧解析当成 0~255，所有用 color-mix 的元素都被算成深灰。
3. 跳过 `[disabled] / [aria-disabled]`（WCAG 明确豁免不可用组件）。

最终结果（1440×900，深浅两套预设）：

```
页面            模式      不合格数
AlbumList      light   0        dark  0
SmartAlbum     light   0        dark  0
ScanHub        light   0        dark  0
Memories       light   0        dark  0
Timeline       light   0        dark  0
Workshop       light   0        dark  0
Home           light   0        dark  0
```

### 2. 单元测试 / 类型 / 构建

```
npm test                             133/133 通过（含 justifiedRows 14 条）
npx vue-tsc --noEmit                 无输出
npm run build                        成功
```

### 3. 几何断言（Playwright 直接量 DOM）

```
album-wall 行宽       1120 = 容器宽（精确铺满）
album-wall-cell       271 / 271 / 271 / 271 + 3×12 gap = 1120
tile-cover 高         212（同行一致）
tile-cap 高           0（已改为压封面的胶囊，不再占流）
行高                  242 = 212 + 30
```

---

## 五、复现

```bash
# 截图（需 dev server 在 1420）
node .visual/shoot-page.cjs AlbumList dark light
node .visual/shoot-page.cjs Home dark

# 对比度审计
node .visual/audit-contrast.cjs            # 默认 6 页 × 深浅
node .visual/audit-contrast.cjs Home       # 指定页面

# 几何探针
node .visual/probe-wall.cjs
```

### ⚠️ 不要把 1420 端口的服务留在后台

1420 是 `tauri dev` 的固定端口（`strictPort: true`）。截图时如果自己起了一个
`npx vite` 占着它，下次 `npm run tauri dev` 会直接失败：

```
Error: Port 1420 is already in use
Error The "beforeDevCommand" terminated with a non-zero status code.
```

而且**已经打开的旧窗口会变成纯白** —— 它连的 dev server 已经不在了，
WebView2 连不上就只画一片白（连启动看门狗都不会触发，因为脚本根本没加载）。
白屏排查顺序：① 端口上还有服务吗；② 窗口是不是旧窗口（关掉重开）。

正确做法：截图前先看 1420 是谁的；若只是要截图，用独立端口：

```bash
npx vite --port 1519 --strictPort &
node .visual/shoot-page.cjs AlbumList dark      # 默认 1420
$env:VITE_PORT=1519; node .visual/shoot-page.cjs AlbumList dark   # 指定端口
```

### 白屏诊断探针

`.visual/app-probe.html` 走**完整启动链路**（真实 `App.vue` + 真实 router +
真实 store + Tauri 桩），把 console.error / window.onerror / 未捕获 Promise
直接打到页面上。与 `themed-probe` 的分工：

| 探针 | 拷什么 | 适合排查 |
|---|---|---|
| `themed-probe.html` | 单个页面组件 | 某页的版式/颜色 |
| `app-probe.html` | 真实 App + 路由 + store | **白屏**（路由守卫 / store 初始化 / 全局组件） |

```bash
node .visual/run-app-probe.cjs    # 需先起 dev server（改脚本里的端口或设 VITE_PORT）
```

- `.visual/themed-probe.ts` 直接挂**真实页面组件**，用桩 invoke 喂 12 个相册；
  页面清单在 `PAGE_PATH` 里加（Home / AlbumList / SmartAlbum / ScanHub /
  Memories / Timeline / Workshop）。
- 截图脚本读 `VITE_PORT`，默认 1420（与 `vite.config.ts` 的 tauri 端口一致）。

---

## 六、不做的事

- 不加侧边栏做一级导航（相册产品靠横向浏览封面）
- 不引图表库（进度条 / 柱阵全部 CSS 画）
- 不改任何 URL 路由（只给 `/smart` 补齐 `?tab=`）
- 不删预设、不动后端 `src-tauri`
- 不恢复渐变 hero / 渐变入口卡
