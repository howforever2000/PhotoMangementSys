# Chinese-CLIP 候选模型评估与实测精度对比

> 日期：2026-09-21 · 关联 FEAT-056 / FEAT-055
> 工具：`python/bench/eval_clip_accuracy.py`（同标注集比准确率+一致性+速度，支持 `--resize crop|squash`
> 与 `--max-side` 复现线上缩略图口径）、`python/bench/verify_clip_tiers.py`（数值一致性）、
> `python/bench/export_clip_torch.py`（官方 PyTorch 权重 → 双塔 ONNX 导出 + 数值自校验）
> 标注集：`python/ground_truth.json`（53 张真实照片，11 类，Qwen3-VL 标注）

---

## 1. 实测：零样本分类准确率（53 张 / 11 类）

方法：11 类中文 prompt × 4 模板 → 类心（各自 L2 归一后取均值再归一）；图像 → 图塔 → L2 →
与类心点积 argmax。`crop` = 短边缩放 + 中心裁剪（**线上现用**）；`squash` = 直接缩放成 S×S
（**官方 `preprocessor_config.json` 配方**：`size=模型尺寸 + do_center_crop=false`）；
`缩略图` 口径 = 先按 `thumbnail(256,256)` 语义（保持宽高比、长边=256）缩图，复现线上
「256px 缩略图直编码」链路。

| 变体 | 原图 crop | 原图 squash | 缩略图 crop | 缩略图 squash | 原图图塔(热) | 说明 |
|---|---|---|---|---|---|---|
| **b16-fp16**（线上默认） | 81.1% | **83.0%** | 75.5% | **77.4%** | 214 ms | `chinese-clip/` |
| b16-fp32（Xenova fp32） | 81.1% | 83.0% | 同 | 同 | 187 ms | `chinese-clip-fp32/` |
| b16-torch（PyTorch 导出 fp32） | 81.1% | 83.0% | 同 | 同 | 182 ms | `chinese-clip-vit-b-16/` |
| l14-224（**官方权重自行导出** fp32） | 71.7% | 69.8% | — | 69.8% | **738 ms** | `chinese-clip-l14-224/` |
| l14-336（Xenova fp16） | 69.8% | 67.9% | — | — | **2131 ms** | `chinese-clip-l14/` |

**每类明细（原图 squash；l14 档的崩溃点一目了然）**

| 类别 | b16 | l14-224 | l14-336 |
|---|---|---|---|
| portrait | 8/8 | 8/8 | 8/8 |
| street | 4/7 | **2/7** | **2/7** |
| night_scene | 6/7 | **3/7** | **2/7** |
| plant_flower | 5/6 | 5/6 | 5/6 |
| food | 5/5 | 5/5 | 5/5 |
| architecture | 4/4 | **1/4** | **0/4** |
| vehicle | 4/4 | 4/4 | 4/4 |
| landscape_nature | 3/4 | 3/4 | 3/4 |
| text | 2/3 | 2/3 | 3/3 |
| other | 1/3 | 2/3 | 2/3 |
| animal | 2/2 | 2/2 | 2/2 |

### 结论 1：三个 B/16 变体**完全等价**（回答「vit-b-16 与 fp32 差多少」）
准确率一致（81.1% / 83.0%）、**错判集差异 0 张**、图塔余弦 **min 0.999999 / mean 1.000000**
（文本塔同为 1.000000）。`chinese-clip-vit-b-16`（自行 `torch.onnx.export`）与
`chinese-clip-fp32`（Xenova 转换）是**同一权重的两条转换管线**，**无精度差异**；
选谁只影响速度 / 体积 / 能否走 GPU。→ 用户决定**保留 fp32 档**（DML 实测数值一致且快 2×）。

### 结论 2：**squash vs crop 的差异不显著 → D1 暂缓（更正）**
一度测出 squash 稳定 +1.9pp（原图 81.1→83.0、缩略图 75.5→77.4），但进一步的检验推翻了它：

| 输入（忠实复现线上：WebP q85 + Triangle 滤镜，长边 256/320/384） | crop（现用） | squash（官方） |
|---|---|---|
| 256px WebP | **79.2%** | 77.4% |
| 320px WebP | **83.0%** | 75.5% |
| 384px WebP | **79.2%** | 77.4% |

方向**翻转**且**非单调**（320 > 384）—— 典型的噪声特征。配对检验（McNemar，原图口径）：

```
crop 错 10 张 / squash 错 9 张；b=2, c=1 → p = 1.000（不显著）
单组准确率标准误 ≈ 5.5pp  ⇒ ±1.9pp（=1 张图）完全在噪声内
要在 95% 置信下分辨 2pp（配对、不一致率 15%）需 ~500~1000 张标注
```

**结论**：在现有 53 张标注集上，**squash/crop 的优劣不可判定**；因此**不做 D1**
（不值得为 1 张图的差异让 10,919 条索引失效 + 重扫 15 min）。保持现状 crop。
若将来要把 recipe 改成官方 squash，**先扩标注集**（见 §1.5）再定。

