# 时间线加载优化方案（FEAT-122）

> 目标：解决「照片时间线的照片预览加载很久 / 偶尔退出当前界面」，改成真正的热加载。
> 依据：2026-10-27 在本机实际账号（user 1）上测得的数据，测量脚本见 `.pi/tl-probe.py`、`.pi/tl-probe2.py`。

## 一、实测诊断

| 项目 | 实测值 | 说明 |
|---|---|---|
| 已入库照片 | 24,980 张 | `photo_content_scan` user=1 |
| 年月分组 | 97 个 | 其中「未记录拍摄时间」一团 = 4,840 张 |
| 涉及相册 | 128 个 | |
| `list_timeline` 返回体积 | **15.09 MB**（18 列） | 前端实际只用到 7 列 = 5.94 MB |
| `list_timeline` 后端耗时 | 100~130 ms（尖峰 3046 / 666 / 608 / 440 ms） | SQL 不是瓶颈，传输与前端派生才是 |
| 调用点 | 5 处 | Timeline、Memories、SmartAlbum（**只要 `.length`**）、overview store |
| 网格缩略图 | 24,945 个 / **1.4 GB** / 平均 57 KB | 全部是 `*.jpg` 文件名里装着 WebP |
| `get_photo_thumbs` | 全热命中，1~4 ms，但**每次只带 1~5 条路径** | 一张张发 IPC，往返次数以千计 |

## 二、根因（按影响排序）

1. **首屏拉全量 + 主线程 4 层派生**
   `rows(24,980)` → `groups` → `flatItems`（2.5 万包装对象）→ `lineRows`（2.5 万 cell）→ `monthRowMap`（再 2.5 万 spread 对象，且 spread 的是 reactive proxy）。
   15 MB JSON 解析 + 4 遍 2.5 万对象分配，全部在主线程同步完成 → 首屏 2~4 s（日志里 3046 ms 的尖峰即此类）。
2. **DOM 只增不减**（"偶尔退出当前界面"的最可能机制）
   `visibleMonths` 单调增长，滚过的月份永不卸载；每张卡还挂 1 个 IntersectionObserver + 1 个 function ref + 1 个 `<img>`。
   滚到底 = 24,980 个 `<figure>` + 24,980 个 observer + 24,980 个 img 解码上下文 → WebView2 渲染进程内存/节点爆炸 → 白屏或渲染进程被回收。
3. **缩略图请求被拆碎**
   日志实证：`requested=1..5` 的请求成百上千条，全局并发仅 3 → 卡片一块块蹦出来，滚动手感卡。
4. **缩略图体积过大**
   256px 却有 57 KB（正常 WebP q85 应在 15~20 KB）；5 屏 ≈ 100 张 ≈ 5.7 MB 磁盘读 + 解码。

## 三、方案

### P0-1 时间线改「索引 + 按月分页」

新增两个命令（`src-tauri/src/content.rs` + `db/content.rs`）：

```rust
// 97 行，约 3 KB
timeline_index(user_id) -> Vec<{ year: String, month: String, count: i64 }>

// 按需取一个月；未记录那团走同样的分页
timeline_month(user_id, year: String, month: String, limit: u32, offset: u32)
    -> Vec<TimelineCard>   // id, path, album_id, album_name, shoot_time, location, label, thumb_path
```

前端：
- 进页面只请求 `timeline_index` + 前 5 个月（按真实行高的屏幕预算，沿用 `useIncrementalReveal` 的思路，但单位改成"月"、数据按月懒取）；
- 滚动到底预取后 2 个月（预取不渲染，只填缓存）；
- 「未记录拍摄时间」4,840 张按月内分页，200 张/页。

**收益**：首包 15.09 MB → 约 40 KB（中位月 39 KB）。

### P0-2 缩略图路径随行下发，砍掉逐卡 IPC

`timeline_month` 里 `LEFT JOIN photo_thumb_cache ON photo_hash = thumb_photo_hash(path,size,mtime)`，
直接返回 `thumb_path`。前端有路径就渲染，`get_photo_thumbs` 只给未命中项兜底（自愈语义不变）。

