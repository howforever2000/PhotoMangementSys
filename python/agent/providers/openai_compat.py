"""OpenAI 兼容客户端：一份代码同时服务图像编辑与规划对话。

覆盖三类真实端点行为（都有 MockTransport 单测，不需要网络）：
1. `POST /images/edits`（multipart，图生图）；
2. `POST /images/generations`（json，文生图）；
3. 异步任务：提交拿到 task_id → 轮询 `/tasks/{id}` → completed 后取 url/b64。

响应形状在不同厂商间差异很大，所以**解析要宽容、判断要严格**：
`data[0].b64_json` / `data[0].url` / `result.url` / `output[0].url` 都认，
但一个都认不出来时必须报 `bad_shape` 并带上响应片段 —— 否则用户只会看到
"出图失败"，无从判断是地址不对还是模型名不对。
"""
from __future__ import annotations

import asyncio
import base64
import time

import httpx

from .base import (
    ERR_AUTH,
    ERR_BAD_SHAPE,
    ERR_NETWORK,
    ERR_RATE_LIMIT,
    ERR_SERVER,
    ERR_TIMEOUT,
    EditRequest,
    EditResult,
    Endpoint,
    ProviderError,
)

DEFAULT_TIMEOUT = 120.0
# 异步任务的轮询参数：最多 90 次 × 3s = 4.5 分钟（出 2K 图可能要一两分钟）
POLL_INTERVAL = 3.0
POLL_MAX = 90


