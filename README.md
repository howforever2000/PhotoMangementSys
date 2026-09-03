# PhotoManagementSys — 本地相册管理系统

> 本地优先（Local-First）的桌面相册管理应用。内置 AI 视觉识别微服务，实现照片的自动分类、人物识别、场景识别与文字识别。全部数据与推理过程均在本机完成，全程可离线运行。

**English:** [README.en.md](README.en.md)

[![License](https://img.shields.io/badge/license-私有-1f6feb.svg)]()
[![Platform](https://img.shields.io/badge/platform-Windows%20(x64)-1f6feb.svg)]()

---

## 一、项目定位

PhotoManagementSys 是一款面向个人照片库的桌面应用，其核心定位如下：

- **本地存储**：照片与全部管理数据存放于本机，不向任何服务器上传。
- **离线 AI**：内置视觉内容识别（VCR）微服务，基于 ONNX Runtime 在本机完成分类、检测、人脸识别与 OCR 推理。
- **文件夹抽象**：相册是对本地文件夹的抽象。既可将已有图片文件夹直接导入为相册，也可新建相册后再绑定文件夹；相册改名、移动、归类均不影响原始文件。
- **检索优先**：提供时间线、智能搜索、条件过滤、人物总览与回忆视图，支持在数秒内定位数月前的目标照片。

与云相册（需上传、依赖网络、按量付费）不同，该系统以本地存储与离线推理为基本前提，照片与隐私数据不出本机。

---

## 二、功能特性

| 模块 | 说明 |
| --- | --- |
| 相册管理 | 创建 / 编辑 / 删除 / 重命名 / 封面 / 标签 / 地点 / 说明；支持导入本地文件夹 |
| 多用户隔离 | 同机多账户；相册、分组、标签、搜索与排序按登录用户隔离；密码采用 Argon2id 加盐哈希 |
| 智能分类 | 照片自动归入 9 大类；动物细分（狗 / 猫 / 鸟）；支持文档 OCR、场景识别与夜景判定 |
| 人物识别 | 人脸检测与特征提取，自动聚集同一人物；支持改名、合并、删除；自动生成人物头像 |
| 时间线 / 回忆 | 跨相册照片时间线；人物、地点、节日的聚合回忆视图 |
| 智能搜索 | 标题 / 标签 / 地点 / 内容组合搜索；支持 EXIF + 影调 + AI 三合一条件过滤 |
| 手动整理 | 分组（文件夹）树；批量移动 / 归类；手动排序；编辑说明 |
| 批量操作 | 批量设地点 / 标签、批量移动到相册、批量导出到文件夹 |
| 数据安全 | 邮箱 / 手机号 / 密码字段以 AES-256-GCM 加密落库；支持记住登录（3 天免密） |
| 离线能力 | 缩略图、人物头像、GPS→省市地理反查均本地完成，不依赖网络 |

---

## 三、系统架构

系统由三层独立进程与一个独立微服务组成，各层职责明确、按契约协作：

```
┌─────────────────────────────────────────────────────────────┐
│  Frontend  Vue 3 + TypeScript + Vite + Pinia + Vue Router     │
│  （渲染进程，WebView；负责界面渲染、状态管理与后端命令调用）      │
└───────────────┬─────────────────────────────────────────────┘
                │  Tauri IPC（invoke 命令 / 事件）
                ▼
┌─────────────────────────────────────────────────────────────┐
│  Backend  Rust（Tauri 主进程）                                 │
│  命令层 #[tauri::command]  · 业务层 db/photo_scan/thumbnail/    │
│  tone/content/persons/auth/geo_index · 持久层 SQLite            │
└───────────────┬─────────────────────────────────────────────┘
                │  HTTP（REST：HTTP 客户端 ← → 127.0.0.1:8765）
                ▼
┌─────────────────────────────────────────────────────────────┐
│  Microservice  Python VCR（视觉内容识别，独立进程，打包为 exe）  │
│  FastAPI + ONNX Runtime，分层：接口 / 服务 / 持久化 / 基础设施    │
│  模型：YOLOv8 分类/检测 · SCRFD 人脸 · ArcFace 识别 · PaddleOCR │
└─────────────────────────────────────────────────────────────┘
```

各层职责划分如下：

- **Rust 后端**负责本地 I/O 与安全相关工作：文件遍历、缩略图、EXIF、SQLite、密码学。
- **Python 微服务**负责 AI 推理：机器学习生态集中于 Python/ONNX，以独立进程承载，避免向 Rust 引入重型推理栈。
- **Vue 前端**负责界面交互：以组件化与组合式 API 实现快速迭代。

### 技术栈

| 层 | 技术 |
| --- | --- |
| 桌面壳 | Tauri 2.x（Rust） |
| 前端 | Vue 3 `<script setup>` + TypeScript + Vite 6 + Pinia + Vue Router 4 |
| Rust 后端 | rusqlite(SQLite 内置) / image / jpeg-decoder / kamadak-exif / walkdir / argon2 / aes-gcm / reqwest / tokio |
| Python 微服务 | FastAPI + uvicorn + onnxruntime + Pillow + numpy + opencv-python |
| AI 模型 | YOLOv8s/n-cls（分类）、YOLOv8n-det（COCO 检测）、SCRFD（人脸框）、ArcFace（人像向量）、PaddleOCR（文字） |

---

## 四、模块设计

### 4.1 解耦原则

系统在架构层面明确了模块边界，各模块可独立演进、替换或剥离：

1. **进程级解耦**：Rust 后端与 Python 微服务通过固定 REST 契约通信（`POST /classify_batch`、`GET /health` 等），端口固定为 `127.0.0.1:8765`。两侧可独立开发、测试并单独打包。
2. **客户端保持薄壳**：`vision.rs` 为轻量 HTTP 客户端与生命周期管理器，不依赖 `db` / `thumbnail` / `tone` 模块（图片扩展名列表本地复制，防止隐式耦合）；服务不可用或模型缺失时返回明确错误，不影响其他功能。
3. **命令层薄壳**：`lib.rs` 中的 `#[tauri::command]` 仅做参数透传、日志与状态注入，业务逻辑位于 `db` / `content` / `persons` 等模块；`tauri::State` 承担依赖注入职责。
4. **配置集中管理**：Python 端的路径、阈值、模型清单与 GPU 策略集中于 `python/vcr/config.py`，服务层仅引用配置常量。
5. **能力自动降级**：可选模型（场景 / 花朵 / 美食 / OCR）缺失时对应通道自动降级，主分类通道不受影响。

### 4.2 VCR 微服务

VCR（Visual Content Recognition）为独立 Python 服务，将照片交由多路模型推理并仲裁出最终分类，采用经典分层架构：

```
python/
├─ server.py                  # 接口层：FastAPI 路由 + DTO（薄壳，无业务逻辑）
├─ vcr/
│  ├─ config.py               # 基础设施：路径 / 阈值 / 模型清单（集中配置）
│  ├─ model_registry.py       # 基础设施：模型注册表，惰性加载 + GPU 提供方选择
│  ├─ preprocess.py           # 基础设施：图像预处理（缩放 / 归一化）
│  ├─ mapping.py              # 基础设施：ImageNet → 9 大类映射
│  ├─ taxonomy.py             # 基础设施：taxonomy 折叠（保证输出 ∈ 9 类）
│  ├─ schemas.py              # 接口层：Pydantic DTO
│  ├─ persistence/
│  │  └─ person_store.py      # 持久层：SQLite 人物注册表
│  └─ services/               # 服务层：业务编排
│     ├─ classifier.py        #   分类通道（YOLOv8-cls）
│     ├─ detector.py          #   检测通道（YOLOv8-det，人 / 车 / 物）
│     ├─ face_service.py      #   人脸通道（SCRFD + ArcFace）
│     ├─ scene_service.py     #   场景通道（Places365，可选）
│     ├─ ocr_service.py       #   文档通道（PaddleOCR，可选）
│     ├─ flower/food_service  #   专家通道（懒加载，可选）
│     ├─ tone_service.py      #   影调通道（夜景 / 低调判定）
│     ├─ arbitrator.py        #   仲裁器：多通道结果 → 单一结论
│     └─ pipeline.py          #   流水线编排：单张图多路推理 → 仲裁 → 结果
```

关键设计：

- **一次解码，多路分发**：`pipeline.py` 对单张图片仅解码一次，再将张量分发给各专家通道，避免重复 IO。
- **条件触发专家通道**：花朵 / 美食等专家模型为懒加载，仅在判定命中触发条件时加载。
- **仲裁器**：分类、检测、场景、影调、OCR、专家各通道给出候选，仲裁器按阈值与优先级合成唯一结论，保证输出类别属于 9 大类。
- **自动降级**：模型缺失时对应通道自动降级，`/health` 返回真实就绪状态。
- **GPU 可选**：默认自动检测 onnxruntime GPU 提供方（DirectML / CUDA），有则用 GPU，无则回退 CPU；可通过 `VCR_PROVIDER=cpu` 强制 CPU。

### 4.3 前端组件复用

前端以组件化与组合式 API 实现界面复用：

- **通用基础组件**：`Toast` / `ToastContainer`（全局通知）、`ConfirmDialog`（确认框）、`CollapseSection`（折叠面板）、`PhotoGrid`（照片网格）、`PhotoLightbox`（灯箱）、`AlbumCard` / `AlbumMiniCard`（相册卡片），在首页、相册列表、时间线、回忆等多个视图复用。
- **复杂视图拆分**：`AlbumDetail.vue` 由 2709 行重构至 183 行，拆分为 `AlbumMeta`、`PhotoGrid`、`CollapseSection` 等子组件，各子组件职责单一，父组件仅做数据装配。
- **状态集中管理**：跨视图共享状态（album / content / toast / theme / auth）集中于 Pinia store，组件通过 store 读写，避免逐层 props 传递。
- **组合函数复用**：`useNotify.ts` 将通知触发逻辑封装为组合函数，任意组件可一行调用。
- **类型驱动开发**：`types/` 中定义 `Album` / `Photo` / `Content` 等类型，与后端 serde JSON 一一对应，契约清晰，字段变更可在编译期发现。

---

## 五、性能优化

开发过程中实施的典型优化如下：

- **缩略图 DCT 降采样解码**：全尺寸解码 6000×4000 巨图需 5~14 秒；改用 `jpeg-decoder` 的 `scale` 仅解码所需 DCT 块后，切换封面耗时由 7.7s 降至 0.76s（约 10 倍）。缩略图缓存写入 `app_data_dir/thumbs`，不写回数据库。
- **批量内容识别**：`/classify_batch` 单次最多处理 64 张（客户端默认 8），配合分批提交与 `classify-progress` 事件实时上报进度。
- **GPU / 批次加速**：识别服务自动探测 GPU（DirectML），可用时走 GPU；否则 CPU 多线程（`THREADS=4`）。
- **GPS→省市离线反查**：内嵌省市边界数据，以点面判断在离线状态下反查照片地理位置，替代逐张联网反编码，兼顾速度与隐私。
- **组合扫描三合一**：EXIF（拍摄时间）、影调（曝光 / 夜景）、AI（内容识别）在一次组合扫描中完成，减少重复遍历。
- **分页加载**：照片网格分页渲染，避免大相册一次性渲染造成卡顿。
- **多用户安全**：密码采用 Argon2id（抗 GPU 暴力破解）；邮箱 / 手机号 / 密码哈希以 AES-256-GCM 加密落库。

---

## 六、使用说明

### 6.1 安装

- **方式一（推荐）**：下载 Release 中的安装包并双击安装：
  - `PhotoManagementSys_<版本>_x64_en-US.msi`
  - 或 `PhotoManagementSys_<版本>_x64-setup.exe`（NSIS）
- 安装包默认内置 AI 模型（详见「七、构建与发布」）。若安装包不含模型，请按「模型放置说明」将模型文件放入模型目录后再启动。

### 6.2 首次使用

1. **注册账号**：以账户名 / 邮箱 / 手机号 + 密码注册（同机可多用户，相册相互隔离）。
2. **导入照片**：在「相册管理」页新建相册并选择本地图片文件夹，系统将扫描其中的图片（支持 jpg / jpeg / png / webp / gif / bmp）。
3. **启动 AI 识别**：在详情页执行「内容识别」/「组合扫描」，进度条完成后每张照片自动归类（动物 / 食物 / 建筑 / 夜景 / 人像等），人物自动聚集。

### 6.3 日常使用

- **时间线**：跨相册按时间浏览全部照片。
- **智能搜索**：输入关键词或组合「条件过滤」（EXIF 拍摄时间 / 影调 / AI 类别）精确检索。
- **人物总览**：进入人物页，对人物进行改名、合并（同一人多张面孔）、删除误检。
- **手动整理**：在「分组」中创建文件夹树并将相册批量移入；在「手动排序」中调整顺序；为相册批量设置地点 / 标签。
- **批量导出**：选中多张照片一键导出到指定文件夹。
- **回忆**：进入「回忆」视图查看基于人物 / 地点 / 节日的聚合。

### 6.4 隐私与安全

- 照片、缩略图与人物数据全部存放于本机 `app_data_dir`，绝不上传。
- 跨机迁移时，拷贝照片文件夹并重新导入即可；相册元数据随 `photos.db` 整体迁移。

---

## 七、构建与发布

### 7.1 依赖说明

- **主程序**：Tauri 编译产物 `PhotoManagementSys.exe`（内嵌 Rust 后端与 Vue 前端）。
- **VCR 微服务**：独立打包为 `vcr-server.exe`（PyInstaller 单文件，约 85MB，含 Python 运行时与依赖），随安装包内嵌至安装目录 `vcr/`。
- **AI 模型**：`.onnx` 模型文件体积较大，不入 git 仓库（由脚本下载 / 导出生成），随安装包内嵌至 `vcr/models/`，或由用户按说明放置。

### 7.2 模型放置说明

默认安装包已包含模型。若安装包不含模型，请确认模型目录下存在以下文件：

```
vcr/models/
├─ yolov8n-cls.onnx        # 分类（必选）
├─ yolov8n-det.onnx        # COCO 检测（必选）
├─ det_500m.onnx           # SCRFD 人脸检测（必选）
├─ w600k_mbf.onnx          # ArcFace 人像特征（必选）
├─ paddleocr-det.onnx      # 文档 OCR（可选）
├─ album_groups.json       # 9 大类定义
├─ imagenet_classes.txt    # ImageNet 类名
└─ imagenet_to_album.json  # ImageNet → 大类映射
```

可选模型（缺失时自动降级）：`resnet18_places365.onnx`（场景）、`efficientnet-b2-flowers.onnx`（花朵专家）。

### 7.3 模型获取与更新

- **人脸 + OCR**：执行 `python/download_models.py`（从 GitHub / ModelScope 下载）。
- **分类 + 检测**：需以 `ultralytics` 导出（`python/export_model.py` 导出分类，检测同理），或直接复制开发机 `python/models/` 下对应 `.onnx`。
- 模型不进入 git 仓库（体积过大），请通过 GitHub Release 附加资产（模型 zip）或 README 链接获取。

### 7.4 发布打包

在项目根目录执行：

```powershell
powershell -ExecutionPolicy Bypass -File release.ps1
```

脚本依次执行：① 以最小化 venv 构建 `python/dist/vcr-server.exe` → ② 校验 / 补齐模型 → ③ `npm install` → ④ `npx tauri build --config src-tauri/release.tauri.conf.json`（合并 release 配置，将微服务与模型内嵌进安装包）。

产物位置：`src-tauri/target/release/bundle/msi/` 与 `nsis/`。

---

## 八、开发环境搭建

### 8.1 依赖

- Node.js 18+，npm
- Rust + Cargo（需 MSVC 构建工具）
- Python 3.10+（用于 VCR 微服务开发）
- Tauri CLI：`npm run tauri` 或 `cargo install tauri-cli`

环境一键脚本（Windows，可选）：`setup-env.ps1`（管理员运行，安装 Rust、C++ Build Tools 与镜像）。

### 8.2 启动

```bash
# 1. 安装前端依赖
npm install

# 2. 安装 VCR 微服务依赖（开发版，使用系统 Python）
pip install -r python/requirements.txt -i https://pypi.tuna.tsinghua.edu.cn/simple

# 3. 下载模型（人脸 / OCR）
python python/download_models.py

# 4. 启动全部（tauri dev 自动拉起前端与 Rust；微服务由 Rust 按需启动 python server.py）
npm run tauri dev
```

开发模式下 Rust 回退到 `python server.py` 启动微服务（见 `vision.rs`）；打包版启动内置的 `vcr-server.exe`。

---

## 九、目录结构

```
PhotoMangementSys/
├─ src/                         # Vue 3 前端
│  ├─ views/                    #   路由视图（相册列表 / 详情 / 时间线 / 回忆 / 搜索…）
│  ├─ components/               #   可复用组件（PhotoGrid / Toast / AlbumCard…）
│  ├─ stores/                   #   Pinia 状态（album / content / toast / theme / auth）
│  ├─ composables/              #   组合函数（useNotify）
│  ├─ router/  utils/  types/   #   路由 / 工具 / 类型
├─ src-tauri/                   # Rust 后端 + Tauri 壳
│  ├─ src/                      #   lib.rs（命令 / 装配）+ db / thumbnail / vision / …
│  ├─ tauri.conf.json           #   主配置
│  └─ release.tauri.conf.json   #   发布专用配置（内嵌微服务 + 模型资源）
├─ python/                      # VCR 视觉识别微服务
│  ├─ server.py                 #   FastAPI 接口层
│  ├─ vcr/                      #   服务层 / 持久层 / 基础设施
│  ├─ models/                   #   模型（不入 git）
│  ├─ build_vcr_exe.ps1         #   以最小化 venv 构建 vcr-server.exe
│  └─ requirements.txt          #   运行依赖
├─ release.ps1                  # 一键发布打包（exe + 模型 + MSI）
├─ package.json / vite.config.ts
└─ README.md
```

---

## 十、常见问题

**Q：`/health` 显示 GPU 不可用？**
打包版默认使用 CPU 推理。如需 GPU，需使用带 DirectML / CUDA 提供方的 onnxruntime 重新打包；开发版可 `pip install onnxruntime-directml` 并设置 `VCR_PROVIDER=auto`。

**Q：更换电脑后，相册中的照片是否可找回？**
照片为本地文件，仅与文件夹路径绑定。将照片文件夹一并拷贝至新机并重新导入即可；标签 / 分类数据建议连同 `photos.db` 一并迁移（需使用同版本）。

**Q：识别服务未启动 / 提示「模型未加载」？**
检查 `vcr/models/` 下是否存在对应 `.onnx` 文件；若无，按「七、构建与发布」获取。开发版需先安装 `python/requirements.txt`。

---

## 十一、许可证

私有项目。作者：haoyuan。
