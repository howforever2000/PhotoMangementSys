# FEAT-135 · 推理腿可选的 OpenVINO/NPU 执行设备

> **状态**：`done`（2026-11-02）· 默认 **关闭**（`VCR_OV_CHANNELS` 为空 = 行为与今天逐字一致）
> **范围**：只接 `det` / `ocr` / `face_rec` 三条腿；`face_det`（det_10g）永久排除；CLIP 不接。
> **为什么值得重做**：FEAT-133 回退的理由是「快 2.9× 的那条腿只占扫描 0.2%」。本轮换了一个
> 抓手（**DML 加锁次数**而不是单模型延迟），实测全流程 **−58%**。

---

## 一、一句话结论

**NPU 的收益不来自「单模型快多少毫秒」，而来自「从 DML 单队列里拿走多少次加锁」。**

三条证据：

1. DML 持锁占墙钟 **98%**，每张图要排 **116 次**锁（`pipeline_cost_probe` 与本轮探针一致）。
2. `det_10g` 单张只要 16ms，但在 P=6 下**每次持锁实测 68~190ms** —— 固定提交 + 排队成本是
   模型算力的 4~10 倍。
3. 于是收益与「加锁次数」成正比，而不是与「加速比」：只搬 det+face_rec（116→53 次）只有
   **−20%**；再搬 ocr（53→15 次）才到 **−55%**。

这解释了 FEAT-133 为什么失败，也推翻了它留下的两条结论（见第五节）。

---

## 二、实测（口径先说清）

- 机器：Intel Core Ultra 5 245K + Intel AI Boost NPU，OpenVINO 2026.4.1（与原 FEAT-133 同）
- 相册 A（探针）：`D:\YUAN HAO\Pictures\2026\test` 53 张
- 相册 B（真扫描）：album 124，**983 张 / 4.12GB**
- 参数：批 8、`VCR_PHOTO_PARALLEL=6`、`intra_op=8`（仓库最优；落盘的 12 在 P=6 下会把 CPU 基线压到失真）
- 单位：**分摊墙钟 ms/张**（整批墙钟 ÷ 张数）。单张真实延迟约为其 ×P。
- 每配置 3 轮取最好；每轮前 12 张当预热丢弃。

### 2.1 探针（AI 腿，同进程，53 张）

| 配置 | 墙钟 ms/张 | vs cpu | det 加锁 | DML 持锁占墙钟 | NPU 持锁占墙钟 |
|---|---|---|---|---|---|
| cpu（**现状默认**） | 145.2 | +0.0% | — | 0% | 0% |
| dml（开 GPU 开关） | 85.7 | −40.9% | 116 次/批 | **98.0%** | 0% |
| 三通道上 NPU，不开 DML | **43.0** | **−70.4%** | 0 | 0% | 78.4% |
| 三通道上 NPU + face_det 留 DML | 38.4 | −73.6% | 15 次/批 | 45.1% | 86.7% |

> 两次独立运行、两档线程数（8 / 12）都复现：**dml → npu 稳定在 −55% ~ −60%**。
> 绝对值会随机器负载漂（cpu 基线 129.7~199.8），所以决策只看同一轮内的相对值。

### 2.2 真扫描（HTTP 服务 + AI 腿 ∥ 语义腿并发，983 张）

| 配置 | 墙钟 | ms/张 | vs cpu |
|---|---|---|---|
| cpu（现状默认） | **328.2 s** | 336.6 | +0.0% |
| 三通道上 NPU | **136.6 s** | 140.1 | **−58.4%** |

> 保真度：语义腿喂 256px 缩略图（对齐真实扫描的 `photo_thumb_cache`）。
> 第一版直接喂原图，两边都被人为拖慢 —— 这个保真度不补齐，量出来的总时长没有意义。
> 影调 / EXIF 是 Rust 侧且与 AI 腿并发，本特性不改变它们。

### 2.3 数值等价（不透这一关，提速没有意义）

| 项 | 结果 | 门槛 |
|---|---|---|
| det 人框（53 张） | 人框数 / 配对框 **53/53 零差异** | 一致 |
| w600k_r50（38 张真人脸） | cos 最小 **0.99997220**，中位 0.99999749 | ≥ 0.9999 ✅ |
| paddleocr 概率图 | 原始 max\|Δ\| 到 0.82，但二值化掩码 IoU **中位 1.0000** | — |
| paddleocr 文本框 | 47/53 完全一致，**6 张差 1 个框**，前景像素 +0.49% | 需容忍（见残留风险） |
| 端到端 ClassifyResult（53 张） | category/sub_category/label/person_count **53/53 一致**，confidence \|Δ\| ≤ 0.0007 | 5e-3 ✅ |

---

## 三、代码落点（最小改动面）

咽喉点只有一个：所有推理都过 `ModelRegistry` 的 `run()` / `run_clip_*`。
所以服务层（`detector` / `face_service` / `ocr_service` / `pipeline`）**一行都没改**。

| 文件 | 改动 |
|---|---|
| `python/vcr/ov_backend.py` | **新增**：运行时发现（installed / `models/_runtime/ovrt` / `.tmp-ovprobe`）、静态 shape 编译 + `CACHE_DIR`、按设备锁、ORT 鸭子垫片 |
| `python/vcr/config.py` | 新增 `OV_CHANNELS` / `OV_DEVICE` / `OV_SHAPES` / `OV_CACHE_DIR`（默认全关） |
| `python/vcr/model_registry.py` | `_load()` 先试 OV 再落 ORT；`run()` 分流；`_gpu_backed()` 对 OvChannel 返回 False；新增 `ov_info()` |
| `python/server.py` | `/health.ov` 如实上报；`VCR_API_VERSION` 12 → 13 |
| `.gitignore` | `python/models/_ov_cache/` |

