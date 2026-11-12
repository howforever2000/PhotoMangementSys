# Python 边车（VCR / AI Studio）进程生命周期

> 本项目 Rust 宿主按需拉起两个常驻 Python 服务（VCR 视觉识别、AI 创作）。
> 这篇文章是这一类坑的合集，均在本仓库真实踩过。来源均为 `bug-notes.json` 条目。

## 长生命周期子进程必须做 API 版本协商

- **触发条件**：Rust 宿主复用「跨应用重启仍存活」的 Python 服务，且服务端新增了端点 / DTO。
- **先做**：`/health` 返回 `api_version`；宿主探测到旧版本时先 `POST /shutdown` 拉起新版本；每次新增端点同步抬升 `VCR_API_VERSION`。
- **为什么**：`ensure_service_ready` 见到端口有存活服务就直接复用，旧进程没有新端点 → 405/404，新版改动永不生效。
- **违规症状**：新增端点 404/405；UI「功能没生效」；改了代码像没改。
- **来源**：`bug-notes.json` BUG-2026-0908-005、BUG-20261001-017（新增端点忘抬 VCR_API_VERSION → 404）。
- **记录日期**：2026-11-12

## 子进程启动必须单飞 + 动态端口，禁止盲杀端口占用者

- **触发条件**：多条命令并发各自 `ensure`，或多实例抢固定端口。
- **先做**：`ensure` 用单飞锁（tokio Mutex，拿锁后重新探测）；绑定 `127.0.0.1:0` 取系统分配端口，经环境变量 `VCR_PORT` 传给服务（打包版落在 18765~18865 冷门段）；不要盲目 kill 端口占用进程。
- **为什么**：并发 `ensure` 同时拉起多个 `server.py` 抢 8765 → `winerror 10048`；固定端口易被第三方占用，盲杀可能误杀第三方并因残留孤儿反复抢端口形成循环。
- **违规症状**：`winerror 10048`；多进程互抢；端口占用者被误杀；进程连环生灭。
- **来源**：`bug-notes.json` BUG-2026-0908-006、BUG-2026-0908-008。
- **记录日期**：2026-11-12

## 「重启进程自愈版本」的前提是重拉的二进制会变

- **触发条件**：`spawn_server` 有「打包 exe 优先」回退链，且开发机 `target/*/vcr/` 残留过期 `vcr-server.exe`。
- **先做**：登记过期组件指纹（组件路径 + mtime）；探测仍为 OldVersion 就杀进程、登记组件、返回「组件过旧请重新打包」明确错误，后续 `ensure` 命中登记直接报错不再杀拉；组件重打包（mtime 变化）自动解除；`taskkill` 加 `/T`。
- **为什么**：旧 exe 的 `/health` 无 `api_version` → 判 OldVersion → 杀掉重拉同一份旧文件 → 仍 OldVersion，杀→拉循环永不收敛；开发模式被 08-25 旧 exe 劫持，`server.py` 上的新修复从未执行。
- **违规症状**：十几个进程连环生灭；改了脚本不生效；先确认实际运行的是哪个文件。
- **来源**：`bug-notes.json` BUG-2026-0917-001。
- **记录日期**：2026-11-12

## 不要用 PATH 上的裸 python，必须解析真解释器并接管子进程输出

- **触发条件**：开发模式 `Command::new("python")` 依赖 PATH 解析解释器。
- **先做**：候选优先级 = 项目 venv（`python/.venv-vcr`、`.venv`）→ Anaconda/Miniconda/`Program Files`/`C:\Python3xx` 绝对路径 → PATH `python/python3`；用 `候选 -c "import sys, fastapi, uvicorn; print(sys.executable)"` 探针真跑一次验真（20s 超时，缓存：成功永久、失败 30s TTL）；子进程 stdout/stderr 接进 `app.log`。
- **为什么**：Windows PATH 首位常是 Microsoft Store「应用执行别名」占位符，`spawn` 成功但进程秒退并打印 `Python was not found; run without arguments to install from the Microsoft Store...`；系统 python 常缺 fastapi。
- **违规症状**：「服务出错、面板展不开」「找不到可用的 Python 解释器」；后台堆一堆 python；进程秒退只剩误导文案。
- **来源**：`bug-notes.json` BUG-2026-0918-002、BUG-2026-1111-001。
- **记录日期**：2026-11-12

## 常驻服务的输出读线程千万别 join

