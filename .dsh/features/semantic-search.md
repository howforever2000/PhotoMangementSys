# 语义搜索（semantic-search）

> Chinese-CLIP 双塔 512 维向量驱动的语义搜索与语义分类；以图搜图 + 描述向量三路 RRF；重复照片清理（内容哈希 + CLIP 相似）。功能清单见 `features/index.md`。

## 核心能力（功能 ID）
- AI 内容入库 + 群相册/单相册搜索（FEAT-022）、内容分类 tab 两级浏览 + 中文映射（FEAT-048）
- 语义搜索 Chinese-CLIP 双塔 512 维（扫描集成 + 智能搜索 RRF 融合 + 覆盖/增量扫描，FEAT-055）
- 语义分类 v5：下线图像分类模型，改由 CLIP 关键词语义匹配驱动用户自建分类（FEAT-056）
- 多模态检索 · 以图搜图 + 描述向量（三路 RRF）+ 人物真名嵌入（FEAT-067）
- 语义模型收敛为单档 B/16 fp32（删 fp16，旧选择与旧向量标签自动迁移，FEAT-137）
- 智能搜索语义路提速（一次编码两路共用 / 向量与描述索引缓存 / 连续 f32 + rayon 并行点积 / 两路并集一次回表 / 缩略图单请求并行，FEAT-154）
- 智能搜索匹配度重做（分路相对相关度 + 同文本折叠 + 按 query 类型调权 + 徽标标来源 + 离线评测台，FEAT-155）
- 重复照片清理：内容哈希精确重复 + CLIP 向量相似分组 + 两档删除（FEAT-143）、重复扫描六段台账日志（FEAT-153）

## 入口文件
- 视图：`src/views/SmartSearch.vue`、`src/views/ScanHub.vue`（重复清理 tab）、`src/views/SmartAlbum.vue`（分类 tab）
- 前端：`src/stores/content.ts`、`src/stores/dedupe.ts`、`src/utils/matchScore.ts`、`src/components/DedupePanel.vue`、`src/components/dedupe/DupeGroupCard.vue`、`src/types/dedupe.ts`
- Rust：`src-tauri/src/vecindex.rs`、`content.rs`、`textdesc.rs`、`dedupe.rs`、`db/embedding.rs`、`db/category.rs`、`db/content.rs`、`thumbnail.rs`
- Python 评测：`python/bench/eval_search_quality.py`、`python/bench/verify_rust_vs_sim.py`、`python/bench/verify_clip_tier_equiv.py`；离线检索评测台 `.pi/search_eval`

## 关键约束与历史坑
- **向量 `model` 标签读写必须一致**：宁可失败不静默写错标；失配要显性化（`index_model_mismatch`）并给重建入口。详见 `knowledge/scan-pipeline.md`（BUG-2026-0921-004）。
- **跨路余弦不能共用绝对阈值**：两路余弦尺度不同，`0.45` 卡图像塔只剩 0~2 条、描述塔 99% 都过 → 整页被同分模板描述占满。改为「分路相对相关度 + 减各自基线」，代码不允许再出现跨路绝对阈值。来源：`bug-notes.json` BUG-2026-1101-007（FEAT-155 已落地）。
- **换档必须换标识**：`config.py` 的 `CLIP_MODEL_META` / 退休标签迁移（`restamp_embedding_models`）要同步，否则旧向量检索不到。来源：`bug-notes.json` BUG-2026-0921-004 + FEAT-137。
- **fp16 CLIP 在 AMD DirectML 上有数值 bug**（余弦 0.43），语义双塔固定 CPU 或走 fp32。详见 `knowledge/gpu-dml-constraints.md`。
- **CLIP 模型缺失会 1s/2s/4s 反复重试白等 7s**。来源：`bug-notes.json` BUG-2026-1026-001。
- 重复组跨父子相册同一照片被算两条（BUG-2026-1101-003）、整页卡片+缩略图一起入队卡顿（BUG-2026-1101-004）、同文件夹 3 份相同照片与「保留」张地址相同（BUG-2026-1101-005）。
