# 推理后端多平台编排（方案 B′）· 设计与决策

> **状态**：`proposal` —— 待决策，未动一行代码
> **来源**：对「NPU 能否加速扫描入库」的实测追问（三轮探针），加上一条硬约束——**必须同时适配 AMD**。
> **前置提交**：`c3eefc3`（已推 `origin/dev` 与 `devremote/dev`），探针脚本入库，本文档引用其数据。
> **一句话**：原版方案 B（整个推理层迁 OpenVINO 原生 Core）**只适配 Intel**，按「要考虑 AMD」的标准应当否掉；但 B 想解决的两个真问题与厂商无关，所以提出 **B′：后端可插拔 + 按通道自动选派**，其改动量反而比 B 小。

---

## 一、先把 B 否掉：它只适配 Intel

原版方案 B 的表述是「整个推理层迁到 OpenVINO 原生 `Core`（一个库多 device）」。这条路的事实边界：

| 后端 | Intel CPU | Intel iGPU | Intel NPU | **AMD GPU** | **AMD NPU** | NVIDIA |
|---|---|---|---|---|---|---|
| OpenVINO 原生 `Core` | ✅ | ✅ | ✅ | ❌ 不支持 | ❌ 不支持 | ❌ 不支持 |
| ORT + DML（现状） | ✅ | ✅ | ❌ | ✅ | ❌ | ✅ |
| ORT + OpenVINO EP | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| ORT + VitisAI EP | — | — | — | — | ⚠️ 待验证 | — |

OpenVINO 的 GPU/NPU 插件只认 Intel 自家硬件，社区结论一致（`openvino#452`、官方 system requirements）。**所以「全量迁 OpenVINO」= 把 AMD 机器（也就是本仓库所有性能基线实测所在的那台 7840HS）变成纯 CPU 跑**——这是拿掉能力，不是重构。

> **判据**：`config.py` / `pipeline.py` / `model_registry.py` 里 15 处性能基线注释全部标注 `7840HS`。AMD 不是"顺带考虑"的平台，它是**现行基线平台**。任何方案只要让 7840HS 变差，就不成立。

---

## 二、但 B 想解决的两个问题与厂商无关，必须保留

| 问题 | 现状证据 | 是否厂商相关 |
|---|---|---|
| **① DML 全局串行锁** | `model_registry._GPU_RUN_LOCK` + 注释「DML 会话非线程安全，P≥2 直接 ACCESS_VIOLATION」⇒ GPU 变单消费者队列 | 锁本身是 DML 的约束，但"单队列"这件事换成任何加速器都还在 |
| **② 无法按通道选派设备** | 各通道最优设备不同（实测：det 最优 iGPU 7.0ms、face_det 绝对不能用 NPU 115.9ms、CLIP 最优 NPU 19.7ms），却只有一个全局 GPU 勾选 | 与厂商无关，是**架构缺一层路由** |
| **③ 档位用硬编码替代能力探测** | fp16 档写死 CPU（`_clip_providers`）、fp32 档专为 DML 存在、`CLIP_MODEL_META` 里还写着"未单独实测" | 与厂商无关 |

所以正确做法不是"选一个库"，而是**把「用哪个后端、跑在哪个设备」变成数据而不是代码**。

---

## 三、B′ 的核心洞察：不需要多进程，也不需要迁走 ORT

原以为"换后端"必须全量迁移，但关键事实是：

> **`openvino`（原生）与 `onnxruntime-directml` 是两个不同 import 名的包，可在同一进程共存。**

同名互斥只发生在 `onnxruntime` / `onnxruntime-directml` / `onnxruntime-openvino` / `onnxruntime-gpu` 之间。**只要我们不用 ORT 的 OpenVINO EP，而是直接调 OpenVINO 原生 API，就能同时拥有两条后端**：

```
vcr-server (单进程，HTTP 契约不变)
├── OrtBackend    ← onnxruntime-directml：det / face_det / face_rec / ocr
│                    设备候选 CPU / DML(AMD·NVIDIA·Intel 全系 GPU)
└── OvBackend     ← openvino 原生 Core：clip_vision / clip_text
                     设备候选 CPU / GPU(仅 Intel) / NPU(仅 Intel)
```

- **AMD 机器**：`OvBackend` 探测不到 GPU/NPU → 该后端整体不启用，所有通道回落 `OrtBackend`，**行为与今天逐字一致**，不会更差。
- **Intel 机器**：`OvBackend` 把 CLIP 图塔放到 NPU（实测 19.7ms vs ORT-CPU ~104ms）→ 白赚，且不抢 CPU。
- **将来要接 AMD NPU（VitisAI）**：那才是真正需要"同名互斥"的场合 → 预留 `IBackend` 接口 + 子进程宿主，第一版不做。

