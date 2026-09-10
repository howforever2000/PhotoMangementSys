# 语义搜索 · 语义扫描集成 + 搜索融合 实施方案（v2 修订）

> **执行状态（2026-09-10）：Phase 1~4 已全部实施并通过验证** —— cargo check 零错误零警告、
> vue-tsc 通过、Python 路由冒烟（/health api_version=4、/embed_text 512维、/embed_batch 错误内联、
> fast tokenizer）、RRF 融合单测通过。int8 已按决策移除；语义召回已改为置信度范围（非 top-N）。

> 前置：`design/clip-model-test-report.md`（Phase 0 实测已完成：查询 <15ms、全库索引 9.3min/1.1万张、零样本 81.1%、int8 不作默认档）
> 本版按三项决策修订：① fp16 默认档 ② 语义扫描作为第 4 个扫描项并入 `scan_album_combined` ③ 语义结果融合进现有 `smart_search`（非独立入口）

---

## 0. 决策确认

| 决策 | 结论 |
|---|---|
| fp16 | ✅ 默认且唯一档（实测 int8 掉点 3.8pp > 2pp 门槛，**不提供 int8 档**）。打包仅 `onnx/model_fp16.onnx`(377MB) + `tokenizer.json`/`vocab.txt`/`preprocessor_config.json`，hf-mirror 直链 |
| batch 可选、默认 8 | ✅ 与现状天然对齐：`scan_album_combined` 已有 `batch_size: Option<i64>`，`unwrap_or(8).clamp(4,64)`——语义扫描直接复用该参数，零新增配置 |
| 多一个语义扫描项 | ✅ `scan_types` 白名单 `["basic","tone","ai"]` → 增加 `"semantic"`，前端三处勾选框各加一项即接入 |
| 融入现有搜索 | ✅ `smart_search` 命令内做 RRF 混合排序，前端搜索框/筛选器/结果组件零结构变动 |

## 1. 数据流总览

```
【扫描侧】scan_album_combined(scan_types + "semantic", batch=8)
  photo_thumb_cache.thumb_path（缺失→现场生成 grid 缩略图）
    → vision::embed_images_batch()  POST /embed_batch {paths}
    → photo_embeddings(photo_hash PK, dim, model, embedding BLOB f32 LE, generated_at)
    → emit content-scan-progress（stage:"semantic"）

【搜索侧】smart_search(keyword, 筛选器…)
  keyword 非空 → POST /embed_text {text} → [f32;512]
    → 内存向量缓存(user_id+version 失效) → 余弦 top-200
    → 现有筛选 SQL 按 hash 集合过滤
    → 与 FTS5 命中 RRF 融合(k=60) → SmartHit + semantic_score
  任何环节不可用（模型缺失/服务未启动/向量库空）→ 静默回落纯 FTS
```

## 2. Phase 1 · Python 服务端（fp16 接入）

| 文件 | 改动 |
|---|---|
| `python/download_models.py` | TASKS 加 `clip`：hf-mirror 下载 Xenova 仓库 5 个文件 → `python/models/chinese-clip/`；`model_dl.rs` 同步注册（Phase 2） |
| `vcr/config.py` | `CLIP_MODEL_DIR` / `CLIP_DIM=512` / `CLIP_MAX_LEN=52` 常量 |
| `vcr/model_registry.py` | 新增 `clip` 懒加载槽位（非必需，缺失→clip_ready=false 降级）；单会话双输入（Xenova 为单文件双塔，vision 走 pixel_values / text 走 input_ids，无需拆图）；provider 继承现有 DML→CPU 机制 |
| `vcr/preprocess.py` | `clip_image_tensor(path)`：resize 224 + CLIP mean/std（preprocessor_config.json 数值，与 bench 相同） |
| `vcr/services/embed_service.py`（新） | tokenizer：优先 `tokenizers` 库加载 tokenizer.json；打包异常时回落 `python/bench/minitok.py` 移植版（实测与 transformers 对齐）；双塔推理封装 |
| `server.py` | `POST /embed_text {text}→[512]`；`POST /embed_batch {paths}→[[512]]`（封顶 64/批，单张错误内联，仿 classify_batch）；`/health` 加 `clip_ready`；`VCR_API_VERSION 3→4`（宿主自动重启旧服务） |
| `requirements.txt` | `+tokenizers>=0.19`（PyInstaller hiddenimports 视需要补） |

