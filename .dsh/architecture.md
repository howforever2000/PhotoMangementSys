# 架构（architecture）

## 三层进程模型
```
前端 Vue3+TS (src/)  ──Tauri IPC(invoke)──▶  Rust 主进程 (src-tauri/src/)  ──HTTP(127.0.0.1:动态端口)──▶  Python VCR 微服务 (python/vcr)
                                                    │
                                                    └─ SQLite: photos.db (主) + persons.db (人物)
                                                    └─ 文件系统: 相册=本地文件夹；缩略图/头像/向量落盘
```
- 前端只经 Tauri 命令与后端交互；Rust 主进程是**唯一**读写 SQLite 与文件系统的角色。
- AI 能力全部下沉到 Python 微服务，Rust 通过 HTTP REST（`/classify_batch`、`/health` 等）调用。
- AI 创作是**独立的第二个** Python 服务（`python/ai_studio`），同样由 Rust 拉起、退出时优雅终止（FEAT-159）。

## 解耦原则（务必遵守，改动前先读 `features/*.md` 的入口层）
- **进程级解耦**：Rust ↔ Python 只靠 REST 契约交互；能力自动降级（GPU 失败回 CPU、模型缺失报可操作错误）。
- **客户端薄壳**：`vision.rs` 不依赖 `db` / `thumbnail` / `tone`，只做 HTTP 客户端。
- **命令层薄壳**：`#[tauri::command]`（注册在 `lib.rs`）只做参数校验/转发，业务落在 `content.rs` / `album.rs` / `persons.rs` / `dedupe.rs` 等；编排集中在明确入口层，不散落到工具函数。
- **配置集中**：Python 侧配置集中在 `python/vcr/config.py`（线程拓扑、`CHANNEL_KEYS` 加速分配、门控开关 `OCR_GATE`/`DECODE_MIN_LONG` 等）。

## VCR 微服务分层（python/vcr）
- 接口层：`python/server.py`（FastAPI，`/health`、`/classify_batch`、`/accel`，`/health.ov` 等；`VCR_API_VERSION` 随端点变更抬版本）。
- 核心：`config.py`（配置/档位/线程/加速表）、`model_registry.py`（会话管理、provider 分配、GPU 串行锁 `_GPU_RUN_LOCK`、`accel_info/set_accel`）、`preprocess.py`（`Letterbox`/`letterbox`/`face_align`，单点 `exif_transpose`）、`timing.py`、`ov_backend.py`（OpenVINO 可选运行时）。
- 服务件：`services/{classifier,detector,face_service,ocr_service,clip_subgraph,embed_service,tone_service,arbitrator,pipeline}.py`；`persistence/person_store.py`（persons.db）。
- 关键设计：一次解码多路分发（FEAT-079）、条件触发专家通道（OCR 前置门 FEAT-127）、仲裁器融合、自动降级、GPU 可选（单消费者串行队列 FEAT-080）。

## 数据模型（SQLite）
- 主库 `photos.db`：
  - `db/mod.rs`：`users, albums, folders, folder_tags, album_tags, folder_albums, album_stats, album_photo_excluded, photo_ratings, album_merged_sources, photo_marks, app_settings`
  - `db/category.rs`：`photo_categories, photo_category_hits, clip_text_cache`
  - `db/content.rs`：`photo_content_scan, face_rebuild_queue`
  - `db/embedding.rs`：`photo_embeddings, photo_text_embeddings`
  - `db/overview.rs`：`overview_cache`；`db/thumb_cache.rs`：`photo_thumb_cache`；`session.rs`：`sessions`；`db/health.rs`：健康体检聚合（FEAT-152）
- 独立库 `persons.db`（`persons.rs`）：`persons, faces`（+ `face_overrides` 人工改挂覆盖，FEAT-132）
- 数据目录：`data_dir()`（`VCR_DATA_DIR` → `app_data_dir/vcr-data`）；模型目录：`vision::resolve_model_dir()`（`VCR_MODEL_DIR` → `resource_dir/vcr/models` → 源码 `python/models`；不可写时硬链接到 app_data）。详见 `knowledge/data-path-and-packaging.md`。

## 关键数据流
- **扫描入库**：前端触发 → `content.rs::scan_album_combined`（后端单飞，三腿并发：影调 ∥ 语义向量 ∥ AI 识别 FEAT-077）→ HTTP 调 VCR → 落 `photo_content_scan` / `photo_embeddings` / 缩略图缓存 → 收尾后台 task（prewarm、分类重建、描述索引重建）。时序纪律见 `knowledge/scan-pipeline.md`。
- **语义搜索**：query → CLIP 文本塔编码 → `vecindex.rs` 向量与描述索引（连续 f32 + rayon 点积）→ 两路并集一次回表 → 分路相对相关度加权融合（FEAT-154/155）。见 `features/semantic-search.md`。
- **人物**：VCR 人脸通道 → `persons.db` 聚类 → `persons.rs`/`avatar.rs` 生成头像/封面 → 前端 `PersonGallery.vue`。见 `features/person.md`。

## 性能要点（改动勿回退）
- 缩略图 DCT 域降采样解码（7.7s → 0.76s）；缩略图缓存键为身份哈希 + 产物版本（见 `knowledge/thumbnail-cache.md`）。
- 批量识别 64；批量扫描三腿并发 + 三层耗时记账（FEAT-076/077）。
- 时间线按月分页 + 窗口化（首包 14.10 MB → 3.91 KB，FEAT-122）；概览快照「数据变动才重算」（14,438 KB → 3.35 KB，FEAT-123）。
- GPU/NPU：数值一致性校验 + DML 设备级串行锁 + 固定 `onnxruntime==1.24.4`（见 `knowledge/gpu-dml-constraints.md`）。

## 常用约定
- 命令注册集中在 `src-tauri/src/lib.rs`；前端能力授权在 `src-tauri/capabilities/*.json`。
- 新增服务端点要抬 `VCR_API_VERSION`（BUG-20261001-017）。
- 主题/样式改动走令牌层 `src/assets/tokens.css` + `main.css`，遵守 `knowledge/ui-contrast-tokens.md` 与 `knowledge/frontend-style-pitfalls.md`。