**这个方案的最大优点是：它比 B 小得多。** 不动 ORT、不动 Rust 侧 HTTP 契约、不动现有四个通道的推理代码，只新增一层路由 + 一个新后端，且新后端首期只接一条腿（CLIP 图塔）。

---

## 四、平台矩阵（每格都必须有降级路径）

| 平台 | det / face / ocr | CLIP 图塔 | 语义腿预期 |
|---|---|---|---|
| Intel CPU 无核显（老机器） | `ort-cpu` | `ort-cpu` | 与今天一致 |
| Intel 核显（现状最普遍） | `ort-dml` | `OvBackend@GPU` 或 `ov@NPU` | 26.1 → **19.7ms** |
| Intel NPU（Core Ultra） | `ort-dml`（**NPU 显式排除**） | `OvBackend@NPU` | 同上 + 不抢 CPU |
| **AMD 核显（7840HS 基线机）** | `ort-dml` | `ort-dml`（fp32 档）或 `ort-cpu` | 与今天一致，**不变差** |
| NVIDIA 独显 | `ort-dml` | `ort-dml` | 与今天一致 |
| AMD Ryzen AI NPU | 不做（VitisAI 生态待验证） | 不做 | — |
| Qualcomm ARM | 不做 | 不做 | — |

> **硬规则**：`face_det` 在 NPU 上的候选资格**永久排除**。实测 115.9ms（iGPU 14.0ms / CPU 36.1ms），这不是调优空间，是 8 倍差距。把它写进代码注释而不是靠自检发现，避免后人重试。

---

## 五、接口契约

```python
# python/vcr/backends/base.py
class Backend(Protocol):
    name: str                      # "ort-dml" | "ort-cpu" | "ov"
    def probe(self) -> list[DeviceInfo]: ...      # 可用设备 + 每设备的能力事实
    def load(self, key: str, spec: ModelSpec) -> SessionHandle: ...
    def run(self, h: SessionHandle, feeds: dict) -> list[np.ndarray]: ...
    def unload(self, key: str) -> None: ...

@dataclass
class DeviceInfo:
    device: str          # "CPU" | "GPU" | "NPU" | "DML"
    vendor: str          # "intel" | "amd" | "nvidia" | "unknown"
    name: str            # "Intel(R) AI Boost"
    # 后端自己声明的硬约束，供路由器过滤
    constraints: dict    # {"static_shape": True, "fp16_only": True, "max_batch": 1}
```

**路由器只做三件事**：查 `perf_profile.json` → 命中就用 → 未命中或指纹变化则跑自检再落表。它**不做推理决策之外的任何事**（不解释、不静默回退到"看起来能用"的档位）。

### `perf_profile.json` schema（落 `python/models/`，与 `current_*.json` 同目录同风格）

```json
{
  "fingerprint": {
    "cpu": "Intel(R) Core(TM) Ultra 5 245K",
    "devices": ["CPU", "GPU:DML", "NPU"],
    "ov_version": "2026.4.1", "ort_version": "1.24.4",
    "models": { "clip_vision": "<sha1前12位>", "det": "...", "...": "..." },
    "generated_at": "2026-11-XXT..."
  },
  "channels": {
    "clip_vision": {
      "chosen": { "backend": "ov", "device": "NPU", "avg_ms": 19.66 },
      "candidates": [
        { "backend": "ov", "device": "NPU", "avg_ms": 19.66, "cos_vs_ref": 0.999997 },
        { "backend": "ov", "device": "GPU", "avg_ms": 26.11, "cos_vs_ref": 0.999992 },
        { "backend": "ort", "device": "CPU", "avg_ms": 104.2, "cos_vs_ref": 1.0 },
        { "backend": "ort", "device": "DML", "avg_ms": 53.0, "cos_vs_ref": 0.9999 }
      ],
      "rejected": [
        { "backend": "ov", "device": "GPU", "reason": "compile_cost", "...": "首次 3.1s" }
      ]
    },
    "face_det": {
      "chosen": { "backend": "ort", "device": "DML", "avg_ms": 14.0 },
      "rejected": [
        { "backend": "ov", "device": "NPU", "reason": "hard_excluded", "...": "实测 115.9ms = 8x" }
      ]
    }
  }
}
```

---

## 六、自检的硬规则（先过滤，再排序）

顺序不可颠倒——**"跑得动"永远是最后一件事**。

