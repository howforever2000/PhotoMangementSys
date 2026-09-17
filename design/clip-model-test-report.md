# CLIP 语义搜索 · Phase 0 模型测试报告

> 测试日期：2026-09-10 ｜ 测试机：AMD Radeon 780M 核显（DirectML）+ CPU
> 测试脚本：`python/bench/`（bench_clip.py / embed_all.py / eval_clip_zero.py / search_check.py / minitok.py）
> 测试素材：本机真实缩略图库 11,372 张（`thumbs/`，256px）+ `ground_truth.json` 53 张已标注原图

## 1. 结论摘要

| # | 结论 | 数据支撑 |
|---|---|---|
| 1 | **方案可行，10s 预算余量巨大**：单次查询（文本编码+全库检索）实测 **< 15ms** | §3.2 / §3.3 |
| 2 | **全库索引很快**：11,372 张实际 **9.3 分钟跑完（20.5 张/s，0 失败）**，5 万张外推 ≈ 41min（DML）；CPU 兜底 ≈ 1.3h | §3.1 |
| 3 | **零样本分类 81.1% > 现有 yolov8m 管线 76.0%**，且支持开放词汇（"海边日落"直接命中分类盲区图） | §4.1 / §4.2 |
| 4 | **int8 掉点 3.8pp > 2pp 决策门 → 默认档选 fp16/fp32**（向量漂移 cos 0.96 独立印证） | §4.1 / §5 |
| 5 | 缩略图直编码正确：预处理仅 2.8ms/张，跳过原图解码的策略成立 | §3.1 |
| 6 | 不引入 sqlite-vec 正确：暴力余弦 1.1 万张 0.36ms / 10 万张 ≈ 4ms | §3.3 |

## 2. 测试对象

- 模型：**Chinese-CLIP ViT-B/16**（OFA-Sys 权重，`export_clip_onnx.py` 自导出 ONNX opset17，torch↔onnxruntime 数值对齐 cos≈1.0）
- 文件：vision fp32 345MB / text fp32 409MB / vision int8 87MB（动态量化）
- 向量：512 维 fp32 = 2KB/张（与实现方案预算一致）
- 推理：onnxruntime-directml 1.24.4（`DmlExecutionProvider` 可用，`model_registry.py` 的 provider 机制可直接继承）

## 3. 性能实测

### 3.1 图像编码（索引路径，缩略图直编码）

| 配置 | ms/张 | 全库 11,372 张 | 5 万张外推 |
|---|---:|---:|---:|
| CPU fp32 b=1 | 91.4 | 17.3 min | ~76 min |
| CPU fp32 b=32 | 97.6 | 18.5 min | ~81 min |
| CPU int8 b=1 | 42.3 | 8.0 min | ~35 min |
| **DML (780M) b=1** | **37.9** | **7.2 min** | **~31 min** |
| DML b=32 | 44.6 | 8.5 min | ~37 min |
| **全库实跑（DML b=32 + 预处理）** | 48.8 | **9.3 min** | **~41 min** |

- 预处理（解码+resize224+normalize，PIL 单线程）：**2.8 ms/张**，可忽略
- 发现：780M 上 **b=1 优于大 batch**（37.9 vs 44.6ms），索引管线用小批异步流水即可
- 53 张**原图**（数 MB）fp32 DML 编码 82ms/张 → 缩略图直编码比原图快 2 倍以上，且质量验证（§4）均基于缩略图向量

### 3.2 文本编码（查询路径，52 token 定长）

| 配置 | 实测 |
|---|---:|
| DML，batch=10 | **6.8 ms/条** |
| CPU 默认线程，batch=4 | ~25 ms/条 |
| CPU 2 线程，batch=10 | 64 ms/条（线程数影响巨大，勿限 2 线程） |

### 3.3 向量检索（Rust 侧暴力余弦量级模拟，numpy）

| 库规模 | 全库点积 | 内存（fp32） |
|---|---:|---:|
| 11,372 × 512（本库实测） | **0.36 ms** | 23 MB（embeddings.npy 实测 23.3MB） |
| 10 万 × 512（外推） | ≈ 4 ms | ≈ 200 MB（int8 存储 50MB） |

端到端单次查询 = 文本编码 ~7ms + 检索 <5ms + top-k 排序 ~1ms ≈ **< 15ms**，对 10s 预算是 3 个数量级的余量。

## 4. 质量实测

### 4.1 零样本分类（53 张 ground truth，11 类，中文 prompt 4 模板集成）

| 档位 | 准确率 | 基线 |
|---|---:|---|
| **fp32 (DML)** | **81.1%**（43/53） | yolov8m 现有管线 76.0% |
| int8 (CPU) | 77.4%（掉点 3.8pp） | — |

易错类：street/text/other（跨类泛化），portrait/food/architecture/animal 全对。

