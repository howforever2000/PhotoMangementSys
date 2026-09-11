# 语义分类（v5）落地方案与实测记录

> 记录日期：2026-09-20 · 关联：FEAT-056（语义分类）· 前置 FEAT-055（语义搜索）
> 本文是该功能的"事实台账"：为什么这么设计、P0 实测数据、阈值怎么定的、
> 哪些模型下线了、验证到什么程度、还留了什么坑。

---

## 1. 目标与决策

| 项 | 决策 |
|---|---|
| 分类来源 | **下线图像分类模型**（yolov8*-cls + Places365 + 花朵/食物专家），改由 Chinese-CLIP **语义关键词匹配**产出 |
| 用户自建分类 | 用户建分类 → 写关键词（自然语言短语）+ 排除词 + 调匹配强度阈值 → 命中照片自动归入 |
| 多标签 | 一张照片可属于多个分类（`photo_category_hits` 多对多） |
| 保留的规则通道 | 人物检测（YOLOv8n-det + 人脸标号）、夜景（影调自算）、文档（PaddleOCR + 截图启发式） |
| 模型档位 | B/16（默认，512 维）↔ L/14-336（可选，768 维）；前端下拉选择，与旧"分类模型"下拉同构 |
| 硬件红线 | 核显本（7840HS）。CLIP 固定 CPU（fp16 在 AMD DML 上有算子级数值 bug，见 BUG-2026-0910-006） |

**为什么保留人物/夜景/文档**：这三条不是"分类模型"，且语义做人脸标号（人名）和
OCR 文档判定都不可靠。它们继续写入 `photo_content_scan.category`，再由 SQL 物化成
builtin 分类命中。

---

## 2. P0 实测（阈值从哪来）

工具：`python/bench/calibrate_categories.py` / `calib_center.py` / `calib_debias.py` / `calib_top.py`
数据：本机真实相册库 **11372 张缩略图**的 Chinese-CLIP B/16 向量（`python/bench/out/embeddings.npy`）

### 2.1 后端一致性
生产链路（fp16 拆分件，CPU）重编码 5 张缩略图 vs bench（fp32 导出，DML）向量：
**cos = 1.0000** → bench 向量可代表生产链路。

### 2.2 裸余弦不可用
| 关键词 | p50 | p95 | max | 命中@0.38 |
|---|---|---|---|---|
| 一只猫 | 0.337 | 0.381 | 0.451 | 647 |
| 美食 | 0.363 | 0.407 | 0.461 | 3337 |
| 文档 | 0.330 | 0.361 | 0.398 | ~0 |

无关图对的余弦普遍落在 **0.33~0.39**，且**不同概念的基线不同**（美食 0.363 / 文档 0.330）。
单一绝对阈值在 0.30 命中 99% 图库、在 0.38 的边界处精度已崩：
「一只猫」阈值 0.38 的第 591~595 名经视觉模型抽查 **5 张仅 1 张有猫**。

### 2.3 均值中心化（P0b）——无效
`cos(x-μ, t-μ)` 把 p50 拉到 0.17，但低信息量图片变成 "hub"（同一张图对**所有**关键词
都排第一），反而更糟。已否决。

### 2.4 中性基线去偏（P0d）——采用
```
base(x)     = mean_b cos(x, b)          # b = 7 条中性提示词（"一张照片" 等）
score(x, c) = max_kw cos(x, kw) - base(x)   # 净语义增益
命中         = score >= threshold 且 排除词未更优
```
中性基线 p50=0.386（高于绝大多数关键词），减去后不同概念的尺度被吸收，且
**图库里不存在的概念自然 0 命中**（不会像裸余弦那样硬凑出 top-N 噪声）。

**抽查精度（视觉模型判读）**：
| 关键词 | 阈值 0.03 命中 | 抽查 | 判对 |
|---|---|---|---|
| 一只猫 | 30 | 10 张（随机抽命中集） | 7 |
| 美食 | 297 | 10 张（随机抽命中集） | 9 |
| 文字 / 文档 | ≈0 | — | 库中确实没有截图类照片 |
| 屏幕截图 | 370 | — | — |

**默认阈值 0.03**（UI 滑块 0.00~0.10，预设 宽松 0.02 / 标准 0.03 / 严格 0.05）。