class OpenAICompat:
    """用户自填端点的客户端。`transport` 只给测试注入（MockTransport）。"""

    def __init__(
        self,
        endpoint: Endpoint,
        *,
        transport: httpx.AsyncBaseTransport | None = None,
        timeout: float = DEFAULT_TIMEOUT,
        poll_interval: float = POLL_INTERVAL,
        poll_max: int = POLL_MAX,
    ):
        self.ep = endpoint.normalize()
        self.ep.validate()
        self._transport = transport
        self._timeout = timeout
        self._poll_interval = poll_interval
        self._poll_max = poll_max

    # ---- 内部 ----

    def _client(self) -> httpx.AsyncClient:
        return httpx.AsyncClient(
            timeout=self._timeout,
            transport=self._transport,
            headers=self._headers(),
        )

    def _headers(self) -> dict[str, str]:
        # ⚠️ 不要在这里写 Content-Type：multipart 上传（/images/edits）必须由 httpx
        # 自己生成带 boundary 的 multipart 头。写死 application/json 会让真端点
        # 直接 400（单测里就是被这一行拦下的）。JSON 请求用 json= 参数自动带头。
        h: dict[str, str] = {}
        if self.ep.api_key:
            h["Authorization"] = f"Bearer {self.ep.api_key}"
        return h

    @staticmethod
    def _classify(exc: Exception) -> ProviderError:
        if isinstance(exc, httpx.TimeoutException):
            return ProviderError(ERR_TIMEOUT, "请求超时（模型端响应太慢或地址不通）")
        return ProviderError(ERR_NETWORK, f"连不上端点：{exc}")

    @staticmethod
    def _raise_for_status(resp: httpx.Response, where: str) -> None:
        code = resp.status_code
        if code < 400:
            return
        snippet = (resp.text or "")[:300]
        if code in (401, 403):
            raise ProviderError(ERR_AUTH, f"{where}: Key 无效或无权限（HTTP {code}）：{snippet}", code)
        if code == 429:
            raise ProviderError(ERR_RATE_LIMIT, f"{where}: 触发限流（HTTP 429），请稍后重试", code)
        if code == 404:
            raise ProviderError(
                ERR_BAD_SHAPE,
                f"{where}: 端点不存在（HTTP 404）。地址应指向 OpenAI 兼容根路径，例如 https://host/v1",
                code,
            )
        if code >= 500:
            raise ProviderError(ERR_SERVER, f"{where}: 服务端错误（HTTP {code}）：{snippet}", code)
        raise ProviderError(ERR_BAD_SHAPE, f"{where}: 请求被拒（HTTP {code}）：{snippet}", code)

    @staticmethod
    def _decode_b64(s: str) -> bytes:
        # 兼容 data:image/png;base64,xxxx 前缀
        if s.startswith("data:"):
            s = s.split(",", 1)[-1]
        try:
            return base64.b64decode(s)
        except (ValueError, TypeError) as e:
            raise ProviderError(ERR_BAD_SHAPE, f"返回的 b64 数据无法解码：{e}") from e

    @staticmethod
    def _extract_images(body: dict) -> list[dict]:
        """把所有常见的"图片数组"形状拉平。空数组 = 形状不认识。"""
        for key in ("data", "output", "images", "result"):
            v = body.get(key)
            if isinstance(v, dict):
                v = v.get("images") or v.get("data")
            if isinstance(v, list) and v:
                items = [x for x in v if isinstance(x, dict)]
                if items:
                    return items
        return []

    async def _fetch_url(self, client: httpx.AsyncClient, url: str) -> bytes:
        try:
            r = await client.get(url)
        except Exception as e:  # noqa: BLE001
            raise self._classify(e) from e
        self._raise_for_status(r, "下载结果图")
        if not r.content:
            raise ProviderError(ERR_BAD_SHAPE, "结果图 URL 返回空内容")
        return r.content

    # ---- 图像 ----

    async def image_edit(self, req: EditRequest) -> EditResult:
        """图生图 / 文生图。图 > 0 走 edits，否则走 generations。"""
        started = time.monotonic()
        route = "edits" if req.images else "generations"
        try:
            async with self._client() as client:
                if route == "edits":
                    files = [
                        ("image[]", (f"in{i}.png", img, "image/png"))
                        for i, img in enumerate(req.images)
                    ]
                    data = {"model": self.ep.model, "prompt": req.prompt, "n": "1"}
                    if req.size:
                        data["size"] = req.size
                    resp = await client.post(
                        f"{self.ep.base_url}/images/edits", data=data, files=files
                    )
                else:
                    payload = {"model": self.ep.model, "prompt": req.prompt, "n": 1}
                    if req.size:
                        payload["size"] = req.size
                    resp = await client.post(
                        f"{self.ep.base_url}/images/generations", json=payload
                    )
                self._raise_for_status(resp, f"图像 {route}")
                body = self._json(resp)

                if self.ep.async_poll or _looks_async(body):
                    return await self._poll_task(client, body, route, started)

                images = self._extract_images(body)
                if not images:
                    raise ProviderError(
                        ERR_BAD_SHAPE,
                        f"返回体里找不到图片（可能是地址不是图像端点，或模型名不对）：{str(body)[:200]}",
                    )
                first = images[0]
                if first.get("b64_json"):
                    data_bytes = self._decode_b64(first["b64_json"])
                elif first.get("url"):
                    data_bytes = await self._fetch_url(client, first["url"])
                else:
                    raise ProviderError(ERR_BAD_SHAPE, f"图片项既无 b64_json 也无 url：{str(first)[:160]}")
                return EditResult(
                    image_bytes=data_bytes,
                    cost=_num(body.get("cost")),
                    model=self.ep.model,
                    latency_ms=int((time.monotonic() - started) * 1000),
                    route=route,
                )
        except ProviderError:
            raise
        except Exception as e:  # noqa: BLE001
            raise self._classify(e) from e

    async def _poll_task(
        self, client: httpx.AsyncClient, body: dict, route: str, started: float
    ) -> EditResult:
        task_id = body.get("task_id") or body.get("id") or (body.get("data") or {}).get("task_id")
        if not task_id:
            raise ProviderError(ERR_BAD_SHAPE, f"异步模式需要 task_id，返回体没有：{str(body)[:200]}")
        for _ in range(self._poll_max):
            await asyncio.sleep(self._poll_interval)
            resp = await client.get(f"{self.ep.base_url}/tasks/{task_id}", headers=self._headers())
            self._raise_for_status(resp, "轮询任务")
            data = self._json(resp)
            status = str(data.get("status") or "").lower()
            if status in ("completed", "succeeded", "success", "done"):
                images = self._extract_images(data)
                if not images:
                    raise ProviderError(ERR_BAD_SHAPE, f"任务完成但没有图片：{str(data)[:200]}")
                first = images[0]
                if first.get("b64_json"):
                    out = self._decode_b64(first["b64_json"])
                elif first.get("url"):
                    out = await self._fetch_url(client, first["url"])
                else:
                    raise ProviderError(ERR_BAD_SHAPE, "任务完成的图片项没有 url/b64_json")
                return EditResult(
                    image_bytes=out,
                    cost=_num(data.get("cost")),
                    model=self.ep.model,
                    latency_ms=int((time.monotonic() - started) * 1000),
                    route=f"{route}+poll",
                )
            if status in ("failed", "error", "cancelled"):
                raise ProviderError(
                    ERR_SERVER, f"任务失败：{data.get('error') or data.get('message') or status}"
                )
        raise ProviderError(ERR_TIMEOUT, f"异步任务 {task_id} 轮询超时（{self._poll_max} 次）")

    # ---- 对话（规划层） ----

    async def chat(
        self,
        messages: list[dict],
        *,
        tools: list[dict] | None = None,
        tool_choice: str | dict | None = None,
        max_tokens: int = 800,
        temperature: float = 0.2,
    ) -> dict:
        payload: dict = {
            "model": self.ep.model,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": temperature,
        }
        if tools:
            payload["tools"] = tools
        if tool_choice is not None:
            payload["tool_choice"] = tool_choice
        try:
            async with self._client() as client:
                resp = await client.post(f"{self.ep.base_url}/chat/completions", json=payload)
                self._raise_for_status(resp, "规划对话")
                return self._json(resp)
        except ProviderError:
            raise
        except Exception as e:  # noqa: BLE001
            raise self._classify(e) from e

    async def list_models(self) -> list[str]:
        """`GET /models`（不是所有端点都有）。失败只返回空表，不抛。"""
        try:
            async with self._client() as client:
                resp = await client.get(f"{self.ep.base_url}/models")
                if resp.status_code >= 400:
                    return []
                body = self._json(resp)
        except Exception:  # noqa: BLE001 —— 探针里的可选信息，失败不该让探测失败
            return []
        items = body.get("data") if isinstance(body, dict) else None
        if not isinstance(items, list):
            return []
        out: list[str] = []
        for it in items:
            if isinstance(it, dict) and it.get("id"):
                out.append(str(it["id"]))
        return out[:200]

    @staticmethod
    def _json(resp: httpx.Response) -> dict:
        try:
            body = resp.json()
        except Exception as e:  # noqa: BLE001
            raise ProviderError(
                ERR_BAD_SHAPE,
                f"返回体不是 JSON（地址可能指向了网页而不是 API）：{(resp.text or '')[:160]}",
                resp.status_code,
            ) from e
        if not isinstance(body, dict):
            raise ProviderError(ERR_BAD_SHAPE, f"返回体不是对象：{str(body)[:160]}")
        return body


def _looks_async(body: dict) -> bool:
    """返回体像"已受理"而不是"已完成"：有 task_id 且没有图片数组。"""
    data = body.get("data")
    inner = data.get("task_id") if isinstance(data, dict) else None
    has_task = bool(body.get("task_id") or inner)
    return has_task and not OpenAICompat._extract_images(body)


def _num(v) -> float | None:
    try:
        return float(v) if v is not None else None
    except (TypeError, ValueError):
        return None


def image_content_parts(images: list[bytes], mime: str = "image/png") -> list[dict]:
    """把图 bytes 变成 OpenAI vision 的 content parts（多模态额度合并计数的落地）。"""
    parts: list[dict] = []
    for img in images:
        b64 = base64.b64encode(img).decode("ascii")
        parts.append(
            {"type": "image_url", "image_url": {"url": f"data:{mime};base64,{b64}"}}
        )
    return parts
