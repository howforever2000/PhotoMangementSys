# FEAT-133 · 语义索引走 NPU（OpenVINO 裁剪运行时）——**已回退**

> **状态**：`reverted`（2026-11-01）—— 功能已整体移除，代码回到纯 CPU + DirectML。
> **回退提交**：见 `git log --grep="回退 OpenVINO"`。
> **本文档保留原因**：它是「为什么这条路不值得走」的实测记录，避免以后重做一遍。

## 为什么回退（一句话）

**NPU 本身没问题，但它能加速的那条腿在扫描里只占 0.2%。**

四条独立判据都证明 NPU 确实在算、且确实快（见下方第五节），但扫描耗时结构是：

| 阶段 | 耗时（album=126 / 776 张） | 占比 | NPU 能帮上吗 |
|---|---|---|---|
| AI 识别（det/人脸/OCR） | 99049 ms | **79%** | ❌ 这四条腿 NPU 全输（face_det 慢 8 倍） |
| 影调 | 26539 ms | 21% | ❌ 纯 numpy |
| 语义向量 | 224 ms | **0.2%** | ✅ 但正是它 |

而且**三腿并发**（`content.rs:1843` 影调 ∥ 语义向量 ∥ AI 识别），语义腿即使快 3×，
776×33ms ≈ 26s 也完全被 99s 的 AI 腿掩盖 —— **扫描总时长一分不变**。

用户侧的真实反馈也是这个：**「NPU 显示加速但无任何占用」、「没有效果」**——
不是因为 NPU 不行，而是因为被加速的对象不在关键路径上。

## 保留它的直接代价

- 用户多一份 40MB 运行时组件（下载 15.5MB）+ 一套后端分支
- 两条 bug（BUG-2026-1101-001/002）都出在这条分支上，而收益用户感知不到
- UI 上多一个开关，反而加重了「性能设置看不懂」的问题

## 回退后保留的东西

- `python/vcr/logutil.py`：**留着**。它修的是 `_create_session`/`_load` 里同类的
  `print(..., file=sys.stderr)` 隐患（宿主管道关闭时抛 OSError 22，会把「GPU 失败回退 CPU」
  这一步炸掉——那是主链路）。
- 性能设置顶部的「这台机器现在怎么跑」摘要：**留着**，但语义那一行改成按档位说明
  （fp16 档固定 CPU、fp32 档跟随 GPU 开关），不再依赖已删除的 /clip_runtime。
- `python/bench/probe_npu_*.py`：四个纯证据探针（不依赖被删模块），留着作为「以后想重做时
  先看这里」的入口。

---

## 以下为回退前的设计与实测记录（存档）

## 一、为什么只动一条腿

本机通道级实测（Intel Core Ultra 5 245K + Intel AI Boost NPU，OpenVINO 2026.4.1，单流定长）：

| 通道 | 输入 | NPU | 核显 | CPU | 结论 |
|---|---|---|---|---|---|
| `clip_vision` | 1×3×224×224 | **19.7 ms** | 26.1 ms | 56.7 ms | ✅ NPU 最优 |
| `det`（yolov8n） | 1×3×640×640 | 12.3 ms | **7.0 ms** | 15.2 ms | ✗ 核显最优 |
| `face_det`（det_10g） | 1×3×640×640 | **115.9 ms** | **14.0 ms** | 36.1 ms | ✗ NPU 慢 8 倍，**永久排除** |
| `face_rec`（w600k_r50） | 1×3×112×112 | 8.2 ms | **6.5 ms** | 22.1 ms | ✗ 核显最优 |
| `ocr`（paddleocr） | 1×3×960×960 | 33.1 ms | 24.4 ms | **25.8 ms** | ✗ NPU 输 |

`clip_text` **也不接管**：它的 `attention_mask` 带无界动态维，NPU 编译器会
`LLVM ERROR: Failed to infer result type(s)` 把**整个进程**带走（实测触发两次，Python 侧无法 catch）。
文本塔本来也不是热点，留在 ONNX Runtime。

---

## 二、组件为什么能这么小

完整 `openvino` 装后 244.8MB，逐项拆解：

| 文件 | 体积 | 处置 |
|---|---|---|
| `openvino_intel_npu_compiler.dll` | 95.7 MB | **删**（只负责 ONNX→NPU blob，我们启动时现编且有缓存） |
| `openvino_intel_cpu_plugin.dll` | 44.2 MB | **删**（OV 不接管 CPU 路径） |
| `openvino_intel_gpu_plugin.dll` | 33.1 MB | **删**（Intel 上 NPU 才是收益点，iGPU 交回 DirectML） |
| `openvino.dll` | 15.7 MB | 保留 |
| `openvino_intel_npu_plugin.dll` | 7.3 MB | 保留 |
| `openvino_onnx_frontend.dll` | 5.2 MB | **保留**（要读 `.onnx`；删了会报 `Available frontends: ir jax tflite`） |
| 其余前端 / `cache.json` / `.lib` / `.pdb` | ~48 MB | 删 |