1. **能力过滤**：模型 shape 是否满足 `constraints`（NPU 要求静态 shape；实测动态维会让 NPU 编译器 `LLVM ERROR` 直接带走进程，不可 catch）
2. **数值过滤**：以 `ort-cpu` 输出为基准算余弦/IOU，低于阈值直接淘汰
   - CLIP：`cos ≥ 0.9999`（实测两档 NPU 均 0.999997+，通过）
   - 检测/分类：mAP/IOU 等价（仓库已有 `verify_postprocess_equiv.py` / `verify_det_source_equiv.py` 可直接复用）
   - **这条规则的历史来源**：`BUG-2026-0910-006` —— fp16 档在 AMD DML 上静默产出错误向量。当年用硬编码 `return ["CPUExecutionProvider"]` 补的洞，现在应该用规则表达
3. **成本过滤**：加载/编译成本超阈值降权（NPU 首次编译：fp16 档 0.7s / fp32 档 6.3s）
4. **排序**：按**整批扫描边际吞吐**排序，不按单张 ms。单张延迟排序会得出"全上 NPU"，但 NPU 单队列、DML 单队列、CPU 14 核可叠——这是三个完全不同的资源形状

> **不能"每次计算"**：自检有确定成本（NPU 编译 0.7–6.3s、DML 会话 1.9–3.1s、首轮 warmup）。形态必须是「一次性自检 → 落表 → 指纹变了才重测」。UI 上给「重新自检」按钮兜底。

---

## 七、分阶段实施（每阶段独立验收、独立回滚）

| 阶段 | 内容 | 工作量 | 风险 | 验收 |
|---|---|---|---|---|
| **P0** | **只读自检工具**：`bench/selfcheck_hardware.py` 跑完通道×设备矩阵，产出 `perf_profile.json`，**不改推理路径** | 0.5 天 | 极低 | 表能生成；INTEL/AMD 两台机器各出一次 |
| **P1** | 抽 `IBackend` + `OrtBackend`，行为与今天**逐字一致**（等价性用现有 bench 全集回归） | 2 天 | 低 | 全量 bench 数字无劣化；`/health` 字段不变 |
| **P2** | 落 `OvBackend`，**只接 CLIP 图塔**，开关默认关 | 2–3 天 | 中 | 数值 cos≥0.9999；端到端扫描吞吐 ≥1.2x（Intel）；**AMD 上开关自动不可用** |
| **P3** | 路由器接 dispatch 表；det / ocr 纳入候选（NPU 对 face_det 永久排除） | 2 天 | 中 | 关闭时完全回退；开启后无新增崩溃 |
| **P4** | UI 收敛：四组旋钮 → 「自动优化（推荐）」+「高级」折叠 + 结论展示 | 2 天 | 低 | 默认路径零操作即可用 |
| **不做** | AMD VitisAI NPU、Qualcomm、CUDA EP、Windows ML 聚合器 | — | — | 只留 `IBackend` 接口位 |

**总计约 9–10 个工作日**。P0/P1 无行为变化，可在任何时点停手不欠债。

### OpenVINO 运行时体积问题的解法

`openvino` 装上占 **297 MB**，不能无条件塞进 MSI（现在安装包只带 `model_fp16.onnx` + 4 个通道模型，白名单在 `release.tauri.conf.json`）。方案：

> 把 OpenVINO 运行时做成**按需下载组件**，复用现成的 `model_dl::specs()` 机制——只有探测到 Intel 核显/NPU 才在性能设置里推荐下载；AMD/NVIDIA 用户永不下载。

这样既不给所有用户加体积，也不妨碍 Intel 用户吃到加速。

---

## 八、风险清单（按"会不会静默出错"排序）

| # | 风险 | 已有证据 | 处置 |
|---|---|---|---|
| R1 | **NPU 动态维导致进程 LLVM abort（不可 catch）** | 实测 `clip_text`（attention_mask 无界动态）与未 reshape 的 `clip_vision` 都触发过 | 加载前强制静态 shape 校验；NPU 会话放**独立可重启子进程**兜底 |
| R2 | 换后端后向量空间漂移 → 库里向量与查询侧模型 id 失配 | `content.rs:1125` 已记录同类坑（"向量在库里却一条都搜不出来"）；`photo_embeddings.model` 是硬 key | 后端变更**不得**改变 `model` id；改 id 必须走"全库重扫"显式流程并提示用户 |
| R3 | AMD 上被"优化"变慢 | fp16 档在 AMD DML 上有算子级数值 bug（历史） | 自检在 AMD 上必须跑一遍；AMD 缺 OV 设备 → `OvBackend` 整体禁用，天然安全 |
| R4 | 打包体积膨胀 | openvino 297MB | 按需下载组件（见上） |
| R5 | 首次启动变慢 | NPU fp32 档编译 6.3s | `CACHE_DIR` 落盘 + 自检异步 + UI 显示进度 |
| R6 | 两套栈同时加载导致显存/内存压力 | OpenVINO + ORT 各持一份上下文 | 自检阶段测峰值内存；超阈值时只保留赢家那头 |
| R7 | AMD Ryzen AI NPU 被过度承诺 | VitisAI EP 的 Windows 支持信息多来自 2026 年公告级来源，未独立实测 | 明确列为"不做"，不进任何 UI 文案 |

