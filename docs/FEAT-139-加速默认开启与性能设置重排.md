# FEAT-139 · 加速默认开启 + 性能设置界面重排（按「加速对象」组织）

> **状态**：`done`（2026-11-02）
> **触发**：用户指出「一般默认 GPU 加速，所以用不到 CPU 跑模型 —— CPU 只是缩略图生成、
> EXIF 扫描、地点解码」，并要求「优化性能设置界面：现在每个按钮和加速对象是干什么的
> 比较难理解，优化布局和说明，去除多余文字」。

---

## 一、两个判断先被验证

| 判断 | 结论 |
|---|---|
| 「模型不该默认跑 CPU」 | 成立。之前 `current_accel.json` 不存在时等于**不加速**，只有用户手动点开关才生效；而实测加速把 AI 腿从 336.6 压到 140.1 ms/张（−58%）。 |
| 「CPU 只做缩略图 / EXIF / 地点」 | 成立。那三项在 Rust 侧，与 `intra_op` 无关；模型搬到 GPU/NPU 后，**CPU 线程数这个旋钮基本不影响速度** —— 它却是旧界面的第一个控件。 |

## 二、默认策略：auto（未表态 = 按硬件开）

`config.accel_setting()` 从两态变三态：

| 文件状态 | mode | 行为 |
|---|---|---|
| `current_accel.json` 不存在（全新安装 / 从未点过） | `auto` | **按硬件推荐并直接生效**：有 NPU → det/ocr/face_rec 给 NPU、face_det 给核显；只有核显 → 四条全给核显；两者都没有 → 全部 CPU（等价于未启用） |
| `{"enabled": true, ...}` | `on` | 用户显式开，按表执行 |
| `{"enabled": false}` | `off` | 用户显式关，全部回 CPU（老行为逐字一致） |

「推荐分配」只在真有可用引擎时才生效（全 CPU 的推荐会被当作「不改动」），
所以老机器/无显卡机器不会被静默钉成 CPU 而失去旧的 GPU 开关语义。

实测（`/accel`）：全新安装 → `mode=auto enabled=true channels={det:NPU, ocr:NPU, face_rec:NPU, face_det:GPU}`，
会话表 `det=['OpenVINO:NPU']`、`face_det=['DmlExecutionProvider']`。

## 三、界面重排

### 之前（自上而下）

1. 「这台机器现在怎么跑」两行摘要（语义索引 / 人物与文字）
2. ① 本机硬件 — **GPU 加速开关**（检测 + 开/关）+ **CPU 线程数**（下拉）
3. ①b 加速分配 — 四行表 + 请求/实际两列 + 大段说明 + 组件下载
4. 语义模型 — 下拉 + 换档确认
5. 人脸模型 — 下拉
6. 高级（折叠）— 扫描批次 + 推理测速

**问题**：两个开关表达同一件事（全局 GPU vs 通道分配），没人知道哪个生效；
顶部摘要与下面的表重复；第一眼看到的是最不重要的「CPU 线程数」；按钮旁边全是长句。

### 现在

```
本机硬件   CPU 14 核 · NPU 可用 · 核显可用            ← 一行事实，无按钮

加速       [NPU 3 条 + 核显 1 条]  默认      [关闭加速]   ← 唯一开关
           人物 / 车辆检测   每张         NPU
           文字检测          约 7 成      NPU
           人脸编码          有脸时       NPU
           人脸检测          人物规则命中  核显
           缩略图 · EXIF · 地点  始终       CPU          ← 明确划出开关边界

语义模型   B/16 fp32 · 双塔 719MB · 已加载            ⟳
人脸模型   高精度 det_10g + w600k_r50              [下拉]

▸ 高级    扫描批次 · 线程数 · 测速诊断
```

删掉的东西：

| 删除 | 理由 |
|---|---|
| 「GPU 加速」开关（`GpuSection.vue` 整个文件） | 与「加速」分区是同一件事；分配表的默认值已包含「没有 NPU 时全部走核显」 |
| 顶部「这台机器现在怎么跑」摘要 | 与对照表重复 |
| 语义模型档位下拉 + 换档确认 | FEAT-137 已收敛为单档，改一行状态 |
| 「推理测速 / 会话实测」以外的长文案 | 统一收进「高级」 |
| 扫描面板的 `GPU: ❌ CPU (provider)` 徽章 | **NPU 在干活时它照样显示 ❌** —— 改成读实际生效分配（`NPU n 条 + 核显 m 条`） |

保留但降级的：

- **CPU 线程数** → 「高级」。模型走 GPU 后它只影响未启用加速时的推理；文案改成
  「语义索引自己用这个数的一半」。
- 组件下载（OpenVINO 运行时 15MB）→ 只在「NPU 可用但组件未装」时出现一行 + 下载按钮。

## 四、代码落点

| 文件 | 改动 |
|---|---|
| `python/vcr/config.py` | `accel_setting()` 三态（`enabled: None` = auto） |
| `python/vcr/model_registry.py` | `_device_of()` 处理 auto；新增 `auto_channels()`（缓存、全 CPU 时视为不改动）；`accel_info()` 增加 `mode` |
| `src/components/ModelGpuSettings.vue` | 重写：硬件 → 加速 → 语义模型 → 人脸模型 → 高级 |
| `src/components/settings/AccelSection.vue` | 重写：唯一开关 + 五行对照表 + 极简说明 |
| `src/components/settings/HardwareSection.vue` | 重写为只读一行（核数 / NPU / 核显） |
| `src/components/settings/GpuSection.vue` | **删除**（无引用） |
| `src/components/ScanPanel.vue` | 徽章改读加速分配；`fetchGpuStatus` → `refreshAccel` |
| `src/stores/content.ts` | 删 `gpuStatus` / `fetchGpuStatus` / `setVcrGpu`（UI 已无入口） |
| `src/types/content.ts` | `VcrAccel` 增加 `mode`；补 `VcrThreadsInfo` 的 `physical/p_cores/e_cores/hybrid` |

## 五、验收

| 项 | 结果 |
|---|---|
| `vue-tsc --noEmit` | 0 error |
| `vite build` | 通过（3.1s） |
| `/accel` 三态 | 全新安装 `auto`→NPU×3+核显×1；显式关 → `off` 且全部回 CPU；再开 → `on` |
| `/health` 会话表 | `det=['OpenVINO:NPU']`、`face_det=['DmlExecutionProvider','CPUExecutionProvider']` |
| 组件缺失 | NPU 可用但未装运行时 → 三条腿自动落回核显（`build()` 返回 None → ORT 分支） |

## 六、残留

| # | 项 | 说明 |
|---|---|---|
| R1 | 两处「默认」要区分 | 服务端 auto 分配（默认开）与用户显式表态（持久化）不是一回事；UI 上用「默认」小徽标标出来 |
| R2 | 界面未做真机截图复核 | Playwright 依赖 Tauri 的 `invoke` 后端，纯浏览器渲染会全报错；需要打开应用看一眼实际观感 |
| R3 | `gpuStatus` 相关的 Rust 命令（`get_vcr_gpu_status` / `set_vcr_gpu`）与 `/gpu` 保留 | 内部/CLI 仍可用；只是不再有 UI 入口 |
