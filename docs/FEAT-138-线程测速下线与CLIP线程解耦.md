# FEAT-138 · 删掉「线程测速」扫档，CLIP 线程数与主链路解耦

> **状态**：`done`（2026-11-02）
> **触发**：用户看到设置里「线程测速 … 最长约 6 分钟」的按钮，问「这个多线程是干嘛的？
> 如果是 CPU 语义模型多线程就去掉，默认取用户线程数的一半即可」

---

## 一、那个按钮在做什么

「线程测速」调 `POST /benchmark_sweep`，对 **CLIP 图塔（clip_vision）** 依次用
`1/2/4/6/8/12/16` 七档线程数各跑 8 次（+2 次预热），给出「哪档最快」，
再让用户点「采用」——而**采用写的是全局 CPU 线程数**（`POST /threads`）。

两个问题：

1. **测的对象错了**：CLIP 是扫描里最次要的腿（album=126：776 张只花 224ms，占 **0.2%**），
   而它调的是 det / face / ocr 共同使用的 `intra_op` 线程数。
2. **单跑曲线天然会推荐「把核全给我」**：CLIP 独占时线程越多越快
   （本机批 8：1→300、6→72、12→48 ms/张，16 线程反而崩到 119），
   按这条曲线调主链路是负收益。

代价还是 6 分钟 + 后端不可取消。

## 二、改法

| 项 | 之前 | 现在 |
|---|---|---|
| 线程扫档 UI / 端点 / 命令 | `benchmark_sweep`（前端 → Rust → Python 三层） | **整层删除**（`/benchmark_sweep` → 404） |
| CLIP 会话线程数 | 跟随全局 `intra_op`（用户设 4 就用 4，设 12 就用 12） | **独立** `config.clip_threads()`：用户线程数的一半，**夹在 [4, 用户线程数] 内** |
| 可调性 | 用户点按钮扫档 | 不给 UI（`VCR_CLIP_THREADS` 仅供对照实验） |

映射实测（`config.clip_threads()`）：

```
用户线程  1 → 1    2 → 2    4 → 4    6 → 4    8 → 4    12 → 6    16 → 8
```

## 三、为什么要加「不低于 4」的下限

`/embed_batch` 32 张缩略图，**每档都核实过 32/32 真的出了向量**（这个验证很关键：

`/embed_batch` 对单张失败是内联返回 error、不报 500，只看墙钟会把「一半的图没编码」
误读成「变快了」——第一轮 e2e 就出现过这种矛盾读数，作废重测）：

| CLIP 线程 | 墙钟 | ms/张 |
|---|---|---|
| 2 | 6.82 s | **213.2** |
| 4 | 4.79 s | 149.7 |
| 8 | 3.39 s | 106.1 |

语义腿在**全新建档**时是关键路径：首次导入 4669 张，8 线程约 8 分钟、2 线程约 17 分钟。
而 AI 腿加速后每张只要 ~88ms —— 卡在 2 线程会把整次导入拖成 2 倍。
所以「一半」只在大线程数下生效；小线程数（4/6/8）保持 4，不再往下砍。

## 四、代码落点

| 文件 | 改动 |
|---|---|
| `python/vcr/config.py` | 新增 `clip_threads()`（一半 + `[4, 全局]` 夹取，`VCR_CLIP_THREADS` 可覆盖） |
| `python/vcr/model_registry.py` | `_so_clip()` 用 `config.clip_threads()`，不再继承全局线程数 |
| `python/vcr/model_registry.py` | 删 `benchmark_sweep()` |
| `python/server.py` | 删 `/benchmark_sweep` 端点与 `SweepRequest` |
| `src-tauri/src/vision.rs` / `vcr_settings.rs` / `lib.rs` | 删 `vcr_benchmark_sweep` / `benchmark_vcr_sweep` 命令 / `sweep_brief` / 两处注册 |
| `src/components/settings/BenchmarkSection.vue` | 删扫档按钮、结果表、「采用」、sweep 状态与样式 |
| `src/stores/content.ts` / `src/types/content.ts` / `utils/withTimeout.ts` | 删 `benchmarkVcrSweep` / `VcrSweepEntry` / `VcrSweepResult` / `PERF_TIMEOUT.sweep` |
| `src/components/settings/ThreadsSection.vue` | 文案改为「语义索引自己用这个数的一半」，去掉「去线程测速跑一次」 |

## 五、验收

| 项 | 结果 |
|---|---|
| `cargo test --lib` | 427 passed, 0 failed |
| `vue-tsc --noEmit` | 0 error |
| 会话实测（用户线程=4） | `clip_vision` intra_op **4**、`clip_text` **4**、`det` **4** |
| `/benchmark_sweep` | HTTP **404**（入口已删） |
| 语义产出 | 32/32 全部出向量（三种线程数都核过） |

## 六、残留

| # | 项 | 说明 |
|---|---|---|
| R1 | 大线程数（12/16）下 CLIP 仍会拿一半 | 这是刻意的：`min(全局, max(4, 一半))`，不会超过用户给的额度 |
| R2 | 语义腿在「首次大库导入」仍是关键路径 | 与线程数无关的结构性问题（CLIP 编码量摆在那）；真要再快得走量化或 NPU |
| R3 | `VCR_CLIP_THREADS` 不是 UI 旋钮，只在文档/探针里用 | 不再鼓励用户手工调这个数 |