---

## 九、验收标准（可量化，缺一不算完成）

1. **AMD 不改差**：7840HS 上全量 bench（`photo_parallel_bench.py` + `bench_cpu_gpu_modes.py`）数字不劣于 `c3eefc3`
2. **Intel 有收益**：CLIP 图塔 ≥1.5x（19.7ms vs ~104ms），整册扫描墙钟 ≥1.2x
3. **数值不漂移**：所有走新后端的通道 `cos ≥ 0.9999` 或等价 IOU；`photo_embeddings.model` 值不变
4. **可一键回退**：开关关掉后 `/health`、会话表、吞吐与 `c3eefc3` 完全一致
5. **无新崩溃**：`cargo test --lib` / `vue-tsc --noEmit` / 全量 bench 三轮零崩溃

---

## 十、待你拍板的三个决策点

| # | 决策 | 选项 |
|---|---|---|
| **D1** | 是否接受 B′（后端可插拔 + 路由）作为方向 | ①接受，从 P0 起 ②只做 P0 拿数据再定 ③不做，维持现状并把 fp16 的 CPU 硬编码改成显式提示 |
| **D2** | OpenVINO 运行时怎么进产品 | ①按需下载组件（推荐） ②打进 MSI（+297MB） ③仅开发环境可用 |
| **D3** | 语义档位是否退场 | ①保留两档（不动现状） ②后端化之后合并成单档，档位只剩精度/体积语义 |

> **我的建议**：先批 **P0**（半天，只读，零风险）——拿到 Intel + AMD 两台机器的真实 dispatch 矩阵之后，D2/D3 会自然有答案，也能验证"是否值得投入后面 9 天"。

---

## 附录 A · 本机实测数据（Intel Ultra 5 245K / Intel AI Boost NPU / OpenVINO 2026.4.1）

单流、定长输入、`bench/probe_npu_feasibility.py` + `probe_npu_clip_tiers.py` 产出：

| 通道 | 输入 | NPU | iGPU | CPU | 结论 |
|---|---|---|---|---|---|
| det `yolov8n-det` | 1×3×640×640 | 12.3 ms | **7.0 ms** | 15.2 ms | iGPU 最优 |
| face_det `det_10g` | 1×3×640×640 | **115.9 ms** | **14.0 ms** | 36.1 ms | NPU 8x 更慢，永久排除 |
| face_rec `w600k_r50` | 1×3×112×112 | 8.2 ms | **6.5 ms** | 22.1 ms | iGPU 最优 |
| ocr `paddleocr-det` | 1×3×960×960 | 33.1 ms | 24.4 ms | 25.8 ms | iGPU 最优 |
| clip_vision fp16 档 | 1×3×224×224 | **20.2 ms** | 26.1 ms | 57.5 ms | **NPU 最优** |
| clip_vision fp32 档 | 1×3×224×224 | **19.7 ms** | 26.1 ms | 58.1 ms | **NPU 最优** |

数值等价（vs CPU fp32 基准余弦）：NPU 0.999997–0.999998 / iGPU 0.999988–0.999992 → **通过**，非 `BUG-2026-0910-006` 同类。

## 附录 B · 待核实的账面矛盾（与本文档并行，不阻塞）

1. `CLIP_MODEL_META["b16"]["accuracy"]` 写「零样本 81.1%」，但 `bench/out/clip_accuracy.json` 实测 `0.7736`（53 张）→ **两个数打架**
2. `b16-fp32` 的 accuracy 字段写「未单独实测」→ 但它正是被讨论"要不要删"的那一档，删之前应补测
3. `python/models/` 实测 5117 MB，其中约 **2760 MB 可清**（`chinese-clip-vit-b-16/` 中转残留 1520MB、`yolov8*-cls`+Places 死模型 505MB、fp32 档 719MB），但这些**本来就不在 MSI 白名单里**，清了对安装包体积无影响，只省开发机磁盘
4. 发布包只带 fp16 档、而 fp16 被硬编码 CPU ⇒ **安装版「GPU 加速」勾选对语义塔 100% 无效**（用户须再下 754MB 的 fp32 档才用到 GPU）—— 这属于本节 R2/D3 的范畴，建议随 P3 一并处理
