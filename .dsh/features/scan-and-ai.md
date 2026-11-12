# 扫描与 AI 识别（scan-and-ai）

> 组合扫描（EXIF / 影调 / AI 三合一）+ Python VCR 微服务（分类/检测/人脸/OCR/CLIP/影调）+ 模型下载 / GPU·NPU 加速 / 性能设置。功能清单见 `features/index.md`。

## 核心能力（功能 ID）
- 单项：EXIF 扫描（FEAT-017）、影调分析（FEAT-018）、视觉识别 v2/v3/v4（FEAT-019 / F20 / F21）
- 内容扫描表 + AI 入库（FEAT-022）、结果表分页（FEAT-023）、GPU 加速可行性探测（FEAT-025）、组合扫描三合一（FEAT-026）
- 批量扫描入库（FEAT-037）、图片扫描 Hub（FEAT-038）、缩略图反向索引缓存 + 预热（FEAT-044）
- 扫描性能设置 GPU 开关/模型可选（FEAT-051）、应用内模型下载（FEAT-052 / 多镜像 FEAT-061 / 断点续传 FEAT-062 / 缺模型弹窗 FEAT-114）
- 模型切换 / GPU 实测验证 / 失败 CPU 回退（FEAT-053）
- 扫描分组工具后台异步 + 多线程 + 推荐线程数（FEAT-069）
- 三层细粒度耗时记账（FEAT-076）、三腿并发（FEAT-077）、后处理向量化（FEAT-078）、一次解码多尺度派生（FEAT-079）、GPU 会话串行锁（FEAT-080）、性能设置重构（FEAT-081）、扫描入库优化清单 P0–P23（FEAT-082）
- AI 腿解码降采样（FEAT-126）、OCR 前置门（FEAT-127）、CPU 线程默认取 P 核数（FEAT-128）、人物检测模型纳入下载（FEAT-129）
- 语义上 NPU / OpenVINO 可选运行时（FEAT-133 / 135 / 136 / 141）、加速默认开启（FEAT-139）、NPU 下载按钮按硬件探测显示（FEAT-158）
- 扫描分区统一路由型 tab（FEAT-151）、HEIC/HEIF 支持（FEAT-142）、应用退出优雅终止两个 Python 服务（FEAT-159）

## 入口文件
- 视图：`src/views/ScanHub.vue`、`src/views/TestScan.vue`、`src/views/SmartAlbum.vue`
- 组件：`src/components/GlobalScanPanel.vue`、`ScanPanel.vue`、`src/components/settings/*`（`AccelSection.vue`/`HardwareSection.vue`/`ModelDownloadRows.vue`/`ModelMissingDialog.vue`/`FaceTierSection.vue`/`ThreadsSection.vue`）、`src/components/DedupePanel.vue`
- 前端状态/工具：`src/stores/content.ts`、`src/utils/scanModuleTabs.ts`、`src/utils/scanTask.ts`
- Rust：`src-tauri/src/content.rs`、`photo_scan.rs`、`vision.rs`、`model_dl.rs`、`thumbnail.rs`、`tone.rs`、`category.rs`、`scan_timing.rs`、`scan_perf.rs`、`db/content.rs`、`lib.rs`
- Python：`python/server.py`、`python/vcr/{config,model_registry,preprocess,timing,ov_backend}.py`、`python/vcr/services/{classifier,detector,face_service,ocr_service,clip_subgraph,embed_service,tone_service,arbitrator,pipeline}.py`

## 关键约束与历史坑（多数已蒸馏为 knowledge）
- 部分勾选 + UPSERT 全字段覆盖会清空未采集字段 → 用 `COALESCE`。详见 `knowledge/scan-pipeline.md`（BUG-2026-0910-007）。
- 有副作用的耗时命令必须**后端单飞**；前端 `running` 布尔不是并发控制。详见 `knowledge/scan-pipeline.md`（BUG-2026-1027-006）。
- 并发段耗时起止都要在 future 内部读（否则三条腿耗时都等于 join 时长）。详见 `knowledge/scan-pipeline.md`（BUG-2026-0928-003）。
- 子进程生命周期（版本协商 / 动态端口 / 收养验 PID / 退出终止）。详见 `knowledge/python-sidecar-lifecycle.md`。
- GPU/NPU 数值一致性、DML 设备级串行锁、固定依赖版本。详见 `knowledge/gpu-dml-constraints.md`。
- 缩略图缓存键身份 vs 产物版本。详见 `knowledge/thumbnail-cache.md`。
- 下载 URL 用仓库内路径 `repo_file()`，本地落位用 `rel_file()`。详见 `knowledge/data-path-and-packaging.md`（BUG-2026-0921-001）。
- **EXIF Orientation=6** 竖持照片：`preprocess.open_image` 单点 `exif_transpose`，`persons.rs` 按显示坐标裁，`THUMB_RENDER_VERSION` 掺版本。来源：`bug-notes.json` BUG-2026-1014-002。
- 新增服务端点后要抬 `VCR_API_VERSION`，否则前端版本协商 404。来源：`bug-notes.json` BUG-20261001-017。
- 收尾 prewarm 双缺陷（合并请求预热路径被丢弃 + 单飞循环重复执行）仍在 open。来源：`bug-notes.json` BUG-2026-0929-003。
