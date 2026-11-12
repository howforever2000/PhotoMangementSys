# 项目概览（overview）

## 定位
- **本地优先**的桌面相册管理应用：「相册」是**本地文件夹的抽象**，不搬移/不改名用户文件。
- **离线优先**：AI 识别与检索在本地完成，不依赖云；检索优先于 AI（能先用结构化条件就别上模型）。
- 系统名（用户可见）：**本地相册搭子**；应用标识符 `com.photomanagementsys.app`（FEAT-102 改中性名 + 老数据目录一次性迁移）。
- 当前版本 **v0.4.5**（2026-11-01）。

## 功能特性
| 域 | 能力 |
|---|---|
| 相册管理 | 相册 CRUD、多级分组文件夹树、标签、自动封面与统计、批量导入/删除、手动排序、撤销/最近删除、面包屑 |
| 多用户 | 本地多用户隔离；密码 **Argon2id** 哈希；登录/注册/忘记密码（仅账户名+邮箱，FEAT-160 去手机号） |
| 智能分类 | 9 大类语义分类（FEAT-056 起由 CLIP 关键词语义匹配驱动用户自建分类） |
| 人物识别 | SCRFD 检测 + ArcFace 识别聚类；头像/封面、人工改挂、质量闸门、模型档位 |
| 时间线 / 回忆 | 按月分页 + 窗口化渲染 + 跳转；月度浏览；那年今日 |
| 智能搜索 | EXIF + 影调 + AI 内容 + Chinese-CLIP 语义（图像塔/描述塔/以图搜图三路 RRF）；匹配度分路相对相关度 |
| 创意工坊 | 图像加工工具台（压缩等）+ AI 创作（手账/美颜/风格迁移/画质增强） |
| 数据安全 | 库内加密 **AES-256-GCM**；归档/隐藏虚拟相册密码解锁；SQLite 本地 |
| 离线能力 | 模型本地加载、GPS 离线反查、人物库 Rust 直读 |
| 模型管理 | 应用内下载（多镜像/断点续传）、GPU/NPU 加速切换、可选 OpenVINO 运行时 |

## 技术栈
- 前端：Vue 3（`<script setup>`）+ TypeScript + Vite 6 + Pinia + Vue Router 4
- 桌面壳 / 后端：Tauri 2.x（Rust）；`rusqlite` / `image` / `jpeg-decoder` / `kamadak-exif` / `walkdir` / `argon2` / `aes-gcm` / `reqwest` / `tokio`；SQLite 持久化
- AI 微服务：Python + FastAPI + uvicorn + onnxruntime（DirectML / OpenVINO 可选）+ Pillow + numpy + opencv
- 模型：YOLOv8s/n-cls（分类）、YOLOv8n-det（人物检测）、SCRFD det_10g（人脸检测）、ArcFace w600k_r50（人脸识别）、PaddleOCR（OCR）、Chinese-CLIP B/16（语义）

## 目录结构（顶层）
```
src/            Vue 前端（views 15 / components 子目录 dedupe·health·settings·ui·workshop / stores 13 / composables / utils 37 / types 13 / router）
src-tauri/      Rust 主进程（src/ 命令层与业务；db/ SQLite 模块；capabilities/ 窗口权限；icons/）
python/         vcr/（视觉识别微服务）、ai_studio/（AI 创作服务）、bench/（评测/探针）、scripts/（构建）
docs/           FEAT-* 技术文档、扫描入库优化清单、推理后端多平台化 B 方案
design/         设计系统（design-system.md）+ preview 效果图
.pi/            规划与探针（plan、probes、search_eval）
covers/         启动封面壁纸
feature-notes.json / bug-notes.json   唯一事实源
release.ps1     发布脚本；python/build_vcr_exe.ps1  打包 VCR exe
```

## 运行与构建
- 开发：`npm run dev`（前端，端口 8765）；`npm run tauri dev`（桌面壳）
- 构建前端：`npm run build`（`vue-tsc --noEmit && vite build`）
- 发布：`release.ps1`；VCR 打包 `python/build_vcr_exe.ps1`；依赖清单 `python/requirements-vcr-packaging.txt`（**必须纯 ASCII**）
- 端口：开发固定 8765；打包版动态 **18765~18865**（选可用端口，PID/端口落盘、启动时收养验 PID 正身）

## 关键文档索引
- 架构细节 → `architecture.md`
- 功能全量索引 → `features/index.md`；分域入口与坑 → `features/*.md`
- 高频避坑 → `knowledge/*.md`
- 项目自述 → 仓库根 `README.md`、`README.en.md`；设计系统 → `design/design-system.md`