### 三条硬约束（都是踩过的，别「优化」掉）

1. **静态 shape 是硬前置条件**。NPU 编译器遇动态维会 `LLVM ERROR` 把整个进程带走，
   Python 侧 catch 不住（FEAT-133 实测触发两次）。`OvChannel` 编译前 reshape，
   并**先断言目标 shape 全为正整型** —— 让将来换模型在 Python 层就炸。
2. **通道形状必须从 `preprocess.py` 取，不能抄外部性能报告**。本仓库 OCR 喂的是
   **640×640**（与 det 共用同一份 letterbox 缩放）。第一版按 960 写死 → 每张抛 shape
   异常 → 被 `ocr_service` 的 `except Exception` 吞成 `OcrResult(error=...)` →
   通道根本没跑，探针却读到「ocr 变快了 18ms」，把 −20% 假报成 −55%。
   → 探针现在带**完整性校验**：每个 OV 通道「实际成功次数」必须等于该 timing 段的调用次数。
3. **NPU 与 DML 必须两把锁**。复用 `_GPU_RUN_LOCK` 会让两个引擎互相串行，
   「iGPU 与 NPU 独立并行」这句话立即失效。

---

## 四、怎么用

```powershell
# 只接 NPU（推荐）：不碰 GPU 开关
set VCR_OV_CHANNELS=det,ocr,face_rec
set VCR_OV_PATH=<openvino 所在目录>   # 未装进解释器时
python/server.py

# 同机对照：NPU vs GPU/CPU
set VCR_OV_DEVICE=GPU
```

`/health.ov` 会如实给出 `channels`（想接哪些）、`active`（实际接上了哪些）、
`devices`、`source`（运行时来自哪）、`error`。**接不上就回落 ORT，且原因写进日志** ——
不允许出现「界面说 NPU 已启用、NPU 占用恒为 0」那种状态。

探针：

```powershell
python/.venv-vcr/Scripts/python.exe python/bench/probe_npu_det_r50_e2e.py <相册> ^
    --rounds 3 --photos-parallel 6 --threads 8 --modes cpu,dml,ov,ov_dml ^
    --ov-channels det,ocr,face_rec
# 只做数值等价
python/.venv-vcr/Scripts/python.exe python/bench/probe_npu_det_r50_e2e.py <相册> --equiv-only
```

---

## 五、它推翻了 FEAT-133 的哪两条结论

| FEAT-133 的结论 | 本轮实测 | 为什么错 |
|---|---|---|
| 「ocr 上 NPU 无收益（33.1 vs 核显 24.4）」 | 640 输入下 ocr 是**分水岭**，不接它只有 −20%，接上才到 −55% | 旧测按 **960** 输入测；本仓库实际喂 640。而且旧测只比较单模型延迟，没看它在 DML 队列里占了多少次加锁 |
| 「clip 是最佳候选（NPU 最优）」 | clip 仍然不接 | 单模型延迟口径没错（18.6 vs 核显 31.7），但语义腿只占扫描 **0.2%~0.3%**，且静态 batch=1 会让 `embed_images` 的整批 `vstack` 前向退化成 N 次单张 |

补充一条旧结论**仍然成立**：`face_det`（det_10g）**永久排除**。
本轮复测 NPU 116.49ms / DML 16.45ms / CPU 37.86ms，与 FEAT-133 的 115.9 完全一致。

---

## 六、残留风险

| # | 风险 | 处置 |
|---|---|---|
| R1 | paddleocr 有 **6/53 张差 1 个文本框**（fp16 固有抖动）。文档判定是「文字框面积 >12% 且 ≥2 框」，压在阈值上的图可能翻类 | 本相册端到端 53/53 一致；上线前在文档类相册上补一次专项等价 |
| R2 | r50 的 cos 0.99997 会让质心极轻微漂移 | 「重建人物库」后人物数不应增加，作为验收项 |
| R3 | NPU 持锁已达 87~89%（接近饱和） | **不要再往 NPU 加第四个通道**；确需再加应先测饱和度 |
| R4 | 未做 UI / 未做 45MB 运行时按需下载（FEAT-129 的机制已回退删除） | 下一阶段；本轮只有 env 开关，AMD/NVIDIA 用户完全不受影响 |
| R5 | 首次编译（缓存未命中）0.1~0.8s/通道 | `models/_ov_cache` 落盘；实测第二次起 0.1s 量级 |

---

## 七、下一步候选（按证据强度）

1. **UI + 按需下载组件**：把 `VCR_OV_CHANNELS` 变成设置页开关 + 探测 Intel 才推荐（复用
   `model_dl` 通道）。⚠️ 恢复 `dc811be` 删除的 ~860 行打包/UI 代码时，注意 FEAT-133 的
   两条 bug（`logutil` 纪律、`clip_ready` 语义）。
2. **OCR 前置门再收紧**：ocr 仍是 38/53 次加锁的贡献者；`OCR_GATE` 目前只在人物规则命中时跳过。
3. **AI 腿改喂缩略图**（仓库既有提案，未做）：decode 在新结构下已是 CPU 侧地板。
