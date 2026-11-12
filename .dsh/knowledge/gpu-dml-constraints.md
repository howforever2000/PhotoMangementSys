# GPU / DirectML / NPU 约束

> 本项目在 AMD DirectML / Intel NPU 上踩过的硬约束。接 GPU/NPU 加速前必读。来源均为 `bug-notes.json`。

## 接新模型 GPU 加速前先做 CPU/GPU 数值一致性对比

- **触发条件**：把模型（尤其 fp16）送上第三方执行提供方（DirectML / NPU / OpenVINO）。
- **先做**：同一输入 CPU/GPU 两路 embedding 余弦 ≥0.999 才算过；自检脚本要覆盖**真实加载路径**（`model_registry` 的 SessionOptions），优化级别多档都测（ALL / BASIC / DISABLE）。
- **为什么**：AMD DirectML 对 Xenova Chinese-CLIP fp16 图算子有数值 bug：ALL 优化 cos=1.0000 但 vision 塔会话创建崩溃，BASIC/DISABLE cos=0.4313 —— 能跑通 ≠ 算得对。
- **违规症状**：embedding 余弦仅 0.43；语义搜索 / 分类结果全部失真；vision 塔初始化崩溃。
- **来源**：`bug-notes.json` BUG-2026-0910-006、BUG-2026-0920-003（`python/vcr/model_registry.py`）。
- **记录日期**：2026-11-12

## 「固定 CPU」要写进代码路径，不能只写在注释

- **触发条件**：某通道不能上 GPU（如 fp16 CLIP 双塔），但 provider 由全局开关决定。
- **先做**：显式按档位选 provider —— 新增 `_clip_providers()`（fp16 档 → `['CPUExecutionProvider']`；fp32 档 → 跟随 GPU 开关），`_create_session` 增加 `providers` 参数，`clip_vision/clip_text` 建会话时显式传入。
- **为什么**：注释宣称「固定 CPU」，但代码仍用全局 `_providers()`，打开「GPU 加速」开关即把 fp16 语义双塔送 DirectML，静默产出错误向量。
- **违规症状**：开 GPU 后向量静默失真（潜在缺陷）；文档与实现脱节。
- **来源**：`bug-notes.json` BUG-2026-0920-003（`python/vcr/model_registry.py`）。
- **记录日期**：2026-11-12

## 固定依赖版本，别浮动 onnxruntime

- **触发条件**：requirements 写 `onnxruntime>=1.17` 让 pip 解析最新版。
- **先做**：pin 具体版本（`onnxruntime==1.24.4`），并在注释写明「浮动前必须用评测脚本重测」及基准数据。
- **为什么**：pip 装到 1.29.0，同机同模型 CLIP fp16 图塔 CPU 推理比 1.24.4 慢约 2.9×（实测 158.7 → 460.0ms/张），属框架层性能回归。
- **违规症状**：打包版比开发版慢 2.9 倍。
- **来源**：`bug-notes.json` BUG-2026-0921-002（`python/requirements-vcr-packaging.txt`）。
- **记录日期**：2026-11-12

## DML 串行锁是设备级硬约束，不能拆成「每会话一把」

- **触发条件**：想按会话拆分 GPU 锁来提并发。
- **先做**：保持全局单消费者队列；想并行只能换 EP（CUDA / TensorRT / OpenVINO）或减少加锁次数（如 FEAT-127 OCR 前置门）。另：`det` 与 `face_det` 的 ONNX 是静态 `batch=1`（declared `[1,3,640,640]`，喂 batch=2 直接 `INVALID_ARGUMENT`），不能合批；`ocr` 与 `face_rec` 是动态 batch 可以合。
- **为什么**：DirectML 非线程安全不是「同一会话内」而是「同一 D3D12 设备上下文」层面，多个 DML 会话并发 `run` 会踩同一资源。
- **违规症状**：随机 `RemoteDisconnected` / 进程消失 / `ACCESS_VIOLATION`。
- **来源**：`bug-notes.json` BUG-2026-1027-008（wontfix，硬约束）。
- **记录日期**：2026-11-12

## 子进程 stderr 写失败不得把成功状态改写成失败

- **触发条件**：服务对 `Stdio::piped()` 子进程 `print(..., file=sys.stderr)`，管道在服务重启 / 宿主回收时关闭。
- **先做**：日志统一走 `vcr/logutil.py` 的 `err()/out()/exc()`（包住写 stream 的异常，永不抛）；状态提交顺序改为「先构造 → 再提交状态 → 最后打日志」；`clip_runtime_status().active` 判据改为同时要求 `_ov_clip.ready` **且** `_ready['clip_vision']`，两个事实源不许分叉。
- **为什么**：管道关闭后写 stderr 抛 `OSError: [Errno 22] Invalid argument`，若写在「状态已提交」之后被外层 `except` 接住，会把成功改写成 failure，而 `_ov_clip` 不回滚 → `active=true` 而 `is_ready=false`。
- **违规症状**：显示「NPU 加速中」但 NPU 占用恒为 0；两个事实来源分叉；每张图重编 NPU（0.7~6.3s/次）。
- **来源**：`bug-notes.json` BUG-2026-1101-001（`python/vcr/model_registry.py`）。
- **记录日期**：2026-11-12