### 2.5 真实 App 库复核（P4）
用真实 `photos.db` 的 `photo_embeddings`（10919 条，`model=chinese-clip-vit-b16-fp16`）
复算同一公式：

| 分类 | 关键词 | 阈值 0.03 命中 |
|---|---|---|
| 猫咪 | 一只猫 / 小猫 / 猫咪 | 49 |
| 美食 | 美食 / 食物 / 菜肴 / 甜点 | 297（与 P0 完全一致） |
| 汽车 | 汽车 / 车辆 / 摩托车 | 4 |
| 文字截图 | 文字 / 文档 / 屏幕截图 | 371 |

> 注意 2.4 的"文字/文档 ≈0"与这里的"屏幕截图 371"差异：**屏幕截图**这个词才是有效的，
> 说明**关键词措辞强烈影响召回**——这也正是要在 UI 里给实时预览的原因。

---

## 3. 数据模型

```sql
photo_categories(       -- 分类定义
  id, user_id, name, icon, source('builtin'|'preset'|'user'), slug,
  keywords(JSON), exclude_keywords(JSON), threshold, sort_order, enabled,
  created_at, updated_at, UNIQUE(user_id,name))

photo_category_hits(    -- 命中物化（多对多）
  category_id, user_id, photo_hash, path, score, matched_keyword, updated_at,
  PRIMARY KEY(category_id, photo_hash))

clip_text_cache(        -- 关键词向量缓存（改阈值不重编码文本）
  model, text, dim, embedding(BLOB f32 LE), generated_at, PRIMARY KEY(model,text))
```

**为什么物化**：浏览必须秒开且要计数 + 封面 + 与筛选叠加；实时现算拿不到这些。
重建成本很低（关键词向量有缓存，匹配是全内存点积：1 万张 × 512 维 ≈ 5.6M 乘加/关键词）。

**生命周期同构**：`photo_category_hits` 冗余 `path`，与 `photo_content_scan`、
`photo_embeddings` 三处同步维护（删除照片 / 删除相册 / 移动照片）。

---

## 4. 分层落位（CS1 / CS2）

```
接口层   src-tauri/src/category.rs :: commands（薄壳：参数校验 + 转发 + 出入口日志）
服务层   src-tauri/src/category.rs（关键词向量解析 / 基线 / 打分 / 重建 / 预览）
持久层   src-tauri/src/db/category.rs（表 + CRUD + 批量命中 + 聚合 + 级联）
外部     python /embed_text_batch（批量文本编码，封顶 64/次）
前端     CategoryGallery.vue（分类画廊，复用 FEAT-048 骨架）
        CategoryManager.vue（原子组件：列表 + 编辑 + 实时预览 + 重建）
```

关键命令：`list_categories` / `save_category`（保存即自动重建该分类）/
`delete_category` / `preview_category`（不落库）/ `rebuild_categories` /
`list_category_photos` / `category_index_stats`。

扫描完成后自动重建全部命中（含 builtin），失败不阻塞扫描结果。

---

## 5. 扫描管线变化

| 通道 | 处置 | 说明 |
|---|---|---|
| yolov8*-cls（x/l/m/s/n） | **删除** | 配置、注册表槽位、下载清单、UI 下拉全部移除 |
| Places365 scene | **删除** | 夜景判定改纯影调（`avg_luma < 45`） |
| 花朵 / 食物专家 | **删除** | 语义分类可覆盖（写"花朵""美食"关键词） |
| YOLOv8n-det + SCRFD/ArcFace | 保留 | 扫描项 `ai` → **`person`**（前端文案"人物 · 文档识别"） |
| 影调 tone | 保留 | 夜景来源 |
| OCR | **触发条件放宽** | 原来靠 `cls ∈ {text, other}` 门控；cls 下线后模型可用即跑（~90ms/张） |
| 语义向量 semantic | 升为主通道 | 用户分类匹配的唯一数据源 |

单张耗时：**437ms → ~140ms**（实测 warm 138ms/张，含 det+tone+OCR；人脸按需）。

`VCR_API_VERSION` 4 → **5**（宿主检测到旧服务会 `POST /shutdown` 自动重启）。

---

## 6. 模型档位（适配不同硬件）

