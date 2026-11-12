# 扫描入库流程

> 组合扫描（EXIF / 影调 / AI 三合一）+ 后台收尾的并发与落库纪律。均来自 `bug-notes.json`。

## 部分勾选 + UPSERT 全字段覆盖 = 未勾选字段被清空

- **触发条件**：组合扫描多选项，某项未勾选时对应字段为 `None`。
- **先做**：凡「本次未采集」的可选字段，更新时一律用 `COALESCE` 保原值；逐字段确认「未勾选」的落库语义（保留 / 清空 / 跳过），不能默认全字段覆盖。
- **为什么**：`photo_content_scan` upsert 是 `ON CONFLICT DO UPDATE SET ... tone_type=excluded.tone_type` 全字段覆盖。
- **违规症状**：勾 AI 不勾影调时，已入库影调字段被覆盖为 `NULL`，数据静默丢失。
- **来源**：`bug-notes.json` BUG-2026-0910-007（`src-tauri/src/db/content.rs`、`content.rs`）。
- **记录日期**：2026-11-12

## 向量 `model` 标签写入侧与读取侧必须一致；宁可失败不静默写错标

- **触发条件**：语义扫描落库贴 `model` 标识，查询侧按 `WHERE model = ?` 精确等值匹配。
- **先做**：删掉写死的 fp16 兜底（`clip_model_id` 失败即**中止并明确报错**，绝不静默写错标）；新增 `index_model_mismatch` 状态显性化失配（`indexed==0 && stale>0` 时告知「N 条向量出自其它档位，无法参与检索」）并给重建入口（`rebuild_desc_index`）。
- **为什么**：写入贴 fp16 标签而实际档位是 fp32 → 查询侧永不匹配 → 向量永久不可读且系统不作任何提示。
- **违规症状**：「向量在库里却一条都搜不出」；语义搜索静默失效。
- **来源**：`bug-notes.json` BUG-2026-0921-004（`content.rs`、`db/embedding.rs`、`textdesc.rs`）。
- **记录日期**：2026-11-12

## 单飞锁 + 需求位：先放锁后复查 + CAS 抢回；RAII 兜底 panic

- **触发条件**：后台收尾任务用单飞（`running` 标志 + 需求位）。
- **先做**：`running.store(false)` 之后**再** load 需求位，若非空则 `compare_exchange(false,true)` 抢回继续跑一轮（CAS 失败说明别人已接手）；`run_post_scan_tasks` 开头加 `RunningGuard`（`Drop` 时 `store(false)`）兜底 panic。
- **为什么**：消费端「查需求 → 放锁」之间的窗口内生产端登记的需求会丢（lost-update）；detached 异步任务 panic 不释放 `running`。
- **违规症状**：需求位丢失；`running` 永久卡 `true`。
- **来源**：`bug-notes.json` BUG-2026-0929-001（open；`content.rs`）。
- **记录日期**：2026-11-12

## 有副作用的耗时命令必须在后端单飞（前端 `running` 不是并发控制）

- **触发条件**：同一相册扫描可被重复 / 并行触发。
- **先做**：后端 `scan_album_combined` 加 `try_lock` 单飞（失败明确拒绝、返回原因，不静默排队）；长任务收尾阶段上报进度（或 UI 显示「收尾中」）。
- **为什么**：前端守卫（`combinedJobs[albumId].running` / `globalScanJob.running` / `runningSingle`）全是 JS 内存布尔，只在单个 Pinia 实例内有效，多开 / 重载 / 多进程立刻失效；进度条 100% 与真实返回之间隔着十几秒收尾段。
- **违规症状**：同一相册被完整扫两遍；用户误判完成重复点击；`[AOP:RET]` 耗时 ≠ 命令返回前端（本例差 9.9~16.8s）。
- **来源**：`bug-notes.json` BUG-2026-1027-006（`content.rs`、`category.rs`、`src/stores/content.ts`）。
- **记录日期**：2026-11-12

## 阶段重排要核对下游消费方；回退功能块要画调用点

- **触发条件**：重排扫描阶段（如 T1 去掉 `do_basic && !do_ai` 守卫）；回退功能块（T3 回退连带删掉 `rebuild_text_index` 调用点）。
- **先做**：重排前列出「被重排的产物在下游还有没有消费方」；回退前列「哪些调用点是我引入的、哪些是借我的通道」；调用点类缺陷用**源码断言测试**（grep 数调用次数）锁住。
- **为什么**：EXIF 被读两遍且丢一份；描述索引重建零调用点 → 扫完的照片用时间 / 地点搜不到，且不报错不崩溃。
- **违规症状**：EXIF 重复读；扫描后描述索引不重建、只静默表现为「搜不到」。
- **来源**：`bug-notes.json` BUG-2026-0929-004、BUG-2026-0929-009、BUG-2026-0929-006。
- **记录日期**：2026-11-12

## 并发段耗时：起止都要在 future 内部读

- **触发条件**：三腿并发 `tokio::join!` 后用 `span_with(name, 起点)` 记耗时。
- **先做**：三腿 future 各返回 `(t_start, t_end, 结果)`，join 后统一用 `span_ms` 记差值；`scan_timing.rs` 模块头写明这条纪律。
- **为什么**：`span_with(name, 起点)` 的 `elapsed()` = 起点到现在 = 最慢那条腿的时长，三条腿读出三个几乎一样的数。
- **违规症状**：三腿耗时全等于 join 时长（195529/195529/195489ms），无法归因并发瓶颈。
- **来源**：`bug-notes.json` BUG-2026-0928-003（`content.rs`、`scan_timing.rs`）。
- **记录日期**：2026-11-12