**Phase 1 spike（半天）**：先跑通 Xenova 单文件双塔 graph IO（dummy input 各喂一次）；若输入输出名/维度异常 → `onnx.utils.extract_model` 一次性拆 vision/text 子图缓存，后续加载拆分件。

## 3. Phase 2 · Rust 索引层

| 文件 | 改动 |
|---|---|
| `src-tauri/src/db/embedding.rs`（新，仿 db/thumb_cache.rs） | 表 `photo_embeddings(photo_hash TEXT PRIMARY KEY, dim INT, model TEXT, embedding BLOB /*f32 LE*/, generated_at)`；`upsert_batch / lookup_hashes / delete_by_hashes / count_by_user / load_all_by_user`；主键与 `photo_content_scan.photo_hash` 同一哈希口径（`content.rs::photo_hash`） |
| `src-tauri/src/db/mod.rs` | `pub mod embedding;` 导出 |
| `src-tauri/src/vision.rs` | `embed_images_batch(paths,&cancel)`：POST /embed_batch，复用 ensure_service_ready/http_client/分批事件；`embed_text(query)` |
| `src-tauri/src/model_dl.rs` | DlSpec 注册 `clip_fp16 / clip_tokenizer`（hf-mirror 直链：`onnx/model_fp16.onnx` 377MB + `tokenizer.json`/`vocab.txt`/`preprocessor_config.json`，复用「官方+镜像并行、进度事件、原子改名」全部现有机制）→ ModelGpuSettings.vue 下载 UI **零改动自动出现**；**不含 int8** |
| `src-tauri/src/content.rs` | `scan_album_combined`：白名单加 `"semantic"`；`do_semantic` 分支（与 do_ai 同为异步 HTTP，每批查取消标记）：① 收集 user+album 照片 → 缩略图路径（查 `photo_thumb_cache`，缺失调缩略图生成补齐）② 差集增量（跳过已有 hash，进度 total=新增数）③ batch_size 批推理 → 500/批事务 upsert ④ emit `content-scan-progress` 加 `stage:"semantic"` |
| 级联删除 ×3 | `db/content.rs:1222 delete_content_by_paths`、`db/mod.rs:1142 delete_album_refs`、`db/mod.rs:1262 move_photo_content_path` 各加一行 `photo_embeddings` 同步删除/更新（按 photo_hash） |
| `src-tauri/src/lib.rs` | 注册变更沿用现有命令壳（无新命令，semantic 并入 combined） |

## 4. Phase 3 · smart_search 融合（RRF）

`content.rs::smart_search` 增加内部流程（签名不变，`SmartHit` 加可选 `semantic_score: Option<f32>`）：

1. keyword 为空 → 现状直返（纯筛选，不碰语义）
2. 语义可用：`/embed_text(keyword)` → 内存缓存全量向量（`AppState` 加 `Mutex<HashMap<user_id, {map, version}>>`；version=行数+max(generated_at)，变更才重载）
3. **置信度范围召回**：余弦 ≥ `min_similarity`（新可选参数，默认 0.30；实测相关命中集中 0.41~0.46，非相关背景值更低）的全部照片进入候选，**非固定 top-N 截断**；硬上限 2000 张保护极端大库（按相似度降序取前 2000）→ 现有筛选 SQL 以 `photo_hash IN (...)` 执行同一套 date/location/category/label/person/tone 过滤 → 语义候选列表（带余弦分）
4. RRF 融合：`score = Σ 1/(60+rank)`（FTS 榜 + 语义榜）排序；阈值以上未入融合榜的语义命中也**追加返回**（按余弦降序排在融合榜之后，保证“不止 top-N”）；`semantic_score` = 余弦原值（前端徽标用）
5. **降级链**：服务未启动/模型缺失/向量库空/编码失败 → 跳过 2~4，返回纯 FTS，不报错不提示（前端无感）