| 档位 | 维度 | 输入 | 整图文件 | 体积 | 建议硬件 |
|---|---|---|---|---|---|
| `b16`（默认） | 512 | 224 | `Xenova/chinese-clip-vit-base-patch16` → `model_fp16.onnx` | 377 MB | 任何核显本；零样本 81.1%（53 张标注集） |
| `b16-fp32` | 512 | 224 | 同仓库 → `model.onnx` | 719 MB | 想用核显加速语义索引；**已实测可走 DirectML** |
| `l14` | 768 | 336 | `Xenova/chinese-clip-vit-large-patch14-336px` → `model_fp16.onnx` | 814 MB | 独显/强 CPU；**本机未实测精度** |

落位约定（`python/models/` 下）：
```text
<root>/onnx/<onnx>      ← 双塔整图（首次使用时服务端自动拆成 clip_vision/clip_text）
<root>/tokenizer.json   ← ⚠️ 必须在模型根目录，不是 onnx/ 子目录（BUG-2026-0920-002）
<root>/vocab.txt
```
档位键与目录一一对应：`b16→chinese-clip`、`b16-fp32→chinese-clip-fp32`、`l14→chinese-clip-l14`；
Rust 下载表与 Python 档位表由单测 `model_dl::tests::clip_specs_match_python_tier_table` 强制对齐，
CLI (`download_models.py`) 的任务表由 `config.CLIP_MODEL_META` **派生**（单一事实源，三处不再各写一份）。

### 6.1 provider 策略（按档位，不是一刀切）

| 档位 | provider | 依据 |
|---|---|---|
| `b16` / `l14`（fp16） | **固定 CPU** | fp16 图在 AMD DML 上有算子级数值 bug（BUG-2026-0910-006） |
| `b16-fp32` | 跟随「GPU 加速」开关（默认关） | 实测 DML 与 CPU **数值完全一致**、快约 2× |

> 顺带修掉一个潜在缺陷（BUG-2026-0920-003）：此前 CLIP 直接用全局 provider，
> 用户一旦打开「GPU 加速」开关就会把 fp16 双塔送上 DML 并**静默产出错误向量**。

### 6.2 实测数据（`python/bench/verify_clip_tiers.py`）

样本：真实库 24 张缩略图（24 张同批，两档同输入协议）

| 对照 | 图塔余弦 min | 文塔余弦 min | 速度 |
|---|---|---|---|
| `b16` CPU ↔ `b16-fp32` CPU | **0.999998** | **1.000000** | 120.3 vs 105.5 ms/张 |
| `b16-fp32` **DML** ↔ `b16-fp32` CPU | **1.000000** | **1.000000** | **53.3 vs 108.2 ms/张（≈2.0× 提速）** |

结论：① fp32 档可安全启用 GPU（数值无损伤，与 fp16 同空间）；
② fp16/fp32 两档在 CPU 上数值等价（余弦 ≥0.999998）——
**意味着同空间档位间切换理论上无需重建索引**（当前仍按 model id 严格隔离，见 §8.5 待办）。

- 落库 `photo_embeddings.model` 用档位 `id`（`chinese-clip-vit-b16-fp16` /
  `chinese-clip-vit-b16-fp32` / `chinese-clip-vit-l14-fp16`）——**历史值不可改**，换档必须换 id。
- 检索 / 匹配 / 增量差集 / 缓存版本 **全部按 model 过滤**；换档后旧向量计入 `stale`，
  分类页提示"需重建索引"。
- 下载：应用内 `chinese-clip` / `chinese-clip-fp32` / `chinese-clip-l14`（hf-mirror 直链 + tokenizer/vocab，
  含瞬时 403 有限重试），或 `python download_models.py --tasks clip,clip-fp32,clip-l14`。
- **fp32 档的 GPU 开关默认未开启**：需先做数值一致性验证（fp32+DML vs fp32+CPU 编码同 N 张，
  余弦应 >0.999）后再放开 provider；验证通过前该档等价于「CPU 慢速＋数值略精确」，价值有限。

---

## 7. 验证记录

