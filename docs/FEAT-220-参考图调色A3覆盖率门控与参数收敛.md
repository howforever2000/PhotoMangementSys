# FEAT-220 参考图调色：A3 覆盖率门控 + 参数面收敛

> 状态：已实施（2026-11-24）。纯 numpy 改动 + 前端参数面收敛，旧配方零回归。
> 前置：FEAT-219（三层解耦）、FEAT-218（多参考图共性）。

---

## 1. 为什么要改

### 1.1 A3 的外推会把参考图没有的颜色也改掉

FEAT-219 的 A3 是「12 个固定 30° 色相扇区 × 3 影调层」的残差网格。空格（参考图在该
格子没有像素）靠 **60 轮邻域扩散**补，于是参考图的暖色 Δab 会被扩散到整个色环。

清橙胶片（10 张暖橙参考）+ 蓝天素材的实测：**天空被染成品红/暖雾**。这不是「整体暖化」
的预期效果，而是「用橙色去改蓝天」——违背文档 §3.3 自己写下的承诺：
「参考图未覆盖的颜色已保持原样」。

**A/B 实测**（`_ab_out/清橙胶片/_关键_蓝天_fixed12_k4_k4g.jpg`，A3 单层显影、权重放大到 1.0）：

| 变体 | 表现 |
|---|---|
| fixed12（当时线上） | 蓝天被推成暖灰 |
| k4 节点式（无门控） | 天空顶部明显偏品红（节点全落在橙色，蓝天被远距离插值） |
| k4 + 门控 | 天空保住干净的蓝，只有地平线暖化 |

同一轮实验还测了另一条路（「每影调 3–5 主色」替代 12 扇区）：**不带门控时赢一组输一组**
（日系组全面更好、清橙组更差），加门控后两组都不劣。结论是「3–5 vs 12」不是关键变量，
**覆盖率门控才是**，而且它可以独立加在现有 12 扇区上 —— 故本期不换节点方案，只加门控。

### 1.2 参数面：1 个下拉 + 6 个滑条/开关，用户无法理解

工作台链条面板的参数是 `op_catalog.json` 的 `params` **无条件全量渲染**
（`workbenchPlan.paramViews` 只按 `type` 决定控件形态，没有 `hidden`/`dependsOn` 机制），
所以选了「智能分层」仍然显示「明度迁移（经典）」：单步工具页做了互斥，链条侧一处都没做。

另有两个既有问题：

- 链条**新增**一步的 `method` 默认是 `reinhard`（经典统计），不是 adaptive
  （`op_catalog.json` ops 段 default 与 tools 段不一致）；
- `keep_exposure` / `protect_skin` 是 bool，`paramViews` 落到兜底分支后**只显示裸的 true/false，改不了**。

---

## 2. 方案

### 2.1 A3 覆盖率门控（算法）

```
RefProfile.support  (3 影调层 × 36 个 10° 色相格)   ← 只在参考池上算一次
  彩度 ≥4 的参考像素 → 按 (影调层, 色相格) 计数 → ≥ min_count 记 1
  → 环向 (1,2,1)/4 平滑两遍（防台阶）
min_count = max(64, 0.002 × N_ref)

Mapping.support = profile.support

eval_mapping:
  r  = 相邻两扇区线性插值(A3 残差)        # (N, 3 层, 3)
  d  = Σ_z  zw[z] × support_at(hue, z) × r[z]
  # support 为 None 时退化为原来的 Σ zw×r（逐值等价）
```

- **只作用于 A3**。A1 影调曲线、A2 分区色偏完全不变 —— A2 本来就该管「整体风格」，
  所以整体暖调仍然在，被去掉的只是「参考图没覆盖的色相上的编造」。
- 仍然是**逐色函数**（不含任何空间运算），可原样烘 33³ LUT，`.cube` 语义不变。
- 不加用户参数：门控常开，符合「少滑条」的目标。

### 2.2 参数面收敛（UI）

| 改动 | 位置 | 说明 |
|---|---|---|
| 6 个参数标 `hidden: true` | `op_catalog.json` 两段 | `method` / `tone_weight` / `chroma_weight` / `keep_exposure` / `protect_skin` / `l_weight` |
| ops 段 `method.default` → `adaptive` | 同上 | 新建链条步骤不再默认落到经典统计 |
| `ParamSpec.hidden?` + `paramViews` 跳过 | `toolCatalog.ts` / `workbenchPlan.ts` | 新增最小机制；`stepSummary` 保留 hidden 枚举（方法名是有效信息）、跳过 hidden 滑条 |
| 单步工具页删控件 | `ColorTransferTool.vue` | 只留「迁移强度」「保护肤色」「导出 .cube」；options 固定 `method:"adaptive"` |
| 入口 method 缺省值 → adaptive | `ops_local.py` / `workflows/color_transfer.py` | 与 catalog 一致 |