不引入 sqlite-vec（实测暴力余弦 10 万张 <5ms）；预留升级路径。

## 5. Phase 4 · 前端（原子复用清单）

| 文件 | 改动 | 复用 |
|---|---|---|
| `components/GlobalScanPanel.vue` | 扫描项组加 1 个 checkbox `value="semantic"`（文案建议「语义向量」） | `gs-check` 样式/禁用态/`scanTypes` v-model 原样 |
| `components/ScanPanel.vue` | 同上加 1 个 checkbox | `combo-check` 样式原样 |
| `stores/content.ts` | 无结构改动：`scanTypes` 数组透传；进度 job 文案映射加 `semantic`；`modelDownloads` 自动收到新模型进度 | 现有 `combinedJobs` 单例/`model-dl-progress` 监听 |
| `views/SmartSearch.vue` | ① 搜索框 placeholder →「关键词 / 标签 / 人物，或自然语言：海边日落、一只猫…」② 结果卡右上角语义徽标：`semantic_score` 存在时显示「AI 匹配 92%」③ 建索引引导：向量覆盖 0 且搜索无语义结果时，卡片式提示「启用语义搜索 → 去扫描中心勾选“语义向量”」+ 跳转按钮 | 结果卡片/筛选器/`openAlbum`/`showTag` 全部不动；徽标复用 `tone-badge` 样式模式（3 行 span，不新建组件）；提示卡复用 CollapseSection/现有空态样式 |
| `components/ModelGpuSettings.vue` | **零改动**（模型清单/下载进度/GPU 检测由后端注册自动带出） | — |

新增文件仅 2 个：`db/embedding.rs`、`vcr/services/embed_service.py`（+2 个接口函数）。前端 0 个新组件。

## 6. 资源预算（实测校准）

- 向量 512×f32=2KB/张：5 万张 ≈ +100MB（photos.db）；内存缓存同量级（fp32，1.1 万张实测 23MB）
- 首次全量索引：DML ≈ 41min / 5 万张（batch=8 实测 37.9ms/张）；增量秒级
- 查询：文本编码 ~7ms + 全量阈值召回（1.1 万张 0.36ms）+ RRF ~0ms ≈ **<15ms**（10s 预算余量 3 个数量级）
- 模型盘上 377MB（fp16）+ 词表 ~2MB，**无 int8**

## 7. 验收清单

1. `/health` 返回 `clip_ready:true`，fp16 加载成功（DML 优先，CPU 兜底自动生效）
2. 扫描中心勾选「语义向量」→ 进度实时上报 → `photo_embeddings` 覆盖数与缩略图数一致；重复扫描幂等（跳过已有）
3. `smart_search("海边日落")` 返回纯 FTS 搜不到的 other 类照片，且 `semantic_score` 降序合理；日期/地点/人物筛选器对语义命中同样生效
4. 降级：未下载模型/服务未启动时 `smart_search` 正常返回 FTS 结果，无报错无卡顿
5. 删除照片 / 删除相册 / 移动照片后 `photo_embeddings` 无孤儿行（count 对账）
6. `cargo check` + `vue-tsc --noEmit` 通过；打包版 PyInstaller 启动含 clip（tokenizers hiddenimports）

## 8. 风险与对策

| 风险 | 对策 |
|---|---|
| Xenova 单文件双塔 graph IO 不符 | spike 先行；`onnx.utils.extract_model` 拆图兜底（自导出脚本已验证可行，双保险） |
| CPU EP 跑 fp16 内部 upcast 变慢 | 可接受（索引是一次性后台任务）；不提供 int8 档，保证单一精度口径 |
| 2 线程限制拖慢 text 编码（实测 2.5 倍差） | `model_registry` clip 槽位不设 intra_op 限制 |
| RRF 融合后 FTS 强命中被稀释 | 语义榜只取 top-200 且 RRF k=60 偏保守；后续可按反馈调权重，参数集中在 Phase 3 常量 |
| 缩略图缺失导致索引不全 | 扫描分支内现场补生成（复用缩略图引擎），失败照片计入 failed 并跳过 |
