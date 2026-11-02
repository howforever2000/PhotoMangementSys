"""端点探针：用户填完地址后，点「测试连接」要知道"到底通不通、通到什么程度"。

## 为什么要探针而不是让用户直接跑任务

自填端点的差异全在细节上：少写 `/v1`、`/images/edits` 不支持、
返回 `url` 而不是 `b64_json`、模型名大小写不对、要异步轮询……
这些混在一次真实任务里失败，用户只会看到"出图失败"，无从下手。

## 探针做什么 / 不做什么

做（免费、几秒）：
  1. 参数自检（地址/模型名/Key 是否齐）；
  2. `GET /models` —— 有就列出可用模型名（帮用户改名）；
  3. 对话端点发一次极小 ping（`max_tokens=1`）—— 这是最可靠的"活着"证据；
  4. 图像端点**不默认出图**（会花钱）。

不做（默认）：真的生成一张图。要看这一项必须显式 `deep=True`
（UI 上写清"会花一次钱"），因为用户不该为了一次连通性检查被扣费。
"""
from __future__ import annotations

import time

import httpx

from .base import ERR_BAD_SHAPE, Endpoint, ProviderError
from .openai_compat import OpenAICompat

# 探针的超时要比真实调用短：探针是给人等的
PROBE_TIMEOUT = 20.0


async def probe(
    endpoint: Endpoint,
    *,
    deep: bool = False,
    transport: httpx.AsyncBaseTransport | None = None,
) -> dict:
    """返回 camelCase 报告（Rust 原样转给前端）。"""
    ep = endpoint.normalize()
    report: dict = {
        "ok": False,
        "baseUrl": ep.base_url,
        "model": ep.model,
        "kind": ep.kind,
        "checks": [],
        "models": [],
        "error": None,
    }

    # ① 参数自检
    try:
        ep.validate()
        report["checks"].append({"name": "参数", "ok": True, "detail": "地址/模型名/Key 齐全"})
    except ProviderError as e:
        report["checks"].append({"name": "参数", "ok": False, "detail": str(e), "kind": e.kind})
        report["error"] = e.to_dict()
        return report

    client = OpenAICompat(ep, transport=transport, timeout=PROBE_TIMEOUT)

    # ② 模型列表（可选信息：没有这个路由不算失败）
    t0 = time.monotonic()
    models = await client.list_models()
    report["models"] = models
    ms = int((time.monotonic() - t0) * 1000)
    if models:
        report["checks"].append(
            {"name": "模型列表", "ok": True, "detail": f"端点返回 {len(models)} 个模型", "ms": ms}
        )
    else:
        report["checks"].append(
            {
                "name": "模型列表",
                "ok": True,
                "detail": "端点没有 /models（很多兼容端点不实现，不影响出图）",
                "ms": ms,
                "optional": True,
            }
        )

    # ③ 对话 ping（图像专用端点跳过：它们没有 /chat/completions）
    if ep.kind in ("planner", "both"):
        t0 = time.monotonic()
        try:
            await client.chat(
                [{"role": "user", "content": "ping"}], max_tokens=1, temperature=0
            )
            report["checks"].append(
                {
                    "name": "对话端点",
                    "ok": True,
                    "detail": "chat/completions 可用",
                    "ms": int((time.monotonic() - t0) * 1000),
                }
            )
        except ProviderError as e:
            report["checks"].append(
                {
                    "name": "对话端点",
                    "ok": False,
                    "detail": str(e),
                    "kind": e.kind,
                    "ms": int((time.monotonic() - t0) * 1000),
                }
            )
    else:
        report["checks"].append(
            {
                "name": "对话端点",
                "ok": True,
                "detail": "图像端点不需要对话接口，已跳过",
                "optional": True,
            }
        )

    # ④ 真出一张（只有 deep 才做，且会花钱）
    if deep and ep.kind in ("image", "both"):
        t0 = time.monotonic()
        try:
            from .base import EditRequest

            res = await client.image_edit(
                EditRequest(images=[], prompt="a single red circle on white", size="1024x1024")
            )
            report["checks"].append(
                {
                    "name": "出图",
                    "ok": True,
                    "detail": f"{len(res.image_bytes)} 字节，路径 {res.route}",
                    "ms": int((time.monotonic() - t0) * 1000),
                }
            )
            report["sampleBytes"] = len(res.image_bytes)
            report["route"] = res.route
        except ProviderError as e:
            report["checks"].append(
                {
                    "name": "出图",
                    "ok": False,
                    "detail": str(e),
                    "kind": e.kind,
                    "ms": int((time.monotonic() - t0) * 1000),
                }
            )
    elif ep.kind in ("image", "both"):
        report["checks"].append(
            {
                "name": "出图",
                "ok": True,
                "detail": "未实际出图（会花钱）。确认连接可用后，首个任务会验证这一项。",
                "optional": True,
            }
        )

    # 判定：必做项（非 optional）全过才算 ok
    required = [c for c in report["checks"] if not c.get("optional")]
    report["ok"] = all(c["ok"] for c in required) and bool(required)
    failed = [c for c in required if not c["ok"]]
    if failed:
        report["error"] = {
            "kind": failed[0].get("kind") or ERR_BAD_SHAPE,
            "message": failed[0]["detail"],
        }
    return report