**为什么「隐藏」而不是「删键」**：`catalog.norm_params` 只保留 schema 里存在的 key，
删键会让旧配方里显式调过的 `chroma_weight=0.5` 被静默丢弃、回落默认值；隐藏则原值照旧透传，
旧配方零回归，且 `toolCatalog.test.ts` 的「catalog 键 == TUNABLE_PARAMS」守卫不用改。

**旧算法怎么处理**：`colortransfer.reinhard_transfer_stats` / `histogram_match_sorted`
**保留可执行、界面不可选**。已保存的链条步骤仍带 `method:"reinhard"` 的照旧跑老算法，
新建步骤一律 adaptive —— 不做「删枚举值 → norm_params 静默回落 default」那种无声改义。

### 2.3 覆盖度告知

平均覆盖度低于 0.25 时，作业 warning：
「参考图只覆盖约 N% 的色相范围，其余颜色已保持原样（想要更强的整体偏色，可增加不同题材的参考图）」。
`meta.gateCoverage` 记录数值，便于排查「为什么某张图效果弱」。

实测覆盖度：清橙 10 张 **0.398**、日系 10 张 **0.611**、单张 **0.139–0.213**（只有单张参考会触发提示）。

---

## 3. 实测（生产链路，清橙 10 参考 × IMG_20260505_063809，强度 100%）

| 指标 | 门控前 | 门控后 | 说明 |
|---|---|---|---|
| clip% | 1.99 | 2.21 | 均 < 3.5% 验收门槛（原图自身 4.47%） |
| 对比度 | 1.041 | 1.058 | 略偏高（门槛 [0.95,1.05] 边缘，记录在案） |
| ΔE（相对原图） | 30.0 | 27.4 | 作用量下降，符合「少改没覆盖的颜色」 |
| 5bit 色数比 | 0.962 | 0.913 | 色阶保留略降，仍远高于 0.70 门槛 |
| 高光彩度比 | 0.733 | 0.491 | 高光不再被外推的暖色抬高彩度 |
| 门控前后全图平均 ΔE | — | **3.68** | 差异可感知但不剧烈 |

目视（100% 裁切，`_ab_out/_验收_门控前后_天空裁切.jpg`）：天空从「明显品红」回到
「偏冷的中性灰蓝」。**注意**：天空仍被 A2 整体暖化，这是设计如此 —— 门控去掉的是 A3 编造的品红，
不是「把天空变回蓝」。若希望天空完全不受影响，应降「色调强度」或整体降强度。

---

## 4. 工程改动清单

| # | 文件 | 改动 |
|---|---|---|
| 1 | `python/ai_studio/refgrade.py` | `_support_mask` / `_support_at` / `support_coverage`；`RefProfile.support`、`Mapping.support`；`build_profile` 计算覆盖度；`fit_mapping` 透传；`eval_mapping` 按层衰减 A3 |
| 2 | `python/ai_studio/workflows/color_transfer.py` | `_LOW_COVERAGE_WARN=0.25`；低覆盖度 warning；`meta.gateCoverage`；method 缺省改 adaptive |
| 3 | `python/ai_studio/ops_local.py` | method 缺省改 adaptive；docstring 同步 |
| 4 | `python/ai_studio/op_catalog.json` | 两段 params 加 `hidden`；ops 段 method default → adaptive |
| 5 | `python/tests/test_refgrade.py` | **新增**（见 §5） |
| 6 | `src/utils/toolCatalog.ts` | `ParamSpec.hidden?` |
| 7 | `src/utils/workbenchPlan.ts` | `paramViews` 跳过 hidden；`stepSummary` 保留 hidden 枚举 |
| 8 | `src/utils/aiPresets.ts` | `CT_METHODS` 注释改为「仅兼容旧档，界面不再提供」 |
| 9 | `src/utils/toolCatalog.test.ts` | 修 TUNABLE_PARAMS 解析正则（允许折行，见 §6） |
| 10 | `src/components/workshop/ai/ColorTransferTool.vue` | 删方法下拉与 4 个控件、固定 adaptive、文案补「参考图里没有的颜色会保持原样」 |

**不改**：`colortransfer.py`（reinhard/histogram 原样保留）、`autograde.py`、`lutcube.py`、
`recipecatalog.py` 的白名单（键没删，守卫照旧）。

