# 创意工坊（workshop）

> 图像加工工具台：左栏选工具 + 右栏工作区（压缩上线，旧调色台整体下线 FEAT-150）；AI 创作走 Python 工作流服务 + Rust 命令层。功能清单见 `features/index.md`。

## 核心能力（功能 ID）
- AI 创作：Python 工作流服务 + Rust 命令层（主题手账 / 人物美颜 / 滤镜迁移 / 画质增强，FEAT-152 第一条）
- 工坊重做：调色台整体下线，改「左栏选工具 + 右栏工作区」图像加工台，压缩上线（FEAT-150）
- AI 创作计费规则 + 运行前费用预估（FEAT-156）、计费改为「按模型登记」（FEAT-157）
- 灯箱「编辑」直达创意工坊（带图打开即用，FEAT-065）
- **历史（已随 FEAT-150 删除，仅作教训）**：传统图像处理组件（FEAT-066）、区域直方图均衡（FEAT-063）、修图前后对比（FEAT-068）、基准调色非破坏引擎 FEAT-099（`src-tauri/src/edit/**` 已删）

## 入口文件
- 视图：`src/views/Workshop.vue`（重写：左栏工具导航 + 右栏工作区）
- 前端组件：`src/components/workshop/{CompressTool,PhotoTray,ToolIcon}.vue`、`src/components/workshop/ai/*`（`BeautyTool`/`JournalTool`/`StyleTransferTool`/`UpscaleTool`/`AiSettingsDialog`/`AiCostHint`/`AiResultPanel`）
- 前端状态/工具：`src/stores/aiStudio.ts`、`src/types/ai.ts`、`src/types/tools.ts`、`src/utils/aiPresets.ts`、`src/utils/bytes.ts`、`src/utils/pathParts.ts`、`src/composables/useAiBilling.ts`、`src/assets/ai-tool.css`
- Rust：`src-tauri/src/tools.rs`（图像压缩 `image_compress`）、`ai_studio.rs`、`ai_service.rs`、`ai_settings.rs`、`lib.rs`
- Python：`python/ai_studio/{server,config,jobs,presets,compose,schemas,logutil}.py`、`python/ai_studio/providers/openai_compat.py`、`python/ai_studio/workflows/{base,journal,beauty,style_transfer,upscale}.py`

## 关键约束与历史坑
- **AI 创作图像编辑 10054/404**：代码调 `/images/edits` multipart 端点而网关未实现 → 改 `/images/generations`。接入 OpenAI 兼容网关时务必按实际实现的端点走。来源：`bug-notes.json` BUG-2026-1111-002。
- **服务启动超时 20s / 无后端日志**：裸 python 命中 Windows Store 占位符 → 解析真解释器候选。详见 `knowledge/python-sidecar-lifecycle.md`（BUG-2026-0918-007）。
- **蒙版画布不透明黑底撞 `source-in`**：整张照片被红色铺满（FEAT-150 前 edit engine）。来源：`bug-notes.json` BUG-2026-0918-006。
- **结果态仍无条件绘制蒙版**：红叠加盖在结果图上不消失；`clearRect` 提到条件外无条件执行。来源：`bug-notes.json` BUG-2026-0918-010。
- AI 创作服务与应用退出：点叉退出要优雅终止 Python 服务（FEAT-159）。详见 `knowledge/python-sidecar-lifecycle.md`。
- 计费：前端费用预估必须与后端「按模型登记」的计费规则同源，避免展示与实际结算不一致（FEAT-156/157）。

## 本地增强（FEAT-163，纯本机·离线·免费）
- 三工具：胶片模拟（内置 7 个富士 .cube，可加颗粒/暗角）、参考图调色（Reinhard / 直方图，可导出 .cube）、画质修复（Real-ESRGAN x4 + 可选 GFPGAN 人脸）。
- Python：`python/ai_studio/lutcube.py` / `colortransfer.py` / `facedetect.py` / `localmodels.py` + `workflows/{film_sim,color_transfer,restore}.py`；LUT 在 `python/ai_studio/luts/*.cube`（CC-BY-NC-SA，见 `NOTICE.txt`）。
- Rust：`ai_studio.rs` 的 `match req.tool` 放行 `film_sim/color_transfer/restore`（免 provider）+ `local_models_status` 转调 `GET /local/models`。
- 前端：`src/components/workshop/ai/{FilmSimTool,ColorTransferTool,RestoreTool,OutputDirField,LocalModelHint}.vue`，左栏「本地增强」组；本地工具免费不计费。
- 模型：`python/models/local/{real_esrgan_x4.onnx(69MB),gfpgan_1.4.onnx(340MB)}`，从 facefusion-assets `models-3.0.0` 下载，工坊「本地模型」面板给下载链接。
- 坑（详见 `knowledge/local-enhance.md`）：`.cube` 轴序 [b,g,r]；SCRFD 需周边上下文（紧裁人脸检出率低，补边即可）；GPU 用 onnxruntime-directml，CPU 可联调。
