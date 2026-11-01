# FEAT-136 · 加速分配表（GPU 与 NPU 各有加速对象）+ OpenVINO 可选组件

> **状态**：`done`（2026-11-02）· 默认**不启用**（`current_accel.json` 不存在 = 旧的全局 GPU 开关语义）
> **前置**：`docs/FEAT-135-推理腿NPU执行设备.md`（为什么 NPU 值得接、实测口径）
> **一句话**：GPU（DirectML）与 NPU（OpenVINO）是**两个独立引擎**，一个全局开关表达不了
> 「谁该跑哪条腿」，所以换成一`张「通道 → 设备」表；运行时做成**可选组件**，不装零影响。

---

## 一、为什么要表

FEAT-135 已经证明：收益 ≈ 从 DML 单队列里拿走多少次加锁，而不是单模型快多少毫秒。
但那轮只有「三条腿全 NPU」一种形态，剩下的 `face_det`（det_10g）落到哪、能不能顺手用上
核显，没有表达方式 —— 旧开关只能「全给 GPU」或「全不开」。

各通道实测最优设备（本机 Intel Ultra 5 245K + AI Boost NPU）：

| 通道 | 触发频率 | 最优设备 | 依据 |
|---|---|---|---|
| `det`（yolov8n） | 每张 1 次 | **NPU** | 单次 14.7ms；且是 AI 腿最大的 CPU 消费者（P=6 争用后 75.8ms/张） |
| `ocr`（paddleocr） | 约 72% 的图 | **NPU** | 640 输入下是分水岭（不搬它只有 −20%） |
| `face_rec`（w600k_r50） | 有脸时每脸 1 次 | **NPU** | 唯一「NPU 赢核显」的通道（5.99 vs 8.44） |
| `face_det`（det_10g） | 命中人物规则时 | **GPU / CPU** | NPU 116.5ms（慢 7 倍，永久排除）；DML 单次 92.9 vs CPU 120.9 |
| `clip_vision/text` | 一次扫描几次 | **CPU** | fp16 档在 DML 上有数值 bug（BUG-2026-0910-006）；且只占扫描 0.2% |

→ 默认分配（服务端推荐）：`det/ocr/face_rec → NPU`，`face_det → GPU`，
两者**同时**有活干且不抢同一个队列。

## 二、实测（300 张，真 HTTP 服务，AI 腿 ∥ 语义腿并发）

| 配置 | 墙钟 | ms/张 | vs 现状 |
|---|---|---|---|
| 现状（全 CPU） | 87.7 s | 300.3 | — |
| 加速分配（NPU×3 + GPU×1） | **25.8 s** | **88.4** | **−70.6%** |

会话表实测（`/health.gpu.sessions`）：
```
det=['OpenVINO:NPU'] ocr=['OpenVINO:NPU'] face_rec=['OpenVINO:NPU']
face_det=['DmlExecutionProvider','CPUExecutionProvider']
```

同机另一组（983 张全量、走环境变量形态）**336.6 → 140.1 ms/张 = −58.4%**。
两组绝对值不同是相册画像差异（人脸密度 / OCR 触发率），方向一致。

探针四配置对比（53 张，P=6，intra 8，摊薄墙钟 ms/张）：

| cpu | 全 GPU | NPU×3 + CPU 人脸检测 | **NPU×3 + GPU 人脸检测** |
|---|---|---|---|
| 136.2 | 88.9（−34.7%） | 38.6 | **39.0**（−71.4%） |

> GPU 拿 `face_det` 的墙钟增量在噪声内（39.0 vs 38.6），但 `face.det` 单次
> 92.9ms（GPU）vs 120.9ms（CPU）是实打实的 —— 它只跑 15/53 次，所以摊薄后不显。
> **不做**「GPU 也跑 clip」：语义腿占 0.2%，那是 FEAT-133 栽过的同一个坑。

## 三、可选组件（不装零影响）

```
GitHub Release 资产 openvino-npu-win-x64.zip (15.5MB)
        │ model_dl：DlSpec "openvino-runtime"，abs_url 走 releases/latest/download/
        ▼
python/models/_runtime/openvino-npu-win-x64.zip
        │ ov_backend.ensure_runtime_extracted()：首次使用时标准库 zipfile 解压
        ▼