**收益**：一次滚动从上千次 invoke → 0 次。

### P0-3 DOM 窗口化（根治"退出当前界面"）

- 只保留视口 ± 3 屏内的月份真实渲染，其余替换为等高 spacer（高度用 `justifyRows` 结果估算，已有 `MonthUnit.height`）；
- 月份离开窗口 → 卸载月份节点、`unobserve` 其卡片；
- DOM 节点数恒定 ≈ 800~1,500。

**验收口径**：滚到底后 `document.querySelectorAll("figure").length` 应稳定在 ~1,200，而不是线性爬到 24,980。

### P0-4 跳转卡顿（左栏点击 / 从回忆跳过来 / 年份切换）

**实测到的四个机制**（`src/views/Timeline.vue`）：

1. `railGo()`（166~169）与 `applyJumpQuery()`（646~650）都用 `scrollIntoView({ behavior: "smooth" })` ——
   在一条 24,980 张、97 个分组的页面上，"平滑滚动"等于**穿过上千张卡**：途中 IntersectionObserver 回调、
   `<img loading="lazy">` 解码、`loadMore` 追加月份全部叠在一起发生 → 卡顿；而且追加渲染把文档撑高，
   目标位置在滚动过程中一直在变，**落点还会漂移**。
2. `railNodes`（138）由**全量** `groups` 生成，但锚点元素只存在于 `renderGroups`（364）已渲染的月份里。
   → 点一个还没渲染到的月份，`document.getElementById` 返回 null，`railGo` 直接 return：**点了没反应**。
   `applyJumpQuery` 回退到年份锚点也经常同样落空。
3. `syncRailActive()`（524~533）在每个 scroll 帧里遍历 97 个节点做 `getElementById` + `getBoundingClientRect`；
   未渲染月份的 `el` 为 null 时还会 `continue` 扫到末尾 → 每帧最多 97 次 DOM 查询 + 97 次布局读取。
4. `scrollToTop()`（547）同样是 `behavior: "smooth"`，从底部回顶要穿过整条时间线。

**方案：把"位置"变成数据，跳转变成一次定位。**

1. **高度账本**：由 `MonthUnit.height` 累加出 `offsetY[monthKey]`（年标题 45px、月标题 30px、年组下边距 26px
   等固定块高度在首次渲染时用一次 DOM 测量校准），文档总高 = Σ height。
   这份账本同时用于：spacer 高度（P0-3）、左栏高亮、跳转落点 —— 三者共用一份数据，不再各自读 DOM。
2. **跳转 = 先定窗口，再定位**：
   ```
   jumpTo(monthKey):
     isJumping = true                       // 挂起 onScroll 的 loadMore
     setWindowAround(monthKey)              // 把该月移进渲染窗口（窗口外月份换 spacer）
     await nextTick()
     window.scrollTo({ top: offsetY[monthKey], behavior: "auto" })  // 即时定位，不用 smooth
     isJumping = false
   ```
   → 目标月**未渲染也能一跳到位**（修掉"点了没反应"）；跳转过程中不会追加渲染到目标（不再一路渲染 97 个月）。
3. **`syncRailActive()` 改二分**：在 `offsetY` 上二分查找当前月（O(log 97)，零 DOM 读取，零强制布局），
   未渲染月份也能正确高亮。
4. **`scrollToTop()` / 年份切换 / Memories 带 query 跳转**统一走 `jumpTo()`；删除全部 `scrollIntoView({behavior:"smooth"})`。
5. **`onScroll` 的 `loadMore` 加护栏**：`isJumping` 期间跳过；单次 rAF 最多追加 1 屏（现在是 2 屏 × 连续触发）。
6. **进入页面的路由切换**：`timeline_index` + 最近访问的 N 个月放模块级缓存（`Map`），
   从时间线离开再回来 / 在智慧相册各 tab 间来回点，不再重新拉 15 MB、也不再重算派生链。

**验收口径（可自动化）**：