> 踩坑记录（两次）：① 我一度用「短边=256」模拟缩略图，得出"两种 recipe 无差异"；
> 核对 `thumbnail.rs` 的 `DynamicImage::thumbnail(256,256)`（**保持宽高比、长边=256**）后重测，
> 结果又变成"有差异"；② 再按线上真实编码（**WebP q85**，非 JPEG）复测，差异又翻转且非单调。
> **教训：模拟线上链路必须核对真实几何与编码格式；且 n=53 的评测集只能筛掉「明显更差」的选项，
> 无法分辨 1~2pp 级别的改进。**

### 结论 3：预处理耗时占比极小 —— 「换成更快的前处理」没有意义
实测（真实 256px 缩略图 vs 原图，本机）：

| 输入 | 解码+预处理 | 相对模型前向 91 ms |
|---|---|---|
| 256px 缩略图 → 224（squash） | **2.84 ms** | **3%** |
| 256px 缩略图 → 224（crop，现用） | 2.09 ms | 2% |
| 3712×5568 原图 → 224 | 171 ms | 188% |

- squash 比 crop **慢 0.75 ms**（要重采样整幅），差别 <1%，用户无感 → **选 recipe 不该看速度**。
- 「缩略图直编码」这个既有决策的价值被精确量化：**2.8 ms vs 171 ms，1 万张省 28 min**。
- 「顺带生成 CLIP 专用缩略图 / 预处理直接从缩略图一步到位」的收益上限 ≈ **2 ms/张（≈20 秒/万张）**，
  除非为了精度改用更大尺寸 —— 但尺寸 sweep 同样不可判定（320→384 反而下降），故**不值得做**。

### 结论 4：**L/14 家族全面劣于 B/16**（唯一有结论的"大模型"问题）
两个**独立** L/14 变体（不同转换管线 + 不同分辨率）：
- l14-224：官方 `OFA-Sys` 权重自行导出，torch vs ORT 余弦 **1.000000**（数值可信）→ **69.8% / 71.7%**
- l14-336：Xenova 现成 ONNX → 67.9% / 69.8%

两者都显著低于 B/16（83.0%），且**崩的正是场景类**（architecture 0~1/4、night 2~3/7、
street 2/7），物体类（food/vehicle/animal/flower）与 B/16 持平；速度还慢 3.5~10×。
→ **不是 Xenova 转换问题**；13pp 差距 + 逐类一致的系统性崩塌，超出了 5.5pp 标准误的解释范围。

### 1.5 评测集需要扩容（否则无法做 ±2pp 级决策）
- 现状：53 张（`python/ground_truth.json`，独立测试目录，非相册库内照片）。
- 能力边界：**只能筛掉"明显更差"**（如 L/14 的 13pp）；对 recipe / 缩略图尺寸 / 转换管线
  这类 1~2pp 的差异**无分辨力**。
- 扩容方案（按成本递增）：① 用 Qwen3-VL 对库内抽样 **200~300 张**自动打 11 类标签
  （分批调用，约 20~30 次），人工抽检 20 张校正；② 人工标注 100~200 张（最可靠）；
  ③ 长期：在真实使用中收集"用户纠正过的分类结果"作为弱标签。

---

## 2. 候选模型家族分析（核显本定位）

速度基准：B/16 缩略图**实测 91 ms/张**（FEAT-055，10k ≈ 15 min）；其余按视觉侧 MACs 相对折算
（ViT-B/16@224 17.6G / RN50 4.1G / ViT-L/14@224 80.7G / L/14@336 180G / ViT-H/14 167G），
并与实测交叉验证（L/14@224 估算 417 ms vs 实测 738 ms 原图 / 964 ms 缩略图；同量级）。

| 模型 | 参数量 | 视觉骨架 | 文本骨架 | 分辨率 | 现成 ONNX | 缩略图/张 | 10k 索引 | fp16 体积 | 实测精度 | 结论 |
|---|---|---|---|---|---|---|---|---|---|---|
| RN50 | 77M | ResNet50 (38M) | RBT3 (39M) | 224 | ✗ 需导出 | 估算 ~21 ms | ~4 min | ~160 MB | 未测（论文最低） | ❌ 不加：B/16 已够快，纯降精度 |
| **B/16** | 188M | ViT-B/16 (86M) | RoBERTa-Base (102M) | 224 | ✓ 在用 | **实测 91 ms** | **实测 ~15 min** | 377 MB | **81.1% / 83.0%** | ✅ **保持默认**（改 squash 再 +1.9pp） |
| L/14 | 406M | ViT-L/14 (304M) | RoBERTa-Base | 224 | ✗ 需导出 | 实测 ~417 ms | ~70 min | ~800 MB | **69.8% / 71.7%（更差）** | ❌ **不加**（已实测排除） |
| L/14@336 | 407M | ViT-L/14 | RoBERTa-Base | 336 | ✓ 已下 | 实测 1.4~2.1 s | ~3~4 h | 1.63 GB | **67.9% / 69.8%（更差）** | ❌ **不加** |
| H/14 | 958M | ViT-H/14 (632M) | RoBERTa-**L** (326M) | 224 | ✗ 需导出 | 估算 ~863 ms | ~2.4 h | ~1.9 GB | 未测 | ❌ 不加：与"核显本"定位冲突 |

