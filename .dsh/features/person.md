# 人物识别（person）

> 人脸检测（SCRFD det_10g）+ 识别（ArcFace w600k_r50）+ 聚类；独立库 `persons.db`（`persons` / `faces`）。人物页头像 / 封面、人工改挂、质量闸门、模型档位。功能清单见 `features/index.md`。

## 核心能力（功能 ID）
- 缩略图人物命名 + 人脸头像角标（FEAT-028）、智慧相册人物注册表 Rust 直读离线可用（FEAT-031）、删人物离线化 + 主题适配（FEAT-032）
- 用户头像（FEAT-045）、回忆·近期人物真实头像 + 跳转（FEAT-046）、人物自选头像作封面（FEAT-047）
- 人脸模型换档 det_10g + w600k_r50（FEAT-088）、封面身份校验 + 右键换头像 + 重建人物库（FEAT-089）、人脸通道预检大声失败 + 重建后增量重做人脸队列（FEAT-090）、人脸模型档位可切换 + 持久化（FEAT-091）、人脸质量闸门（FEAT-092）
- 相册详情页移除人物注册表（职责归位智慧相册，FEAT-083）
- 人物页照片裁切封面 + 文字嵌入（4:5 人脸裁切，FEAT-124）、人像头像+封面统一写入与时间线缩略图自愈（FEAT-130）
- 人脸归属人工改挂 + `face_overrides` 覆盖表（重扫不覆盖人工修正，FEAT-132）

## 入口文件
- 前端：`src/components/PersonGallery.vue`、`src/views/SmartAlbum.vue`（人物 tab）、`src/components/PhotoGrid.vue`
- Rust：`src-tauri/src/persons.rs`、`avatar.rs`、`db/content.rs`（`face_rebuild_queue`）、`lib.rs`
- Python：`python/vcr/services/face_service.py`、`python/vcr/persistence/person_store.py`、`python/vcr/services/pipeline.py`、`arbitrator.py`
- 数据表：`persons.db` 的 `persons`/`faces`（`persons.rs`）；`face_rebuild_queue`（`db/content.rs`）；`face_overrides`（`persons.rs`）

## 关键约束与历史坑
- **头像缓存只判「文件存在」→ P 编号复用后旧文件冒充本人**：人物封面显示别人。来源：`bug-notes.json` BUG-20261001-013。
- **代表脸原图不在磁盘 → 头像/封面永远裁不出**：选代表脸时要校验原图存活。来源：`bug-notes.json` BUG-2026-1027-005。
- **`get_person_avatar` 报「无法读取图片头信息」/ 缩略图 `Illegal start bytes:8950`**：解码入口不统一。来源：`bug-notes.json` BUG-2026-1027-004。
- **「设置头像」无效**：改了 96×96 头像但人物卡画的是 4:5 封面（两张不是同一产物）。来源：`bug-notes.json` BUG-2026-1028-013。
- **EXIF Orientation=6 竖持照片人脸判「不是人物」**：`preprocess.open_image` 单点 `exif_transpose` + `persons.rs` 按显示坐标裁 96×96 + `THUMB_RENDER_VERSION` 掺版本（否则老用户命中躺倒旧缓存）。来源：`bug-notes.json` BUG-2026-1014-002。
- **换人脸/语义模型要重建**：换档会静默清空历史 `person_ids`/旧向量，需重建人物库/向量。来源：`bug-notes.json` BUG-2026-0921-004 + FEAT-090/137。
- **人物画廊头像风暴**：每开一次画廊 ~330 次顺序 IPC + 330 次全渲染 → 批量取数、限流。来源：`bug-notes.json` BUG-2026-0910-002（同类：`get_person_photos` 同步命令主线程生成 884 张缩略图 BUG-2026-0910-003）。
