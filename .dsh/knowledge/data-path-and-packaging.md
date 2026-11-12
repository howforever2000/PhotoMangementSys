# 数据路径 / 打包 / 安装

> 本项目「开发态能跑、装到别的机器就崩」类问题的集合：路径解析、只读目录、标识符迁移、SQLite 权限、打包编码。均来自 `bug-notes.json`。

## 禁止用 `CARGO_MANIFEST_DIR` 拼运行时路径，收敛为唯一解析入口

- **触发条件**：`model_dl.rs` / `persons.rs` / `vision.rs` 用 `env!("CARGO_MANIFEST_DIR")` 拼模型目录或数据目录。
- **先做**：收敛为唯一入口 —— `vision::resolve_model_dir()`（优先级：`VCR_MODEL_DIR` → `resource_dir/vcr/models`（打包）→ 源码 `python/models`（开发））与 `data_dir()`（`VCR_DATA_DIR` → `app_data_dir/vcr-data`）；`lib.rs::setup` 启动时解析一次并 `std::env::set_var(...)` + 写 `app.log`（打印「模型目录: ...」「人物数据目录: ...」）；修复时做**全仓 `CARGO_MANIFEST_DIR` 审计**。
- **为什么**：编译期宏固化为构建机源码路径，装到别的机器指向不存在目录；且宿主进程自身从不设 `VCR_MODEL_DIR/VCR_DATA_DIR`，只有 `spawn_server` 给子进程注入 → 宿主侧全部落到坏回退分支。
- **违规症状**：模型内置却显示「未下载」；应用内下载落到不存在目录；人物页与微服务各读写两份 `persons.db`；release 二进制暴露构建机用户名。
- **来源**：`bug-notes.json` BUG-2026-0916-003、BUG-2026-0916-005、BUG-2026-1013-001。
- **记录日期**：2026-11-12

## Program Files 只读 → 模型目录不可写时硬链接到 app_data

- **触发条件**：MSI `ALLUSERS=1` 装到 `Program Files`，但语义链路需要**写**模型目录（子图拆分产物、档位记录、模型下载落位）。
- **先做**：`resolve_model_dir()` 内加 `dir_writable()`（试写 `.pms-write-probe` 探针）；不可写则 `link_or_copy_tree()` 用 `std::fs::hard_link`（同卷零拷贝、失败退回 `copy`）把整棵模型树链到 `app_data_dir/vcr-models`，并把该可写副本作为模型目录。
- **为什么**：Tauri 的 WiX(MSI) 不支持 `installMode`（只有 NSIS 支持），MSI 固定按机器装 `Program Files`，对非提权进程只读 → 写入必然失败。
- **违规症状**：语义子图拆分、档位持久化、应用内模型下载全部写入失败。
- **来源**：`bug-notes.json` BUG-2026-0916-004（`src-tauri/src/vision.rs`、`release.tauri.conf.json`）。
- **记录日期**：2026-11-12

## 改应用标识符必须做数据目录迁移 + 库内绝对路径重写

- **触发条件**：改 `tauri.conf.json` 的 identifier，`app_data_dir` 物理路径随之变化。
- **先做**：一次性 `migrate_legacy_data_dir()` 搬目录，**并重写库内绝对路径**（`albums.cover_path`、`photo_thumb_cache.thumb_path` 等）的前缀。
- **为什么**：标识符落进每台用户机的 `%APPDATA%` 目录名（隐私 + 路径变更）；库内绝对路径在目录改名后原地变死链，`convertFileSrc()` 拿到不存在路径。
- **违规症状**：改标识符后所有相册封面碎图（25054 行变死链、130 行封面全碎）；相册库 / 缩略图 / 加密密钥「集体丢失」。
- **来源**：`bug-notes.json` BUG-2026-1014-001、BUG-2026-1021-005。
- **记录日期**：2026-11-12

## SQLite `ReadOnly`/`CannotOpen` 先测目录可写，再怀疑库

- **触发条件**：启动即崩 `attempt to write a readonly database` / `unable to open database file`。
- **先做**：测目录可写（试写探针）；最快归因 = **同一 exe 换个可写路径跑**（可写位能起、不可写位崩 → 环境权限；两边都崩 → 代码/库）。
- **为什么**：SQLite 因此无法创建 `photos.db-wal`/`-shm`；Low 完整性标签下从工作区启动的 exe 只能写 Low IL 对象，而 `%APPDATA%` 是 Medium。
- **违规症状**：`ReadOnly(8)` 与 `CannotOpen(14)` 是同一根因的两种表现（取决于失败瞬间 WAL 是否还在），别解释成两个 bug。
- **来源**：`bug-notes.json` BUG-2026-1026-002、BUG-2026-1027-007。
- **记录日期**：2026-11-12

## requirements 文件必须纯 ASCII

- **触发条件**：pip 在中文 Windows 上 `install -r`。
- **先做**：requirements 注释全改英文 ASCII。
- **为什么**：pip 用系统 locale（zh-CN → cp936/GBK）解码而非 UTF-8，含中文即抛 `UnicodeDecodeError`。
- **违规症状**：打包流水线挂死，只留「依赖安装失败」一句。
- **来源**：`bug-notes.json` BUG-2026-0920-001（`python/requirements-vcr-packaging.txt`）。
- **记录日期**：2026-11-12

## 下载 URL 只用「仓库内路径」，本地落位路径单独拆

- **触发条件**：应用内「下载模型」全部 404（CLI 能下、点界面下不了）。
- **先做**：拆 `repo_file()` = `onnx/<file>` 专供 URL，`rel_file()` = `<root>/onnx/<file>` 只用于本地落位 / 存在性判定；加断言锁死 URL 形态（不得含本地目录名 `/{root}/`）。
- **为什么**：把「本地落位目录名 `root`」当成「仓库内路径」拼进 URL，仓库里没有这一层目录。
- **违规症状**：整图 404，而附带文件（tokenizer/vocab，用 `base_url + 文件名`）恰好正确。
- **来源**：`bug-notes.json` BUG-2026-0921-001、BUG-2026-0920-002（`src-tauri/src/model_dl.rs`）。
- **记录日期**：2026-11-12