→ **45.4MB 装上 / 15.5MB zip**，`devices = ['NPU']`，`cos(完整版, 裁剪版) = 1.00000000`。

> ⚠️ 一个踩过的坑：最初连 `openvino_onnx_frontend.dll` 一起删了，裁剪版只能 `import_model(blob)`、
> 不能 `read_model(.onnx)`，运行时报「Please check that model format .onnx is supported」。
> 用 blob 的路线才不需要 onnx 前端；用现编路线必须保留。

---

## 三、交付链路

```
GitHub Release 资产 openvino-npu-win-x64.zip  (15.5MB)
        │  model_dl：DlSpec "openvino-runtime"，abs_url 走 releases/latest/download/
        ▼
python/models/_runtime/openvino-npu-win-x64.zip
        │  vcr/ovrt.py：首次使用时标准库 zipfile 解压（先落 .partial 再改名）
        ▼
python/models/_runtime/ovrt/openvino/…        → sys.path 注入 → import openvino
```

- **Rust 不引 zip 依赖**：下载单元仍是「文件」，解压交给 Python。
- **解压不迷信改名**：Windows 上杀毒软件会在刚写入 DLL 后占句柄，`os.rename` 实测抛
  `WinError 5 拒绝访问`。现在的策略是「改名失败不算失败」——直接用 `.partial` 目录跑，
  下次启动再试改名，避免用户每次都要重下 15MB。
- **不进 MSI**：`release.tauri.conf.json` 的 resources 白名单不动；只有探测到 Intel 才在
  性能设置里推荐下载。`release.ps1` 新增 2b 步负责构建产物 + 提醒上传资产。

---

## 四、后端选择与退路

持久化在 `models/current_clip_runtime.json`：

| 值 | 含义 |
|---|---|
| `auto`（默认） | 组件已装 + 有可用设备 + 模型文件在 → 挂 OpenVINO；否则一切照旧 |
| `ort` | 强制回 ONNX Runtime（**不需要卸载组件**的退路） |

接口：`GET /clip_runtime`、`POST /clip_runtime {mode}`；Tauri：`get_vcr_clip_runtime`、`set_vcr_clip_runtime`。
切换只清空 `clip_vision` 槽位并后台重建，文本塔与其他通道完全不受影响。

**红线**：后端切换**不改变** `photo_embeddings.model`（那是模型档位 id，不是后端）。
改它就会踩 `content.rs:1125` 那个「向量在库里却一条都搜不出来」的老坑。

---

## 五、验收数据

| 项 | 结果 |
|---|---|
| 数值等价（端到端，ORT-CPU vs OV-NPU） | `cos = 0.99999779`，`max|Δ| = 0.00055` |
| 8 张一批延迟 | ORT CPU **50.8 ms/张** → OV NPU **19.3 ms/张**（2.63×） |
| 单张散跑 | NPU 约 29 ms（含约 10ms 固定提交开销）→ 文案按「一批」口径，不承诺单张倍数 |
| 组件缺失 | `/health` → `backend=ort, installed=false`，`embed_batch` 正常 |
| 非 Intel | `recommended=false`，UI 显示「不适用」且不给下载按钮 |
| 来回切换 | `auto → ort → auto` 后端与 provider 均正确翻转 |
| 静态检查 | `cargo check --all-targets` 0 error；`vue-tsc --noEmit` 0 error |

复跑：`python/bench/verify_ov_clip_equiv.py`（任意机器，会自报门槛与结论）。

---

## 六、性能设置菜单改了什么

1. **顶部「这台机器现在怎么跑」**：语义索引 / 人物与文字 两行，各显示实际设备。
   把「四个开关分属两条腿」这件事实摆到最前，开关从猜谜变成解释。
2. **新增「语义索引加速」分区**：
   - Intel + 未装 → 显示体积与收益 + 下载按钮（复用 `model_dl` 进度通道）
   - Intel + 已装 → 显示实际设备、首次编译耗时、一键切回 ORT
   - AMD / 未知 → 明确写「不适用」并说明原因，**不给按钮**
3. **运行时组件从「模型下载」列表里过滤掉**：它只在性能设置里管理，避免两处入口各说一套状态。
4. **GPU 加速文案改准**：本开关只管人物检测 / 人脸 / OCR；语义索引是另一条腿。

---

## 七、已知边界（不在本次范围）

- **AMD/NVIDIA NPU**：OpenVINO 只认 Intel；AMD XDNA 需要 Vitis AI EP（生态待验证），未做。
- **OV 接管 iGPU**：GPU 插件已被裁掉。若将来要让 OV 作为 iGPU 降级候选，把
  `openvino_intel_gpu_plugin.dll` 从 DROP 列表移出即可（+33.1MB）。
- **离线 blob**：可省掉编译器但 blob 绑编译器/驱动版本，维护成本大于收益，不做。
- **首启编译**：NPU 首次编译 fp32 档 6.3s、fp16 档 0.7s（有缓存后 1s 内），期间 `clip_ready=false`。