### 4.2 端到端语义搜索（全库 11,372 张真实向量 + 中文查询 top-10 对照 AI 分类）

| 查询 | top-10 分类分布 | 判定 |
|---|---|---|
| 一只猫 | **animal ×10**（含 cat 子类） | ✅ 完美 |
| 小狗 | **animal ×10**（#1 即 dog） | ✅ 完美 |
| 文件文档截图 | **document ×10** | ✅ 完美 |
| 雪山风景 | **landscape ×10** | ✅ 完美 |
| 城市夜景 | night_scene 3 + landscape 3 + cityscape 1 + other 3 | ✅ 合理 |
| 朋友聚餐 | food 5 + portrait 4 | ✅ 合理 |
| 人物自拍特写 | portrait 7 + 未扫描 2 | ✅ 合理 |
| 海边日落 / 樱花盛开 | 标签多 other → **视觉模型抽查** | ✅ 见下 |

**视觉抽查**（Qwen-VL 看图，「海边日落」top3：2 张高度相关（日落+水体），1 张部分相关（城市黄昏，有日落无海）；「樱花盛开」top3 全部命中花卉，含典型重瓣樱花特写）。

关键观察：命中图大多被 yolov8m 管线标为 `other`（日落/樱花不在 ImageNet 1k 词汇）——**语义检索能覆盖标签体系的盲区，这是相对现有 FTS5 标签搜索的核心增量**。另「人物自拍特写」命中了 2 张未跑 AI 扫描的照片：语义索引只依赖缩略图，覆盖面天然大于内容扫描。

## 5. int8 档位决策门

| 证据 | 数值 |
|---|---|
| 零样本分类掉点 | **3.8pp（> 2pp 门槛）** |
| 向量漂移（100 张 fp32 vs int8 余弦） | mean 0.9604 / min 0.8654 |

**判定：int8 不作默认档**（两独立证据一致）。fp16 精度损失 ~1e-3 量级无此问题，默认档 fp16（打包用 Xenova 现成 ONNX fp16 377MB / int8 191MB），int8 仅作可选低配档。

## 6. 与《语义搜索子组件实现方案》逐点对照

| 方案条目 | 实测结论 |
|---|---|
| 选型 Chinese-CLIP ViT-B/16 / 512 维 / 224 输入 | ✅ 成立，质量与速度双达标 |
| Xenova 现成 ONNX（fp16/int8，免 torch） | ✅ 采纳；本次 torch 自导出路径也已验证可行（脚本留存双保险），单文件双塔拆图 spike 照计划做 |
| 缩略图直编码（256→224 resize 可忽略） | ✅ 2.8ms/张，成立 |
| 资源预算「首次全量索引 CPU 1~2h」 | ⚠️ 修正：CPU 兜底 ~1.3h（5 万张）没错，但**默认 DML 仅 ~41min**；建议 DML 优先、CPU 兜底自动回退 |
| 查询链路「文本编码 20~50ms + 余弦 <50ms」 | ⚠️ 实测更优：< 15ms（DML text 6.8ms + 检索 0.36ms@1.1万） |
| 向量 2KB/张，5 万张 +100MB | ✅ 一致 |
| 不引入 sqlite-vec（暴力扫） | ✅ 数据支持：10 万张仍 < 5ms |
| `photo_thumb_cache.thumb_path` 取缩略图 | ✅ 实际布局 `thumbs/` + `thumbs/grid/`（11,240 张），取表路径即可，勿自行拼目录 |
| `model_registry.py` provider 机制继承 | ✅ DML EP 已验证；注意 CPU EP 勿限 2 线程（text 编码 2.5 倍差距） |
| 分词（tokenizers 库 + hiddenimports） | ✅ 已备轻量方案：`minitok.py` 手写 BERT 分词与 transformers 对齐（[CLS] 海 边 日 落 [SEP]），可作 PyInstaller 减包的备选 |

## 7. 建议的默认配置（进入 Phase 1）

- 模型：Chinese-CLIP ViT-B/16 fp16（DML）＋ CPU 兜底；int8 不默认
- 索引：缩略图直编码，batch 8~16 异步流水，500/批事务写库，ScanState 可取消
- 查询：`/embed_text` 单条 DML 编码 → Rust 内存向量（版本号失效）→ 归一化点积 top-K → JOIN `photo_content_scan`
- 检索融合（二期）：语义得分与 FTS5 标签得分加权，弥补 int8/零样本在细粒度词上的边缘误差

> 产物清单：`python/bench/`（5 个脚本 + bench_result.txt + out/embeddings.npy 11,372×512 真实向量），
> `python/models/chinese-clip-vit-b-16/`（fp32×2 + int8 + vocab），`design/clip-model-test-report.md`（本文件）
