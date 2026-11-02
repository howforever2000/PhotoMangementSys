# 模型端点探针记录（agent-provider-probe）

> 状态：**待真实端点实测**（本仓库开发环境没有可用的 provider Key）。
> 代码侧的形状覆盖已完成（见文末清单），本文只负责记录"真实端点到底怎么回话"。

## 为什么需要这份记录

用户决定的接入方式是 **不预设 provider、自己填地址 + Key**（见 `docs/v5.md` §25 修订）。
于是"端点行为"从"我们写死"变成"运行时才知道"，必须留一份实测台账，
否则每次出问题都要重新猜一遍：

- 返回 `b64_json` 还是 `url`？
- 需不需要异步轮询（`task_id`）？轮询路由是 `/tasks/{id}` 还是别的？
- `size` 参数收什么格式？支不支持 2048？
- 有没有 `cost` 字段（决定"已花金额"是真实值还是估算）？
- `image[]` 多图上传支持几张？
- 规划端点是否支持 `tools` / `tool_choice`（不支持的话 P1b 无法工作）？

## 怎么跑

### 路线 A：应用内（推荐）

1. 工坊 →「AI 创作」→ 左栏勾选**启用 AI 创作**；
2. 展开**模型服务** → 添加端点：地址、模型名、Key、用途（出图 / 规划 / 两者都行）、单价；
3. 点**保存**（保存后会自动测一次），或点**测试连接**。

探针会逐项报告：参数自检 → `/models` 列表 → 对话端点 ping。
**默认不真的出图**（会花钱）。确认连通后再跑一个真实任务验证出图路径。

### 路线 B：直接打 HTTP（排障用）

```powershell
# 需要先让侧车跑起来（应用里进一次工坊即可），或手动起：
#   $env:AGENT_PORT=8791; $env:AGENT_TOKEN=dev; python python/agent_server.py

$body = @{
  endpoint = @{
    baseUrl = "https://你的端点/v1"
    apiKey  = "sk-..."
    model   = "你的模型名"
    kind    = "both"      # image | planner | both
  }
  deep = $false           # $true 会真的出一张图（花钱）
} | ConvertTo-Json -Depth 6

Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:8791/agent/probe" `
  -Headers @{ "X-Agent-Token" = "dev" } -ContentType "application/json" -Body $body |
  ConvertTo-Json -Depth 8
```

## 待填台账

| 端点 | 模型 | 用途 | 返回形状 | 异步 | `size` 上限 | `cost` 字段 | 工具调用 | 实测人/日期 |
|---|---|---|---|---|---|---|---|---|
| （待填） | | | b64 / url | 是/否 | | 有/无 | 支持/不支持 | |

## 代码侧已覆盖的形状（`python/tests/test_providers.py`）

这些分支已经有 MockTransport 测试，真实端点只要落在其中一种就能用：

| 场景 | 断言 |
|---|---|
| 同步 `b64_json` | 返回体解析 + 走 `multipart/form-data` |
| 同步 `url` | 二次下载图片字节 |
| 异步轮询 | `task_id` → 轮询 `running` → `completed` → 取图；记录 `route=generations+poll` |
| 异步失败 | `status=failed` 带 error 文案 |
| 401 / 429 / 404 / 5xx | 分类成 `auth` / `rate_limit` / `bad_shape`（带"应指向 /v1"提示）/ `server` |
| 回复是网页（地址填错） | `bad_shape` + "不是 JSON" |
| 返回体里没有图片数组 | `bad_shape` + 响应片段 |

> 踩过的一个真 bug：早期客户端给**所有**请求都加了 `Content-Type: application/json`，
> 导致 multipart 上传没有 boundary，真端点会 400。单测（`test_sync_b64_response`）
> 就是被这一行拦下的 —— 所以探针必须复用真实客户端代码，而不是另写一套 HTTP 探测。

## 实测后要回填到代码的地方

1. **异步路由**：如果端点的轮询路由不是 `/tasks/{id}`，需要在
   `providers/openai_compat.py::_poll_task` 里加分支（并把 URL 记进本文）；
2. **`size` 上限**：若端点不支持 2048，调整 `providers/registry.py::TIER_MAP`；
3. **多图上传**：若 `image[]` 只收 1 张，`style_transfer_remote` 需要改成逐张单图；
4. **`cost` 缺失**：`EditResult.cost` 为 `None` → 台账落 `estimated=1`（已实现，无需改）；
5. **工具调用不支持**：P1b 无法工作 → 该端点只能当出图端点（`kind=image`），
   规划另配一个支持 `tools` 的端点（本地 Ollama / llama.cpp 的 `/v1` 通常支持）。
