# FEAT-137 · 语义模型收敛为单档（B/16 fp32 权重）

> **状态**：`done`（2026-11-02）· 两档 → 一档，旧档位选择与旧向量标签均自动迁移
> **触发**：用户提出「b16（fp16，默认）这一档可以去掉，核显加速 fp32 比 fp16 CPU 快得多」
> **前序**：`docs/FEAT-136-加速分配与可选组件.md`（CLIP 设备由全局 provider 决定）

---

## 一、结论

**fp16 档是三输，删掉。** 实测（真实 256px 缩略图，批 8 / 批 32，无 HTTP 无并发）：

| 组合 | 批 8 | 批 32 | 说明 |
|---|---|---|---|
| **fp16 / CPU**（旧默认档） | 74.4 ms/张 | 113.7 ms/张 | x86 上 ORT 跑 fp16 权重必须反复 cast，纯亏 |
| **fp32 / CPU** | 60.6 ms/张 | **76.4 ms/张** | 比 fp16 快 **18%~33%** |
| **fp32 / DML** | 61.3 ms/张 | 63.7 ms/张 | 唯一能上 GPU 的那一档 |

三项事实同时成立：

1. **慢**：fp16 在 CPU 上反而更慢（复跑守卫脚本：79.7 vs 65.9 ms/张，1.21×）。
2. **用不上 GPU**：fp16 权重在 DirectML 上有算子级数值 bug（BUG-2026-0910-006，
   会静默产出错向量），所以旧代码里把它硬钉在 CPU —— 它连「换个设备」这条路都没有。
3. **精度等价**：同批 32 张图 **cos 最小 0.99999928**、逐元素最大差 0.000316。
   同一套权重，只是导出精度不同 —— 这也是「旧向量标签可以直接重打」的唯一依据。

> 第三点很关键：如果两档不等价，收敛就必须让全库重编码（1 万张 ≈ 13 分钟，
> 且期间语义搜索会变空）。等价才有「改标签不改数据」这条路。

## 二、迁移（用户零重建）

三件事都是**自动**的，不需要用户操作：

| 旧状态 | 新行为 |
|---|---|
| `current_clip.json` 写着 `b16-fp32` / `b16-fp16` | `config.CLIP_RETIRED_TIERS` 映射到唯一档 `b16`，不报错 |
| `photo_embeddings.model` / `photo_text_embeddings.model` 是 `...-b16-fp16` 或 `...-b16-fp32` | 宿主启动时 `restamp_embedding_models()` 一律重打成 `chinese-clip-vit-b16`（幂等，`UPDATE OR REPLACE` 处理主键冲突） |
| 老布局 `python/models/chinese-clip/` 里已有 fp32 整图/拆分件 | `clip_paths()` 自动回退到老目录，不让人白下 719MB |

**为什么必须重打标签**：所有向量查询都是 `WHERE model = ?`（`category.rs:362`、
`embedding.rs::lookup_embedding_hashes`）。不迁移的话，升级后一条也命中不了 →
语义搜索/分类变成「尚未建立索引」，不报错，只是搜不到 —— 这类静默失效最难查。

**新 id 为什么是中性的 `chinese-clip-vit-b16`**：不再带精度后缀。两个旧标签各自
重打过来，语义上「一套权重 = 一个空间」，以后换精度/量化就换 id（那次该重建）。

## 三、代码落点

| 文件 | 改动 |
|---|---|
| `python/vcr/config.py` | `CLIP_MODEL_META` 只留 `b16`（dir `chinese-clip-fp32` / onnx `model.onnx` / id `chinese-clip-vit-b16`）；新增 `CLIP_RETIRED_TIERS` / `CLIP_RETIRED_IDS` / `CLIP_LEGACY_DIRS`；`clip_paths()` 老目录回退；`active_clip()` 映射旧名 |
| `python/vcr/model_registry.py` | `_clip_providers()` 删掉「fp16 硬钉 CPU」特例 —— 雷源没了，不再需要守 |
| `src-tauri/src/model_dl.rs` | 删 `chinese-clip`(fp16) 下载规格与 `CLIP_FP16` 文件表；`chinese-clip-fp32` 改标签「语义模型 · B/16」；新增单测 `clip_model_ids_match_python` 钉住三个 id 字符串 |
| `src-tauri/src/db/embedding.rs` | `restamp_embedding_models()` + `CLIP_MODEL_ID` / `CLIP_OLD_MODEL_IDS` 常量 + 单测（含幂等） |
| `src-tauri/src/db/mod.rs` | 建表后调用迁移；失败只 warn，不阻断启动 |
| `src/components/settings/ClipTierSection.vue` | 从「下拉 + 换档确认弹窗」改写为**一行状态**（模型 / 双塔体积 / 就绪 / 刷新） |
| `src/components/ModelGpuSettings.vue` | 「语义索引」摘要改为读实际 provider（不再按档位猜） |
| `python/bench/verify_clip_tier_equiv.py` | **新增守卫脚本**：速度（fp16 必须慢于 fp32）+ 精度（cos ≥ 0.9999）+ 设备；不通过退出码 1 |
| `python/bench/verify_clip_tiers.py` | 删除（两档比对已无意义；历史口径见 git 历史） |
| `python/download_models.py` | CLI 任务表随之只剩一档 |

## 四、验收

| 项 | 结果 |
|---|---|
| `cargo test --lib` | **427 passed**, 0 failed（含新增的 restamp 与 id 一致性单测） |
| `vue-tsc --noEmit` | 0 error |
| 老用户升级路径（`current_clip.json` = `b16-fp32`） | `/health.model_id=chinese-clip-vit-b16`、`/models` 只列一档、`/embed_batch` 4/4 出 512 维向量 |
| 守卫脚本 | 通过：fp16 79.7 vs fp32 65.9 ms/张（1.21×）；cos 最小 0.99999928 |

## 五、残留风险与代价

| # | 项 | 说明 |
|---|---|---|
| R1 | **新用户下载 360MB → 719MB** | 唯一实质代价。想压回来的正路是 NNCF int8 量化（~180MB，且 CPU/iGPU/NPU 三端都快），属另一件事 |
| R2 | 老用户盘上 `chinese-clip/` 的 fp16 文件成了垃圾（~720MB） | 工程上无引用；目前要手动删。若需要可加「清理旧模型」按钮 |
| R3 | 重打标签让 fp16 时代的老向量与新向量混在同一空间（cos ≥ 0.9999993） | 影响面是「近乎并列的余弦档位」，实测排序无变化；守卫脚本把门槛钉在 0.9999 |
| R4 | 旧服务（api < 13）不认新 id | `VCR_API_VERSION` 语义未变，宿主会按版本重启服务；升级包内两者同版本 |