python/models/_runtime/ovrt/openvino/…  → sys.path 注入 → import openvino
```

- **裁剪**：完整 244.8MB → 45.4MB 装上 / **15.5MB zip**（删编译器 95.7MB + CPU/GPU 插件
  77.3MB + 多余前端）。实测裁剪版仍能 `compile_model(onnx, "NPU")`，且与完整版/CPU 等价：
  det 53/53 人框零差异、r50 38 张脸 cos 最小 0.99997220。
- **不进 MSI**：`release.tauri.conf.json` 白名单不动；只有探测到 Intel 才在设置页推荐下载。
- **Rust 不引 zip 依赖**：下载单元仍是「文件」，解压交给 Python。
- **解压不迷信改名**：Windows 上杀毒会在刚写完 DLL 时占句柄（`os.rename` 抛 WinError 5）→
  改名失败不算失败，直接用 `.partial` 目录跑，下次启动再试。

## 四、代码落点

| 文件 | 改动 |
|---|---|
| `python/vcr/config.py` | `CHANNEL_KEYS` / `accel_setting()` / `set_accel_setting()`，持久化 `models/current_accel.json` |
| `python/vcr/model_registry.py` | `_device_of()` / `_ort_providers_for()` / `default_channels()` / `accel_info()` / `set_accel()`；`_load` 按通道分派 |
| `python/vcr/ov_backend.py` | 组件 zip 解压（`ensure_runtime_extracted`）+ 运行时发现 |
| `python/server.py` | `GET/POST /accel`；`/health.accel`；API 12 → **13** |
| `python/scripts/build_ov_runtime_zip.py` | 裁剪 + 自检 + 打包（复用 FEAT-133 的脚本） |
| `src-tauri/src/model_dl.rs` | `openvino-runtime` 规格 + 三处同名一致性单测 |
| `src-tauri/src/vision.rs` / `vcr_settings.rs` / `lib.rs` | `vcr_accel` / `vcr_set_accel` / 命令注册；404（旧服务）→ 重启重试 |
| `src/components/settings/AccelSection.vue` | 新分区：分配表（请求 vs 实际）+ 组件下载 + 启用/关闭 |
| `src/components/settings/ModelDownloadRows.vue` | 把运行时组件从「模型下载」列表里过滤掉（一处入口一个说法） |
| `release.ps1` | 2b 步：缺产物就现跑裁剪脚本 + 提醒上传资产 |

## 五、验收（都真跑过）

| 项 | 结果 |
|---|---|
| `cargo check --all-targets` | 0 error（仅既有 warning） |
| `cargo test --lib openvino_runtime` | 1 passed（DlFile.local / ZIP_NAME / abs_url 三处同名） |
| `vue-tsc --noEmit` | 0 error |
| 产品路径烟测（零环境变量） | 默认未启用 → 启用后 `effective = NPU×3 + GPU×1`（等 1~2s）→ 关闭后回 CPU，组件仍在 |
| 组件等价 | 裁剪运行时 det 53/53 零差异、r50 cos 0.99997220 |
| 真扫描 | 300 张 −70.6%；983 张 −58.4% |

## 六、附：CLIP fp32 能不能上 GPU（2026-11-02 补测）

**能。但不建议默认开。**

代码路径：`_clip_providers()` 里 fp16 档**硬钉 CPU**（DML 上有算子级数值 bug，
BUG-2026-0910-006，不能动）；**fp32 档跟随全局 provider**。而本特性的
`set_accel(True)` 会把 `_providers_sel` 复位成自动探测 → fp32 档的 CLIP 自动落到
DirectML。实测会话表：

```
b16（fp16 档）  clip_vision=['CPUExecutionProvider']
b16-fp32 档    clip_vision=['DmlExecutionProvider','CPUExecutionProvider']
```

实测（120 张全新建档，AI 腿 ∥ 语义腿并发；两轮）：

| 档位 | clip 设备 | 语义腿单独 | AI 腿 | 语义腿 | 合计（并发） |
|---|---|---|---|---|---|
| b16 (fp16) | CPU | 85.5 / 121.8 ms/张 | **8.3 / 9.0 s** | 14.9 / 18.1 s | 124.0 / 150.7 ms/张 |
| b16-fp32 | **DML** | **75.7 / 79.8 ms/张** | 11.6 / 12.1 s | 11.6 / 12.0 s | **96.7 / 100.6 ms/张** |

两个现象同时成立：

1. fp32 + DML **确实更快**（语义腿 −12% ~ −35%）。
2. 但它把 **AI 腿拖慢 35~40%** —— CLIP 是「整批 8 张一次前向」，一次持锁数百毫秒，
   会和 `face_det` 抢全局 DML 锁。这正是 FEAT-080 那条「GPU 队列是稀缺资源」，
   也解释了 FEAT-133 当时 166.5 vs 141.8 的读数。
3. 净效果取决于**新照片占比**：本测试 120 张全是新的，此时语义腿才是关键路径 →
   净赚 22~33%；而真实重扫里语义腿只处理新照片（album 126 实测 224ms/776 张 = 0.2%），
   语义腿不是关键路径，AI 腿的 +35% 就是**净亏**。

叠加两项固定代价：fp32 模型 719MB（fp16 是 360MB），且**换档必须重建语义索引**
（两个档的向量空间不兼容）。为了 0.2% 的腿付这个代价不划算 ——
所以默认保持 fp16/CPU；fp32 档仍可通过旧的「GPU 加速」开关手动上 GPU（能力没丢）。

> 本轮同时修掉一个 bug：`set_accel(False)` 之前只重置了分配表，没有恢复启用前的
> 全局 provider 状态，导致 fp32 档 CLIP 在「关闭」后仍留在 DML。现在启用时快照、
> 关闭时原样恢复（实测关闭后 `clip_vision=['CPUExecutionProvider']`）。

## 七、残留风险

| # | 风险 | 处置 |
|---|---|---|
| R1 | `face_det` 放 GPU 的收益很小（单次快 23%，摊薄后噪声内） | 保留：GPU 因此才有加速对象；AMD 机器上它就是主力（全 GPU 形态 −34.7%） |
| R2 | paddleocr fp16 抖动：6/53 张差 1 个文本框 | 文档判定的面积阈值 12% 是唯一敏感点，上线前补文档类相册专项等价 |
| R3 | NPU 持锁 87~89%（接近饱和） | 别再往 NPU 加通道；要加先测饱和度 |
| R4 | 组件 zip 需随 Release 上传，缺资产时下载 404 | 静默留在 ONNX Runtime（不崩），`release.ps1` 已提示 |
| R5 | 分配表是**持久化设置**，换机器后可能指向不存在的设备 | 每通道独立回落：分配 GPU 但无 DML → CPU；分配 NPU 但无组件 → ORT |