补充说明：
- **文本塔不同 = 不同向量空间**：RN50 用 RBT3、H/14 用 RoBERTa-Large，与 B/16 的 RoBERTa-Base
  都不通用 → 换档必须重建索引（架构按 `photo_embeddings.model` 隔离，机制现成）。
- **L/14 的文本塔与 B/16 相同**（都是 RoBERTa-wwm-Base），但投影维度 768 ≠ 512，仍不通用。
- 现成 ONNX 只有 B/16（Xenova）与 L/14@336（Xenova）；其余需自行导出 —— 用
  `export_clip_torch.py` 一条命令即可（本次 L/14@224 就是这样做出来的）。

---

## 3. 「新档位准入」流程（建议固化为验收门）

任何新档位合入前按顺序过四道门，缺一不可：

1. **可下载 / 可导出**：ONNX 就位（或官方权重可导出），落位符合
   `<root>/onnx/<onnx>` + `<root>/clip_vision.onnx` + `<root>/clip_text.onnx` + `tokenizer.json`
   （附带文件必须落模型根目录，见 BUG-2026-0920-002；下载 URL 不得带本地目录名，见 BUG-2026-0921-001）。
2. **数值自洽**：`verify_clip_tiers.py <新档> <新档> --provider-a DmlExecutionProvider --provider-b CPUExecutionProvider`
   余弦 **> 0.999**（否则开 GPU 会静默错）。自行导出的还必须过 `export_clip_torch.py` 内置的
   torch-vs-ORT 校验（>0.999）。
3. **精度准入**：`eval_clip_accuracy.py --max-side 256 --resize squash --only <新档>` 在 53 张标注集上
   **≥ B/16「缩略图 squash」基线 77.4% + 5pp** 才值得占用体积与重扫成本（并写入本文档）。
   ⚠️ 用**线上口径**（缩略图 + 官方 recipe）判定，别用原图/crop 口径自欺。
4. **速度准入**：10k 缩略图索引 ≤ 60 min（核显本目标）；超过则标"仅强机"或不上架。

---

## 4. 复现

```bash
# 官方 recipe + 线上口径（判定用的权威口径）
python python/bench/eval_clip_accuracy.py --max-side 256 --resize squash
# 原图口径 / crop 口径对照
python python/bench/eval_clip_accuracy.py --resize squash
python python/bench/eval_clip_accuracy.py --resize crop
# 档位/provider 数值一致性（换档或开 GPU 前必跑）
python python/bench/verify_clip_tiers.py b16-fp32 b16-fp32 \
  --provider-a DmlExecutionProvider --provider-b CPUExecutionProvider
# 官方 PyTorch 权重 → 双塔 ONNX（含 torch-vs-ORT 数值校验）
python python/bench/export_clip_torch.py \
  --repo OFA-Sys/chinese-clip-vit-large-patch14 --out python/models/chinese-clip-l14-224 --size 224
```
结果 JSON：`python/bench/out/clip_accuracy_squash.json` / `clip_accuracy_crop.json`。

---

## 5. 待决事项

- [x] ~~L/14 是否值得加入~~ → **已实测否决**（官方导出的 224 与 Xenova 的 336 都 ~68-72%，
      低于 B/16 的 83.0%，且慢 3.5~10×）。H/14 明确不上架，RN50 不值得。
- [x] ~~是否把线上预处理改为官方 squash（+1.9pp）~~ → **撤销**：配对检验 p=1.000，忠实 WebP
      复测方向翻转且非单调 ⇒ 差异不显著，不值得付"索引失效 + 重扫 15 min"的代价。保持 crop。
- [ ] **评测集扩容到 200~300 张**（Qwen3-VL 自动标注 + 人工抽检）——这是解锁 recipe /
      缩略图尺寸 / 转换管线等 ±2pp 级决策的前置条件（见 §1.5）。
- [ ] 缩略图尺寸（256 → 320/384）与「原图编码」选项：等评测集扩容后再评估（现无分辨力）。
- [x] ~~实验目录占用~~ → **已执行（2026-09-21）**：删除 `chinese-clip-l14/`(1.63GB) +
      `chinese-clip-l14-224/`(1.63GB)，回收 **3.26GB**；同时把 `l14` 从 `CLIP_MODELS` 与
      `model_dl.rs` 下载表中**下架**（否则界面还会给一个"下载已被证明更差模型"的按钮）。
      单测改为「由 Python 档位表派生档位数」，不再硬编码 3 档。
      `chinese-clip-vit-b-16/`（1.5GB）与 **fp32 档**按用户要求**保留**。
      需要重做 L/14 实验时按 §4 两条命令重下/重导（约 20 min）。
