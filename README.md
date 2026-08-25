# PhotoManagementSys — 本地相册管理系统

> 一款**本地优先（Local-First）** 的桌面相册管理应用，配合 AI 视觉识别，帮你把成千上万张散乱的照片**自动归类、可搜索、可回溯、轻松整理**。全程离线可运行，数据不出本机。

[![License](https://img.shields.io/badge/license-私有-1f6feb.svg)]()
[![Platform](https://img.shields.io/badge/platform-Windows%20(x64)-1f6feb.svg)]()

---

## 一、它是谁？相册定位

在这个手机拍照越来越方便、照片暴涨到几万张的时代，主流云相册（Google Photos / iCloud / 百度网盘）把照片上传到服务器，既耗费流量、又担心隐私，还得付存储费。**PhotoManagementSys 反其道而行**：

- **本地存储，隐私优先**：照片与全部管理数据都留在你自己的电脑上，绝不上传。
- **AI 自动整理，离线可用**：内置视觉识别微服务，把照片自动分类为「动物 / 食物 / 花朵 / 建筑 / 夜景 / 人像 / 文档 / 扫街 / 城市风光」等大类，还能识别人物、场景、文字，全部在你本机 CPU 上完成推理。
- **抽象你的真实相册**：相册是对**本地文件夹**的抽象。你既可以把一个已有图片文件夹直接“导入”成相册，也可以新建相册再绑定文件夹，之后改名相册、移动文件夹、批量归类，都不会弄乱原始照片。
- **为“找回一张照片”而生**：时间线、智能搜索、条件过滤、人物总览、回忆，都是为了让你在几秒内找到几个月前的某一张。

一句话概括：**一台电脑上，离线运行的“私人智能相册”**。

---

## 二、核心特性

| 模块 | 能力 |
| --- | --- |
| 相册管理 | 创建 / 编辑 / 删除 / 重命名 / 封面 / 标签 / 地点 / 说明；对已有文件夹一键导入 |
| 多用户隔离 | 同机多账户，相册、分组、标签、搜索、排序按登录用户隔离；密码 Argon2id 加盐哈希 |
| 智能分类 | 视觉微服务把照片自动归入 9 大类；动物细分狗/猫/鸟；支持文档 OCR、场景、夜景判定 |
| 人物识别 | 人脸检测 + 特征提取，自动聚集“同一人”，可改名、合并、删除，生成人物头像 |
| 时间线 / 回忆 | 跨相册查看照片时间线；聚合人物、地点、节日的回忆视图 |
| 搜索 | 标题/标签/地点/内容语义组合搜索，支持 EXIF + 影调 + AI 三合一条件过滤 |
| 手动整理 | 分组（文件夹）树、批量移动/归类、手动排序、编辑说明 |
| 批量操作 | 批量设地点/标签、批量移动到相册、批量导出到文件夹 |
| 隐私安全 | 邮箱/手机号/密码字段 AES-GCM 加密落库；记住登录（3 天免密） |
| 可离线 | 缩略图、人物头像、GPS→省市地理反查均本地完成，不依赖网络 |

---

## 三、技术架构：三端解耦的桌面应用

项目不做成“一个 monolith”，而是拆成**三层独立进程 + 一个独立微服务**，各司其职、按契约协作：

```
┌─────────────────────────────────────────────────────────────┐
│  Frontend  Vue 3 + TypeScript + Vite + Pinia + Vue Router     │
│  （渲染进程，WebView；负责 UI、状态、调用后端命令）              │
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
│  FastAPI + ONNX Runtime，分层：接口/服务/持久化/基础设施         │
│  模型：YOLOv8 分类/检测 · SCRFD 人脸 · ArcFace 识别 · PaddleOCR │
└─────────────────────────────────────────────────────────────┘
```

**为什么这样分？**
- **Rust 负责“重活”**：文件遍历、缩略图、EXIF、SQLite、密码学等本地 I/O 与安全性，用 Rust 的高性能与内存安全。
- **Python 负责“AI”**：机器学习生态都在 Python/ONNX，用独立的 Python 微服务承载推理，避免往 Rust 里塞笨重的推理栈。
- **Vue 负责“界面”**：响应式 UI、组件复用、状态管理，用前端生态快速迭代。

### 技术栈

| 层 | 技术 |
| --- | --- |
| 桌面壳 | Tauri 2.x（Rust） |
| 前端 | Vue 3 `<script setup>` + TypeScript + Vite 6 + Pinia + Vue Router 4 |
| Rust 后端 | rusqlite(SQLite 内置) / image / jpeg-decoder / kamadak-exif / walkdir / argon2 / aes-gcm / reqwest / tokio |
| Python 微服务 | FastAPI + uvicorn + onnxruntime + Pillow + numpy + opencv-python |
| AI 模型 | YOLOv8s/n-cls（分类）、YOLOv8n-det（COCO 检测）、SCRFD（人脸框）、ArcFace（人像向量）、PaddleOCR（文字） |

---

## 四、解耦开发原则

工程从第一天就把边界画清楚，让各个模块可以独立演进、替换、甚至剥离：

1. **进程级解耦**：Rust 后端与 Python 微服务之间用**固定 REST 契约**（`POST /classify_batch`、`GET /health`…）通信，端口固定 `127.0.0.1:8765`。两边都可以单独开发、单独测试、单独打包成 exe。
2. **客户端不直连复杂逻辑**：Rust 的 `vision.rs` 只是一个**薄 HTTP 客户端 + 生命周期管理器**，它不依赖 `db` / `thumbnail` / `tone` 模块（图片扩展名列表本地复制一份，防止隐式耦合），服务不可用 / 模型缺失时返回明确错误，**不影响其他功能**。
3. **命令层薄壳**：`lib.rs` 中的 `#[tauri::command]` 只做参数透传 + 日志 + 状态注入，真正的逻辑在 `db` / `content` / `persons` 等模块里。每个命令等价于一个 Controller 端点，`tauri::State` 等价于 Spring 的 `@Autowired` 注入。
4. **配置文件集中**：Python 端所有硬编码（路径 / 阈值 / 模型清单 / GPU 策略）都集中在 `python/vcr/config.py`，服务层只引用 `config` 常量，改参数不动代码。
5. **降级优先**：任何可选的模型（场景 / 花朵 / 美食 / OCR）缺失都自动降级，主分类通道（必选）不受影响——保证“万一没模型也能用”。

---

## 五、微服务设计原理（VCR 视觉内容识别）

VCR（Visual Content Recognition）是一个独立的 Python 服务，把照片交给多路模型推理并仲裁出最终分类。它采用**经典分层架构**：

```
python/
├─ server.py                  # 接口层：FastAPI 路由 + DTO（薄壳，无业务逻辑）
├─ vcr/
│  ├─ config.py               # 基础设施：路径/阈值/模型清单（集中配置）
│  ├─ model_registry.py       # 基础设施:模型注册表，惰性加载 + GPU 提供方选择
│  ├─ preprocess.py           # 基础设施：图像预处理（缩放/归一化）
│  ├─ mapping.py              # 基础设施：ImageNet→9 大类映射
│  ├─ taxonomy.py             # 基础设施：taxonomy 折叠（保证输出 ∈ 9 类）
│  ├─ schemas.py              # 接口层：Pydantic DTO
│  ├─ persistence/
│  │  └─ person_store.py      # 持久层：SQLite 人物注册表
│  └─ services/               # 服务层：业务编排
│     ├─ classifier.py        #   分类通道（YOLOv8-cls）
│     ├─ detector.py          #   检测通道（YOLOv8-det，人/车/物）
│     ├─ face_service.py      #   人脸通道（SCRFD + ArcFace）
│     ├─ scene_service.py     #   场景通道（Places365，可选）
│     ├─ ocr_service.py       #   文档通道（PaddleOCR，可选）
│     ├─ flower/food_service  #   专家通道（懒加载，可选）
│     ├─ tone_service.py      #   影调通道（夜景/低调判定）
│     ├─ arbitrator.py        #   仲裁器：多通道结果→单一结论
│     └─ pipeline.py          #   流水线编排：单张图多路推理→仲裁→结果
```

**关键设计点**：

- **一次解码，多路分发**：`pipeline.py` 只解码一次图片，再把张量分发给各专家通道，避免重复 IO。
- **条件触发专家通道**：花朵 / 美食等专家模型是**懒加载**的，只有判定命中触发条件才加载，避免全量推理开销。
- **仲裁器（arbitrator）**：各通道（分类、检测、场景、影调、OCR、专家）各自给出候选，仲裁器按阈值与优先级合成**唯一**结论，保证“category ∈ 9 组”。
- **自动降级**：模型缺失时对应通道自动降级，`/health` 返回真实就绪状态。
- **GPU 可选**：默认自动检测 `onnxruntime` 的 GPU 提供方（DirectML / CUDA），有则用 GPU，无则 CPU，可用 `VCR_PROVIDER=cpu` 强制 CPU。

---

## 六、前端组件可复用原理

前端大量采用**组件化 + 组合式 API**，让同样的界面元素在多处复用，降低重复代码：

- **通用基础组件**：`Toast` / `ToastContainer`（全局通知）、`ConfirmDialog`（确认框）、`CollapseSection`（折叠面板）、`PhotoGrid`（照片网格）、`PhotoLightbox`（灯箱）、`AlbumCard` / `AlbumMiniCard`（相册卡片）。这些组件在首页、相册列表、时间线、回忆等多个视图反复使用。
- **复杂视图拆分**：`AlbumDetail.vue` 从 **2709 行重构到 183 行**，拆成 `AlbumMeta`（相册元信息）、`PhotoGrid`、`CollapseSection` 等 4 个子组件——每个子组件只关注一件事，父组件只做数据装配。
- **状态集中（Pinia store）**：把跨视图共享的状态（album / content / toast / theme / auth）放进 store，组件通过 store 读写，避免“层层 props 钻透”。如 `useNotify.ts` 组合函数把 Toast 通知逻辑封装成可注入串。
- **composable 复用**：`useNotify.ts` 把“触发通知”抽成组合函数，任何组件都能一行调用。
- **类型驱动**：`types/` 里定义 `Album` / `Photo` / `Content` 等 TS 类型，与后端 serde JSON 一一对应，前后端契约清晰，改字段立刻能编译期发现。

---

## 七、优化细节（性能与体验）

项目在开发中持续做“体验优化”，这里挑几个典型的：

- **缩略图 DCT 降采样解码**：`image::open` 全尺寸解码 6000×4000 巨图需 5~14 秒，切换封面会卡死。改用 `jpeg-decoder` 的 `scale`（只解所需 DCT 块），**换封面从 7.7s 降到 0.76s**（约 10 倍）。缓存写入 `app_data_dir/thumbs`，不写回数据库。
- **批量内容识别**：`/classify_batch` 支持一次最多 64 张（默认客户端 8），配合 Rust 端分批提交，全程用 `classify-progress` 事件实时上报进度条。
- **GPU / 批次加速**：识别服务自动探测 GPU（DirectML），可用时走 GPU；否则 CPU 多线程（`THREADS=4`）。
- **GPS→省市离线反查**：把各省市边界数据内嵌本地，用“点面判断”在离线状态下反查照片地理位置，替代“逐张联网反编码”，省时且隐私。
- **组合扫描三合一**：EXIF（拍摄时间）+ 影调（曝光/夜景）+ AI（内容识别）统一在一次“组合扫描”里完成，减少重复遍历。
- **分页加载**：照片网格分页，避免大相册一次性渲染卡顿。
- **多用户安全**：密码用 **Argon2id**（抗 GPU 暴力）；邮箱/手机号/密码哈希用 **AES-256-GCM** 加密落库，不存明文。

---

## 八、面向普通用户的使用说明

### 8.1 安装

- **方式一（推荐，Windows）**：下载 Release 里的安装包，双击安装：
  - `PhotoManagementSys_<版本>_x64_en-US.msi`
  - 或 `PhotoManagementSys_<版本>_x64-setup.exe`（NSIS）
- 安装包通常已内置 AI 模型（详见「九、Release 打包与模型」）。**若安装包不含模型**，请按「模型放置说明」把模型文件放到 `模型目录` 后再启动。

### 8.2 首次使用

1. **注册账号**：打开应用，用账户名 / 邮箱 / 手机号 + 密码注册（同机可多用户，各自相册隔离）。
2. **导入已有照片**：在「相册管理」页点击「新建相册」，选择你放照片的本地文件夹，应用会扫描其中的图片（支持 jpg / jpeg / png / webp / gif / bmp）。
3. **开始 AI 识别**：在详情页点击「内容识别」/「组合扫描」，进度条走完后每张照片会被自动归类（动物/食物/建筑/夜景/人像…），人物会自动聚集。

### 8.3 日常使用

- **看时间线**：跨相册按时间浏览所有照片。
- **智能搜索**：输入关键词，或用「条件过滤」组合 EXIF 拍摄时间 / 影调（夜景、低调）/ AI 类别精确找图。
- **人物总览**：进入人物页，给某个人改名、合并“同一个人的多个分身”、删除误检。
- **手动整理**：在「分组」里建文件夹树，把相册批量拖入；在「手动排序」里调整顺序；给相册批量设地点 / 标签。
- **批量导出**：选中多张照片一键导出到指定文件夹。
- **找回“回忆”**：进入「回忆」视图，看到基于人物 / 地点 / 节日的聚合。

### 8.4 隐私与安全

- 照片、缩略图、人物数据全部留在本机 `app_data_dir`，**绝不上传**。
- 如需在另一台电脑继续管理，只要把照片文件夹拷贝过去并导入即可（相册元数据随 `photos.db`，可整体迁移）。

---

## 九、Release 打包与模型放置说明

### 9.1 依赖说明

- **主程序**：Tauri 编译出的 `PhotoManagementSys.exe`（内嵌 Rust 后端 + Vue 前端）。
- **VCR 微服务**：独立打包为 `vcr-server.exe`（PyInstaller 单文件，约 85MB，含 Python 运行时+依赖），随 MSI 内嵌到安装目录 `vcr/` 下。
- **AI 模型**：`.onnx` 模型文件**体积较大、不入 git 仓库**（由脚本下载/导出生成），随 MSI 内嵌到 `vcr/models/`，或由用户按说明放置。

### 9.2 安装包是否包含模型？

默认**包含**（`release.ps1` 会把微服务 + 模型一起塞进 MSI）。若你的 MSI 不含模型，请确认模型目录 `模型目录` 下有以下文件：

```
vcr/models/
├─ yolov8n-cls.onnx        # 分类（必选）
├─ yolov8n-det.onnx        # COCO 检测（必选）
├─ det_500m.onnx           # SCRFD 人脸检测（必选）
├─ w600k_mbf.onnx          # ArcFace 人像特征（必选）
├─ paddleocr-det.onnx      # 文档 OCR（可选）
├─ album_groups.json       # 9 大类定义
├─ imagenet_classes.txt    # ImageNet 类名
└─ imagenet_to_album.json  # ImageNet→大类映射
```

> 可选模型（缺省自动降级）：`resnet18_places365.onnx`（场景）、`efficientnet-b2-flowers.onnx`（花朵专家）。

### 9.3 获取 / 更新模型

- **人脸 + OCR**：`python/download_models.py`（从 GitHub / ModelScope 下载）。
- **分类 + 检测**：需用 `ultralytics` 导出（`python/export_model.py` 导出分类；检测同理），或直接从已有开发机复制 `python/models/` 下对应 `.onnx`。
- 模型不会上传到 GitHub（体积过大），请通过 **GitHub Release 的附加资产（模型 zip）** 或在 README 链接下载，再放入安装目录。

### 9.4 重新打包（开发者）

在项目根目录执行：

```powershell
powershell -ExecutionPolicy Bypass -File release.ps1
```

脚本会依次：① 用最小化 venv 构建 `python/dist/vcr-server.exe` → ② 校验/补齐模型 → ③ `npm install` → ④ `npx tauri build --config src-tauri/release.tauri.conf.json`（合并 release 配置，把微服务+模型内嵌进 MSI/NSIS）。

产出位置：`src-tauri/target/release/bundle/msi/` 与 `nsis/`。

---

## 十、开发环境搭建

### 10.1 依赖

- Node.js 18+，npm
- Rust + Cargo（需 MSVC 构建工具）
- Python 3.10+（用于 VCR 微服务开发）
- Tauri CLI：`npm run tauri` 或全局 `cargo install tauri-cli`

一键环境脚本（Windows，可选）：`setup-env.ps1`（管理员运行，装 Rust + C++ Build Tools + 镜像）。

### 10.2 启动（前端 + Rust 后端 + 微服务）

```bash
# 1. 安装前端依赖
npm install

# 2. 安装 VCR 微服务依赖（开发版，用系统 python）
pip install -r python/requirements.txt -i https://pypi.tuna.tsinghua.edu.cn/simple

# 3. 下载模型（人脸/OCR）
python python/download_models.py

# 4. 启动全部（tauri dev 会自动拉起前端与 Rust；微服务由 Rust 在需要时自动拉起 python server.py）
npm run tauri dev
```

> 开发模式下 Rust 会回退到 `python server.py` 启动微服务（见 `vision.rs`）；打包版则启动内置的 `vcr-server.exe`。

---

## 十一、目录结构

```
PhotoMangementSys/
├─ src/                         # Vue 3 前端
│  ├─ views/                    #   路由视图（相册列表/详情/时间线/回忆/搜索…）
│  ├─ components/               #   可复用组件（PhotoGrid/Toast/AlbumCard…）
│  ├─ stores/                   #   Pinia 状态（album/content/toast/theme/auth）
│  ├─ composables/              #   组合函数（useNotify）
│  ├─ router/  utils/  types/   #   路由 / 工具 / 类型
├─ src-tauri/                   # Rust 后端 + Tauri 壳
│  ├─ src/                      #   lib.rs(命令/装配) + db/thumbnail/vision/…
│  ├─ tauri.conf.json           #   主配置
│  └─ release.tauri.conf.json   #   发布专用配置（内嵌微服务+模型资源）
├─ python/                      # VCR 视觉识别微服务
│  ├─ server.py                 #   FastAPI 接口层
│  ├─ vcr/                      #   服务层/持久层/基础设施
│  ├─ models/                   #   模型（不入 git）
│  ├─ build_vcr_exe.ps1         #   用最小化 venv 构建 vcr-server.exe
│  └─ requirements.txt          #   运行依赖
├─ release.ps1                  # 一键发布打包（exe + 模型 + MSI）
├─ package.json / vite.config.ts
└─ README.md
```

---

## 十二、常见问题（FAQ）

**Q：为什么 /health 显示 GPU 不可用？**
打包版 exe 默认用 CPU 推理（`onnxruntime`）。若要 GPU，需用带 DirectML/CUDA 提供方的 onnxruntime 重新打包（体积更大）。开发版可 `pip install onnxruntime-directml` 并设置 `VCR_PROVIDER=auto`。

**Q：换了台电脑，相册里的照片还能找到吗？**
照片是本地文件，只与文件夹路径绑定。把照片文件夹一起拷到新机器，重新导入即可；相册的标签/分类最好连同 `photos.db` 一并迁移（需用同版本）。

**Q：识别服务没起来 / 报“模型未加载”？**
检查 `vcr/models/` 下是否有对应 `.onnx` 文件；若无，按“九、Release 打包与模型”获取。开发版需先 `pip install -r python/requirements.txt`。

---

## 十三、License

私有项目，暂不对外开源。作者：haoyuan。