---

## 5. 测试（`python/tests/test_refgrade.py`，14 个用例）

FEAT-219 §6 清单的 10 条 + 门控 4 条：

1. 恒等：`strength=0` 与输入逐字节一致
2. 影调单调 + 锚定：合成亮度斜坡输出 L 不反转、中位偏移 ≤2
3. 无层台阶：软权重混合后 5bit 色数比 ≥0.90、无孤立 ΔL 大跳变
4. 色域回收只缩 C：越色域目标经 headroom + sRGB 裁剪后色相漂移 ≤1.5°
5. 肩部等比：knee 以下逐字节不变、暖高光彩度比 ≥0.90
6. LUT 等价 + `.cube` 往返互逆（轴序 [b,g,r]）
7. 多参考聚类：2 暖 + 1 冷 → 冷被剔除；单张直通
8. 肤色门控：protectSkin 把肤色色度位移压到 ≤45%
9. 旧路径 golden：reinhard / histogram 输出哈希锁定（改动前录制），且确定性
10. 双层限幅：结构性（A2 ‖Δab‖≤30、A3 ‖Δab‖≤5.4、|ΔL|≤5.4）+ 渲染性（C≤60 时 ‖Δab‖≤42）
11. **门控：覆盖度全 1 与不门控等价，且真实覆盖度下门控真的起作用**
12. **门控：参考未覆盖的色相，A3 作用量降到不门控的 50% 以下**
13. **门控：不影响 A2 分区色偏（逐值相等）**
14. **门控：support 形状/值域/`support_coverage` 语义**

> 说明：FEAT-219 记录的 `test_refgrade.py` 当时**并未入库**（git 无任何历史），
> refgrade 此前零单测覆盖。本文件是本仓库里第一份 refgrade 单测。

---

## 6. 顺带修的问题

### 6.1 `norm_params` 的 bool 缺省值（影响出片质量）

`catalog._norm_one` 对 `bool` 直接 `return bool(raw)`。缺省时 `raw is None` → **`False`**，
于是 `color_transfer.keep_exposure`（schema 默认 `true`）在链条步上一律变成 `false`：

```python
>>> norm_params("color_transfer", {})
# 修复前: {'keep_exposure': False, ...}     ← 曝光锚定被静默关掉
# 修复后: {'keep_exposure': True,  ...}
```

FEAT-219 的核心结论之一就是「锚定素材中位亮度」把 100% 强度的 clip 从 20.1% 压到 13.2%；
链条路径一直在关着它跑。本期既然把开关藏进 `hidden`（用户已无法在 UI 里翻回来），
必须同时修掉：缺省回 schema default，字符串按真值解析（`bool("false")` 曾得到 `True`），
显式传入的值一律保留。回归用例见 `python/tests/test_catalog_norm.py`。

### 6.2 守卫测试的折行正则

`toolCatalog.test.ts` 的 TUNABLE_PARAMS 解析正则写成 `frozenset({...})` 紧邻，
不允许 `frozenset(` 与 `{` 之间、`}` 与 `)` 之间有换行。FEAT-219 把 `color_transfer`
的键加到 8 个后 ruff 折行成：

```python
"color_transfer": frozenset(
    {"method", "strength", ...}
),
```

正则从此匹配不到这个算子，**守卫对它静默失效，`npm test` 自 FEAT-219 起一直是红的**
（FEAT-219 的验收记录里只写了 `vue-tsc` 与 python 回归，没有 `npm test`）。
正则改为允许空白后 356 项全过。

---

## 7. 风险与回滚

| 风险 | 等级 | 缓解 |
|---|---|---|
| 单张参考覆盖窄 → 效果变弱 | 中 | warning 明示覆盖度 + 建议加参考图；A2 仍会给整体风格 |
| 对比度略升到 1.058（门槛边缘） | 低 | 记录在案；可用强度滑条收敛 |
| 旧配方 `method=reinhard` | 低 | 保留可执行；`hidden` 不删键，值照旧透传 |
| 一键回滚 | — | 删除 `Mapping.support` 的赋值（`fit_mapping` 里一行）即回到 FEAT-219 行为；前端把 `hidden` 去掉即恢复全部滑条 |

---

## 8. 非目标

- 换掉 12 扇区为「每影调 3–5 主色」节点方案（实验已做：无门控时互有胜负，带门控才不劣；
  收益主要是可解释性，留待需要时另立项）
- 空间化 mask（人脸/天空分区）：会破坏「逐色函数 → 可烘 LUT」的性质
- 删除 reinhard / histogram 的后端代码
