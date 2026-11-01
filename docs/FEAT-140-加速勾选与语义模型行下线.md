# FEAT-140 · 性能设置：去掉「语义模型」行，加速改成 NPU / 核显两个独立勾选

> **状态**：`done`（2026-11-03）
> **触发**：用户要求「语义模型那行去掉（只有一档、不选了）；NPU 和 GPU 分开：
> 有些用户没 NPU 或没下载组件就不显示；只勾 GPU 就全在 GPU；两个都勾就分开」。

---

## 一、界面变化

### 之前

```
本机硬件  CPU 14 核 · NPU 可用 · 核显可用
加速      [五个行对照表]  [关闭加速]
语义模型  B/16 fp32 · 双塔 719MB · 已加载   ⟳      ← 用户要求删掉这行
人脸模型  高精度 det_10g + w600k_r50  [下拉]
```

### 现在

```
本机硬件  CPU 14 核 · NPU 可用 · 核显可用

加速                                    [刷新]
  ☑ NPU   检测 · 文字 · 人脸编码      ☑ 核显  全部计算
  ─────────────────────────────────────────
  人物 / 车辆检测   每张         NPU
  文字检测          约 7 成      NPU
  人脸编码          有脸时       NPU
  人脸检测          人物规则命中  核显
  缩略图 · EXIF · 地点  始终      CPU

人脸模型  高精度 det_10g + w600k_r50   [下拉]
▸ 高级    扫描批次 · 线程数 · 测速诊断
```

改动要点：

| 项 | 说明 |
|---|---|
| **删掉「语义模型」整块** | FEAT-137 只剩一档，没有「选」这件事；`ClipTierSection.vue` 整个文件删除（168 行） |
| **服务探针换人** | 原来靠「语义模型」那次 `list_vcr_models` 判断服务是否可用；现在由 `AccelSection` 调 `/accel` 兼任（最轻且必然依赖服务），失败→横幅/成功→解锁的语义不变 |
| **两个独立勾选** | 不再是「一个开关 + 一张表」，而是「勾哪个用哪个」 |
| **不可用不渲染** | 没有 NPU 硬件 → 连行都不出现；有 NPU 但没装运行时 → 只出现一行「需先装运行时 [下载 15MB]」（不是假勾选框） |

## 二、勾选 → 通道分配的映射（实现即这句话）

| 勾选 | det | ocr | face_rec | face_det | 效果 |
|---|---|---|---|---|---|
| 只勾核显 | GPU | GPU | GPU | GPU | 四条全在核显 |
| 只勾 NPU | NPU | NPU | NPU | **CPU** | 能上 NPU 的三条走 NPU；核显没勾就不占它 |
| 两个都勾（默认） | NPU | NPU | NPU | GPU | **两个引擎并行**：触发最频繁的三条给 NPU，人脸检测给核显 |
| 都不勾 | CPU | CPU | CPU | CPU | 老行为 |

> ⚠️ 「只勾 NPU」时 `face_det` 必须**显式**写 CPU。留空会跟随自动探测又跑回核显
> —— 勾选语义就破了（这条已在实现里注释，回归时最容易踩）。

## 三、实测（`.pi/smoke_feat140.py`，逐通道读 `/health` 会话表）

```
[auto]      mode=auto channels={det:NPU, ocr:NPU, face_rec:NPU, face_det:GPU}
[只勾核显] ✓ {det:GPU, face_det:GPU, face_rec:GPU, ocr:GPU}
[只勾 NPU] ✓ {det:NPU, face_det:CPU, face_rec:NPU, ocr:NPU}
[两个都勾] ✓ {det:NPU, face_det:GPU, face_rec:NPU, ocr:NPU}
[都不勾]   ✓ {det:CPU, face_det:CPU, face_rec:CPU, ocr:CPU}
```

会话表读的是 ORT 实际绑定（`get_providers()`），不是请求值 —— 所以这是「真的跑在
那个引擎上」的证据，不是配置回显。

## 四、代码落点

| 文件 | 改动 |
|---|---|
| `src/components/settings/AccelSection.vue` | 重写：两个勾选 + `channelsFor(useNpu, useGpu)` 映射 + 对照表 + 组件下载行；兼任服务探针（`emit failed/recovered`） |
| `src/components/ModelGpuSettings.vue` | 移除语义模型分区与其 import；探针事件改接 AccelSection |
| `src/components/settings/ClipTierSection.vue` | **删除**（168 行） |
| `src/components/settings/FaceTierSection.vue` | 注释同步（探针换人） |

服务端无需改动：`/accel` 本来就接受任意 `channels` 映射，勾选组合只是不同的映射表。

## 五、验收

| 项 | 结果 |
|---|---|
| `vue-tsc --noEmit` | 0 error |
| `vite build` | 通过（2.2s） |
| 四种勾选组合 | 全部按映射落地（见上表） |
| 默认态 | `current_accel.json` 不存在 → `mode=auto` → 两个勾选都是勾上状态 |
| 无加速引擎的机器 | 两个勾选都不渲染，只显示「这台机器没有可用的加速引擎，只跑 CPU」 |

## 六、残留

| # | 项 | 说明 |
|---|---|---|
| R1 | 「只勾 NPU」时人脸检测在 CPU 上较慢（det_10g 37.9ms vs 核显 16.5ms） | 这是用户显式选择的结果（没勾核显）；`face_det` 在 NPU 上 116ms，所以没有第三个选项可给 |
| R2 | 勾选与通道表的对应关系写在两处（前端 `channelsFor` / 后端 `default_channels`） | 前者是用户意图，后者是「首次默认」；两者故意独立（用户可偏离推荐），单测在 `.pi/smoke_feat140.py` 里对着前者验 |
| R3 | 界面仍未做真机截图复核 | Playwright 依赖 Tauri `invoke`，纯浏览器渲染会全报错 |