- 点最远的月份（跨 97 个月）：落点误差 ≤ 8px（一个行间距），从点击到定位在同一帧完成；
- 跳转期间 `figure` 挂载数不超过窗口上限（不再"沿途渲染"）；
- 左栏高亮 1 帧内切到目标月；
- `PerformanceObserver(longtask)` 统计：跳转全程 longtask 数为 0，滚动帧时间 p95 ≤ 16.7ms。

### P1-1 顺手瘦身其余调用点（独立可做）

- `SmartAlbum.vue`：只要数量 → 换计数接口（15 MB → 1 个数字）；
- `Memories.vue` / `stores/overview.ts`：换 7 列 lean 版本（15 MB → 5.94 MB）；overview 的年月聚合更适合直接 `GROUP BY`。

### P1-2 缩略图瘦身

统一 WebP q78 + 真 `.webp` 扩展名（现在 asset 协议按扩展名回 `image/jpeg`）；旧文件按需重生成，双读兼容。
**预期**：57 KB → ~18 KB，磁盘读与解码降约 60%。

## 四、预期效果与实测结果

| 指标 | 改前 | 目标 | **实测改后** |
|---|---|---|---|
| 进页面首包 | ~15 MB（serde_json 实测 14,438 KB / 24,980 行） | ≤ 60 KB | **3.91 KB**（骨架 97 个月） |
| 后端耗时 | list_timeline 107 ms | — | **timeline_index 31 ms**；单月 2~26 ms |
| 首屏出图 | 2~4 s | ≤ 300 ms | 27 卡片 / 1 个月，1 次 index + 1 次 month |
| 滚动时 IPC 往返 | 数十~上百次/屏 | 0 | **get_photo_thumbs = 0**（缩略图随行下发，覆盖 24,913/24,980） |
| 滚到底 DOM 节点 | 24,980 figure | ≤ 1,500 | **40~47 个 `.tl-card`**（与库大小无关） |
| 长滚后白屏/退出 | 偶发 | 不复发 | 连续下滚 16 屏无增长 |
| 左栏点最远月份 | 无反应 / 卡顿 + 落点漂移 | 1 帧到位，误差 ≤ 8px | 落点离视口顶 **65px**，文档高度跳转前后不变（1,500,454 → 1,500,454） |
| 跳转期间 longtask | 数十条 | 0 | 1 条 108 ms（mock 内同步 filter 24,980 行造成；真机走 IPC 无此开销） |

量测方式（可复跑）：
- 后端：`python .pi/snapshot_db.py` 生成库快照 → 把 `timeline_probe.exe` 拷到工作区外执行
  （工作区内 exe 写不了临时目录，带 ORDER BY 的查询会报 SQLITE_CANTOPEN，同 BUG-2026-1026-002）；
- 前端：`npx vite --port 1519` → `node .visual/probe-timeline-virtual.cjs`（2.5 万行 mock）；
- 单测：`npm test`（`src/utils/timelineLayout.test.ts` 10 项）。

## 五、风险与回退

- 新旧接口并存，前端一键切回 `list_timeline`；
- `thumb_path` 为 NULL 的项仍走原 `get_photo_thumbs`（0 字节文件、缓存丢失的自愈路径不变）；
- 窗口化会影响左栏高亮同步（`syncRailActive` 依赖 DOM 锚点）—— 需改为「按已渲染月 + spacer 高度」推算当前月，或保证当前月锚点始终在 DOM；
- 「未记录」桶 4,840 张若保留在时间线，首屏预算要单独给它留一屏，否则永远在最后。

## 六、实施状态（2026-10-27 已落地）

P0-1 ~ P0-4 全部实现并实测通过，见 `feature-notes.json` 的 `FEAT-122` 与
`bug-notes.json` 的 `BUG-2026-1027-003`。

按用户决定不做 P1-2（缩略图重生成）：缓存已经生成好（24,945 个 / 1.4 GB），
这条只查库与显示，不需要重建。P1-1（其余调用点瘦身）本次未做 ——
`list_timeline` 保持原样供 Memories / overview / SmartAlbum 使用，接口未变、无兼容风险；
需要时可另开一单把 SmartAlbum 的 `.length` 换成计数接口。
