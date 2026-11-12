# PhotoMangementSys 单项目知识库

> 本目录是本项目的**唯一项目级知识库**（与全局库 `~/.dsh/knowledge/`、`~/.dsh/skills/` 分离）。目标是让编码 agent 用最少 token 拿到「改哪里、有什么坑、事实在哪」，而不是每次重读全项目。

## 目录结构

```
.dsh/
├── README.md            ← 本文件（导航 + 使用约定）
├── overview.md          ← 项目概览：定位 / 功能特性 / 技术栈 / 目录 / 构建
├── architecture.md      ← 架构：三层进程 / VCR 分层 / 数据模型 / 关键数据流
├── features/
│   ├── index.md         ← 全量功能索引（150 条，ID｜状态｜名称）+ 主题速查
│   ├── album-management.md
│   ├── scan-and-ai.md
│   ├── semantic-search.md
│   ├── person.md
│   ├── workshop.md
│   ├── ui-and-theme.md
│   └── devtools.md
└── knowledge/           ← 避错经验（避错四段，来源均可回溯）
    ├── python-sidecar-lifecycle.md
    ├── thumbnail-cache.md
    ├── gpu-dml-constraints.md
    ├── frontend-style-pitfalls.md
    ├── data-path-and-packaging.md
    ├── scan-pipeline.md
    └── ui-contrast-tokens.md
```

## 事实源（不要在本库复制、也不要去改它们）

- `feature-notes.json`（150 条功能）与 `bug-notes.json`（151 条缺陷）是**唯一事实源**。本库只是它们的索引与蒸馏，冲突时以 JSON 为准。
- `docs/`（FEAT-* 技术文档、扫描入库优化清单、推理后端多平台化 B 方案）、`.pi/`（plan / probes / search_eval）为补充材料。
- `README.md`（仓库根）、`design/design-system.md` 为项目自述与设计系统。

## 使用约定（给接手的 agent）

1. **先看 `overview.md` 与 `architecture.md`**（短文件，全读）建立整体认知。
2. **定位功能**：在 `features/index.md` 搜关键词/模块 → 命中 FEAT-ID → 打开对应域文档看入口文件与坑。
3. **改代码前**：在 `knowledge/` 里按主题检索是否已有避错条目（尤其改 Python 服务、缩略图、GPU/NPU、样式、打包路径、扫描流程、对比度）。
4. **每条经验都标了来源**：单项目来源给 `bug-notes.json` 的 BUG-ID，或「文件:行号」；标不出来源的视为通用知识，不冒充项目经验。
5. **知识回流**：任务结束若有新坑/新做法，按避错四段（触发条件 / 先做 / 为什么 / 违规症状 / 来源 / 记录日期）写回对应文件；仅本项目成立的写本目录，跨项目可复用的写全局库。不重复写入，更新原条目标注日期。

## 维护状态

- 生成日期：2026-11-12；基线版本 v0.4.5（2026-11-01）。
- 覆盖：150 条功能全量索引、7 个功能域、7 类高频避坑（共 43 条经验）。
- 占位约定：未知内容标「待填写」，不臆造。