- **触发条件**：`ensure` 探活成功后 `join` 子进程 stdout/stderr 读线程。
- **先做**：成功路径先落实例文件 / 缓存，再 `drop(readers)`（线程后台排空管道，进程退出自然结束），**绝不 join**；要日志就用「带排空上限的读取」（轮询缓冲取尾部）；副作用（写 instance、设缓存）排在任何可能阻塞的语句之前。
- **为什么**：读线程体是 `read_to_string`，要 EOF 才返回，常驻服务活着就永不 EOF → `join` 永久阻塞 → 命令永不 resolve；排在 join 之后的 `save_instance` 永不执行 → 每次重试又新起一个服务。
- **违规症状**：列表永远「加载中」；后台堆一堆 python；app 数据目录始终没有 `studio-instance.json`（收养形同虚设）。
- **来源**：`bug-notes.json` BUG-2026-0918-009。
- **记录日期**：2026-11-12

## 收养存活服务要验明 PID 正身，别把「PID 存在」当「我们的服务存活」

- **触发条件**：`instance.json` 记录 pid，跨应用重启收养存活服务。
- **先做**：`pid_alive` 之外再验映像名 + 命令行（打包版认 `vcr-server.exe`；开发版认 `python.exe` 且命令行含 `server.py`）；收养等待加累计次数上限（`wait_fails`），超限判僵死强杀转冷启动；kill 加 `/T`（Windows venv python 是「启动器 + 真实解释器」双层进程）。
- **为什么**：PID 会被操作系统回收复用，只判 PID 存在会把别的程序当自己服务，收养等待永久为真、无限重复。
- **违规症状**：对空端口白等 90s；服务永远起不来；误杀用户其它 python 进程。
- **来源**：`bug-notes.json` BUG-2026-0928-001。
- **记录日期**：2026-11-12

## 大模型加载不能占住 HTTP 请求

- **触发条件**：服务端切换 / 加载大模型（CLIP、人脸等）。
- **先做**：`/model` 切换改异步两步（`begin` 校验登记立即返回 + 后台 `finish` 加载，失败回退默认候选并清持久化）；用 `/health` ok 协议让宿主等待就绪，而不是让请求挂着。
- **为什么**：宿主 HTTP 超时（15s）远小于加载耗时，请求挂起会被当超时 → 杀进程 → 重启，形成死循环。
- **违规症状**：切换模型转圈；15s 超时死循环；前端选择模型一直转圈。
- **来源**：`bug-notes.json` BUG-2026-0917-001、BUG-2026-0908-009。
- **记录日期**：2026-11-12

## 应用退出要优雅终止 Python 服务

- **触发条件**：应用点叉退出。
- **先做**：退出钩子里终止 VCR 与 AI 创作两个 Python 服务（含 `/T` 杀进程树），不留后台孤儿。
- **为什么**：不终止会留下孤儿进程，下次启动端口 / 实例收养混乱。
- **来源**：`feature-notes.json` FEAT-159。
- **记录日期**：2026-11-12

## 解释器探针必须覆盖「该服务真实 import 全依赖」，公共探针不能一钥通用

- **触发条件**：多个 Python 边车（VCR / AI Studio）共用同一个 `resolve_python_interpreter` 与 `INTERP_PROBE`。
- **先做**：按服务拆分解析函数与缓存。VCR 用独立 `VCR_INTERP_PROBE`（额外 `import onnxruntime, cv2, tokenizers, onnx, numpy, PIL, requests`）+ `VCR_INTERP_CACHE`，`spawn_server` 开发分支调 `resolve_vcr_interpreter()`；AI 创作仍用只查 `fastapi, uvicorn` 的 `INTERP_PROBE`/`resolve_python_interpreter`。
- **为什么**：`INTERP_PROBE` 只查 fastapi/uvicorn，会把「能跑 AI 创作但缺 onnxruntime」的 Anaconda 3.14.6 判为合格；`python/server.py:46` 在 `uvicorn.run` 绑定端口前就 `import onnxruntime` 崩溃 → 端口永不监听 → `poll_alive` 空转超时（约 27s×3 轮），前端扫描反复 `ensure` 成 ~87s 死循环。预检前置后，缺依赖在 `spawn` 前即报「请创建 .venv-vcr 并 pip install -r python/requirements.txt」的明确错误。
- **违规症状**：开发包扫描无限刷 `[PERF] ensure 请求`/`poll_alive 超时`；`[VCR:py] ModuleNotFoundError: No module named 'onnxruntime'`；单次 `ensure` 数十秒。
- **来源**：`bug-notes.json` BUG-2026-1112-002；`src-tauri/src/vision.rs` 的 `resolve_vcr_interpreter` / `VCR_INTERP_PROBE`。
- **记录日期**：2026-11-12