| 项 | 结果 |
|---|---|
| `cargo check` | 0 warning |
| `cargo test --lib category` | **13/13 通过**（8 个持久层用例 + 5 个打分公式用例） |
| `cargo test --lib model_dl` | **2/2 通过**（下载落位与 Python 档位表一致性守护 + 三档条目完整性） |
| `python/bench/verify_clip_tiers.py` | 档位一致性：fp16↔fp32 CPU 余弦 ≥0.999998；fp32 DML↔CPU 余弦 =1.000000，提速 2.0× |
| `cargo test --lib` 全量 | 79 通过 / 4 失败 —— 4 个失败在 `logger::tail_*` 与 `thumbnail::*`，**与本次改动无关**（对应源文件未修改，属既有环境相关用例） |
| `vue-tsc --noEmit` | 0 错误 |
| `npm run build` | 成功 |
| Python 服务端到端 | `/health`（api_version=5 / model_id 正确）、`/models`（档位清单）、`/embed_text_batch`（512 维）、`/classify_batch`（规则通道）、`/benchmark`（det 58ms）均通过 |
| 真实库公式复核 | 见 §2.5（美食 297 与 P0 完全一致） |

---

## 8. 已知限制与后续

1. **分类只覆盖已建向量索引的照片**。语义扫描没跑过的照片不会出现在语义分类里——
   分类页常驻"语义索引 N / 已入库 M 张"，空索引时给出去扫描中心的引导。
2. **关键词措辞影响大**（§2.5：`文字` 无效、`屏幕截图` 有效）→ 依赖实时预览校准；
   后续可加"同义词扩展/推荐关键词"。
3. **绝对阈值仍是近似**：去偏后尺度可比，但概念间仍有差异（美食最高 0.077 vs 汽车 0.035）。
   已用"每分类独立阈值 + 实时预览 + 分布分位展示"兜住；后续可考虑按库分布自适应。
4. **L/14-336 未实测**：精度/速度门（准确率提升 ≥5pp 且 CPU ≤250ms/张）尚未跑，
   UI 标注为"未实测"。要下结论需先下载模型 + 跑 53 张标注集对照。
5. **fp32 档 GPU 已可启用**（默认仍关）：实测 DML 数值与 CPU 完全一致、快 2×；
   用户可在「⚙ 性能设置 → 🚀 启用加速」开启（fp16 档不受影响，始终 CPU）。
6. **同空间档位切换仍要求重建索引**：实测 b16 fp16↔fp32 向量等价（余弦 ≥0.999998），
   但当前按 model id 严格隔离 → 切档会提示重建（1.1 万张 CPU 约 17 分钟）。
   优化方案：引入 `space` 概念（同一 `space` 的多个 model id 共享索引，SQL 用 `model IN (...)`），
   改动涉及 embedding/content/category 三处过滤，建议单独一轮做 + 补测试。
7. **发布版（PyInstaller exe）无 GPU**：打包 venv 只装 `onnxruntime`，不含 `onnxruntime-directml`，
   而 UI 有「GPU 加速」开关（见 BUG-2026-0920-004）。要么把 directml 打进发布包，要么在 UI 上
   把加速标为"开发环境/需自备运行时"。
5. **CLIP 的 GPU 加速未启用**（fp16 数值 bug）。若要提速，可研究 fp32 模型 + DML
   （Phase 0 曾测 37.9ms/张 vs CPU 91ms），但需先验证 fp32 索引 + fp16 文本塔的跨塔一致性。
6. 旧库中 YOLO 时代写入的 `photo_content_scan.category`（landscape/animal/food/flower…）
   保留不删（不做不可逆清理），UI 不再展示，新扫描只写 4 个规则类别。

---

## 9. 复现步骤

```bash
# 1) 阈值校准（真实库向量已在 python/bench/out/）
python python/bench/calibrate_categories.py      # 全量分位 + 命中数
python python/bench/calib_debias.py 0.03         # 去偏方案 + 边界样张
python python/bench/calib_top.py "一只猫" 0.38 5  # 指定关键词 topN（人工/视觉抽查）

# 2) 语义模型拆分（首次使用自动做，也可手动）
python python/extract_clip_subgraphs.py b16

# 3) 服务端冒烟
python python/server.py   # 另开终端 curl /health /models /embed_text_batch

# 3b) 档位一致性 / GPU 数值对照（换档或开 GPU 前跑）
python python/bench/verify_clip_tiers.py b16 b16-fp32
python python/bench/verify_clip_tiers.py b16-fp32 b16-fp32   --provider-a DmlExecutionProvider --provider-b CPUExecutionProvider

# 4) 应用内：扫描中心 →（至少勾「语义向量」）扫描 → 内容分类页 → ⚙ 管理分类
```
