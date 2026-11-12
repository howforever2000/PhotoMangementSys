# 相册管理（album-management）

> 相册 = 本地文件夹抽象；分组为多级文件夹树。所属模块：相册管理 / 时间线 / 批量操作 / 删除体系 / 归档与隐藏。功能清单见 `features/index.md`。

## 核心能力（功能 ID）
- 相册 CRUD + SQLite 持久化（FEAT-001）；自动封面与统计（FEAT-002）、统计缓存 v2「文件数变更探测」替代 TTL（FEAT-015）
- 时间分组排序视图（FEAT-003）、地点排序（手动标签，FEAT-004）
- 批量导入（FEAT-005）、勾选批量删除（FEAT-006）、照片多选删除（本地文件删除+二次确认，FEAT-029）
- 手动排序（分组文件夹树 / 拖拽，FEAT-007）、详情页父相册定位（FEAT-009）、面包屑（FEAT-041）
- 相册标签（FEAT-010）、右键菜单（FEAT-011 / 照片 FEAT-043）
- 数据模型收敛（`folder_id` 单源）+ 迁移回填（FEAT-013）
- 前端组件拆分 AlbumCard / AlbumMiniCard（FEAT-016）
- 撤销机制 Toast + 最近删除（FEAT-039）、骨架屏+固定宽高比防 CLS（FEAT-040）、搜索 debounce/历史/高亮（FEAT-042）
- 照片预览器改造 + 归档/隐藏虚拟相册（密码解锁）+ 缩略图矩阵缩放（FEAT-095）
- 照片「处理方式」四分化：移除 / 改挂 / 清除数据库记录 / 删除文件（FEAT-131）
- 入库体检（FEAT-152 第二条：只读诊断覆盖率/缩略图/向量/失效/缓存体积）

## 入口文件（先看这里再改）
- 视图：`src/views/AlbumList.vue`、`src/views/AlbumDetail.vue`、`src/views/Home.vue`、`src/views/Timeline.vue`、`src/views/Memories.vue`
- 状态：`src/stores/album.ts`、`src/stores/vault.ts`
- 组件：`src/components/AlbumCard.vue`、`AlbumMiniCard.vue`、`PhotoGrid.vue`、`PhotoLightbox.vue`、`PhotoActionMenu.vue`、`PhotoDeleteDialog.vue`、`ContextMenu.vue`、`SelectAllCheckbox.vue`、`CollapseSection.vue`、`AlbumHealthPanel.vue`
- 后端命令层（薄壳）：`src-tauri/src/album.rs`、`photos.rs`、`photo_mark.rs`、`db/content.rs`、`db/mod.rs`、`auth.rs`、`lib.rs`
- 数据表：`albums/folders/folder_tags/folder_albums/album_tags/album_stats/album_photo_excluded/photo_ratings/album_merged_sources/photo_marks`（`db/mod.rs`）

## 关键约束与历史坑
- **拖拽 vs 点击**：用 Pointer Events + 移动阈值 `DRAG_THRESHOLD=5px` 区分；`pointerup` 先清状态再判断，`cleanupDrag` 完整重置。否则点击被误判为拖拽 → `move_album(id, null)` 把相册挪到根。WebView2 别用 HTML5 DnD（`dataTransfer.getData` 常空、动作不稳）。浮层 `pointer-events:none`。来源：`bug-notes.json` BUG-2026-0801-001、BUG-2026-0801-002。
- **全选取消不掉**：用三态组件 `SelectAllCheckbox`（空圈→横杠→打勾），子组件只 emit「点击意图」，父组件只执行、不再二次判断。来源：`bug-notes.json` BUG-2026-1007-002。
- **右键菜单点了没反应**：遮罩 `@click.stop` 会掐断 document 关闭分支。详见 `knowledge/frontend-style-pitfalls.md`（BUG-2026-1007-001）。
- **改标识符后封面全碎**：迁移要重写库内绝对路径（`albums.cover_path` 等）。详见 `knowledge/data-path-and-packaging.md`（BUG-2026-1021-005）。
- **相册瓦片封面高度塌成 0px**：`aspect-ratio` + flex 收缩 + `<img>` 循环依赖。来源：`bug-notes.json` BUG-2026-1023-006。
- **封面卡顿**：`image::open` 全尺寸解码大图 7.7s → 用降采样解码。来源：`bug-notes.json` BUG-2026-0815-003（配合 `knowledge/thumbnail-cache.md`）。
