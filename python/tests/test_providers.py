"""模型通道：OpenAI 兼容客户端 + 端点探针（全部走 httpx.MockTransport，不出网）。

覆盖的是"自填端点"这个决定带来的全部真实形状差异：
返回 b64 / 返回 url / 异步轮询 / 任务失败 / 401 / 429 / 返回网页而不是 JSON。
这些分支如果没有测试，出问题时用户只会看到一句"出图失败"。
"""
from __future__ import annotations

import base64
import json

import httpx
import pytest

from agent.providers.base import EditRequest, Endpoint, ProviderError
from agent.providers.openai_compat import OpenAICompat
from agent.providers.probe import probe


def ep(**kw) -> Endpoint:
    base = {
        "base_url": "https://api.example.com/v1",
        "api_key": "sk-test-1234567890",
        "model": "img-model",
        "kind": "image",
    }
    base.update(kw)
    return Endpoint(**base)


def transport(handler) -> httpx.MockTransport:
    return httpx.MockTransport(handler)


PNG_1PX = base64.b64decode(
    # 1×1 透明 PNG
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=="
)


# ------------------------------------------------------------------ 图像


@pytest.mark.asyncio
async def test_sync_b64_response():
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path.endswith("/images/edits")
        assert "multipart/form-data" in request.headers.get("content-type", "")
        return httpx.Response(200, json={"data": [{"b64_json": base64.b64encode(PNG_1PX).decode()}]})

    client = OpenAICompat(ep(), transport=transport(handler))
    res = await client.image_edit(EditRequest(images=[b"fake"], prompt="warm"))
    assert res.image_bytes == PNG_1PX
    assert res.route == "edits"


@pytest.mark.asyncio
async def test_sync_url_response_downloads_bytes():
    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/images/generations"):
            return httpx.Response(200, json={"data": [{"url": "https://cdn.example.com/a.png"}]})
        if request.url.host == "cdn.example.com":
            return httpx.Response(200, content=PNG_1PX)
        return httpx.Response(404)

    client = OpenAICompat(ep(), transport=transport(handler))
    res = await client.image_edit(EditRequest(images=[], prompt="a cat"))
    assert res.image_bytes == PNG_1PX
    assert res.route == "generations"


@pytest.mark.asyncio
async def test_async_task_polling():
    calls = {"poll": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/images/generations"):
            return httpx.Response(200, json={"task_id": "t-1", "status": "pending"})
        if "/tasks/t-1" in request.url.path:
            calls["poll"] += 1
            if calls["poll"] < 2:
                return httpx.Response(200, json={"status": "running"})
            return httpx.Response(
                200,
                json={
                    "status": "completed",
                    "result": {"images": [{"b64_json": base64.b64encode(PNG_1PX).decode()}]},
                    "cost": 0.15,
                },
            )
        return httpx.Response(404)

    client = OpenAICompat(
        ep(async_poll=True), transport=transport(handler), poll_interval=0.0
    )
    res = await client.image_edit(EditRequest(images=[], prompt="x"))
    assert res.image_bytes == PNG_1PX
    assert res.route == "generations+poll"
    assert res.cost == pytest.approx(0.15)
    assert calls["poll"] == 2


@pytest.mark.asyncio
async def test_async_task_failed():
    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/images/generations"):
            return httpx.Response(200, json={"task_id": "t-2"})
        return httpx.Response(200, json={"status": "failed", "error": "content policy"})

    client = OpenAICompat(ep(async_poll=True), transport=transport(handler), poll_interval=0.0)
    with pytest.raises(ProviderError) as ei:
        await client.image_edit(EditRequest(images=[], prompt="x"))
    assert "content policy" in str(ei.value)


@pytest.mark.asyncio
async def test_auth_error_is_classified():
    client = OpenAICompat(
        ep(), transport=transport(lambda r: httpx.Response(401, json={"error": "bad key"}))
    )
    with pytest.raises(ProviderError) as ei:
        await client.image_edit(EditRequest(images=[], prompt="x"))
    assert ei.value.kind == "auth"
    assert ei.value.status == 401


@pytest.mark.asyncio
async def test_rate_limit_is_classified():
    client = OpenAICompat(
        ep(), transport=transport(lambda r: httpx.Response(429, json={"error": "slow down"}))
    )
    with pytest.raises(ProviderError) as ei:
        await client.image_edit(EditRequest(images=[], prompt="x"))
    assert ei.value.kind == "rate_limit"


@pytest.mark.asyncio
async def test_html_response_is_bad_shape():
    # 地址少写了 /v1 或指向了网页：这是最常见的一种错，必须给出可读原因
    client = OpenAICompat(
        ep(), transport=transport(lambda r: httpx.Response(200, text="<html>hello</html>"))
    )
    with pytest.raises(ProviderError) as ei:
        await client.image_edit(EditRequest(images=[], prompt="x"))
    assert ei.value.kind == "bad_shape"
    assert "不是 JSON" in str(ei.value)


@pytest.mark.asyncio
async def test_unknown_shape_lists_snippet():
    client = OpenAICompat(
        ep(), transport=transport(lambda r: httpx.Response(200, json={"foo": "bar"}))
    )
    with pytest.raises(ProviderError) as ei:
        await client.image_edit(EditRequest(images=[], prompt="x"))
    assert "找不到图片" in str(ei.value)


@pytest.mark.asyncio
async def test_404_mentions_v1_hint():
    client = OpenAICompat(
        ep(), transport=transport(lambda r: httpx.Response(404, json={"error": "no route"}))
    )
    with pytest.raises(ProviderError) as ei:
        await client.image_edit(EditRequest(images=[], prompt="x"))
    assert "OpenAI 兼容根路径" in str(ei.value)


# ------------------------------------------------------------------ 校验


def test_local_endpoint_without_key_is_allowed():
    # 本地 llama.cpp / Ollama 通常不校验 Key，不该被自己的参数校验拦下
    e = ep(base_url="127.0.0.1:11434/v1", api_key="", model="qwen2.5-vl").normalize()
    assert e.base_url.startswith("http://")
    e.validate()


def test_remote_endpoint_without_key_is_rejected():
    with pytest.raises(ProviderError) as ei:
        ep(api_key="").normalize().validate()
    assert ei.value.kind == "auth"


def test_model_name_required():
    with pytest.raises(ProviderError):
        ep(model="").normalize().validate()


def test_masked_key_does_not_leak_middle():
    m = ep().masked_key
    assert m.startswith("sk-tes")
    assert "1234567890" not in m


# ------------------------------------------------------------------ 规划对话


@pytest.mark.asyncio
async def test_chat_carries_tools_and_parses_choices():
    seen = {}

    def handler(request: httpx.Request) -> httpx.Response:
        seen.update(json.loads(request.content))
        return httpx.Response(
            200,
            json={
                "choices": [
                    {"message": {"role": "assistant", "content": "ok", "tool_calls": []}}
                ],
                "usage": {"total_tokens": 12},
            },
        )

    client = OpenAICompat(ep(kind="planner", model="vl-model"), transport=transport(handler))
    body = await client.chat([{"role": "user", "content": "hi"}], tools=[{"type": "function"}])
    assert body["choices"][0]["message"]["content"] == "ok"
    assert seen["model"] == "vl-model"
    assert seen["tools"] == [{"type": "function"}]


@pytest.mark.asyncio
async def test_list_models_tolerates_missing_route():
    client = OpenAICompat(ep(), transport=transport(lambda r: httpx.Response(404)))
    assert await client.list_models() == []


@pytest.mark.asyncio
async def test_list_models_returns_ids():
    client = OpenAICompat(
        ep(),
        transport=transport(lambda r: httpx.Response(200, json={"data": [{"id": "a"}, {"id": "b"}]})),
    )
    assert await client.list_models() == ["a", "b"]


# ------------------------------------------------------------------ 探针


@pytest.mark.asyncio
async def test_probe_ok_for_planner_endpoint():
    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/models"):
            return httpx.Response(200, json={"data": [{"id": "qwen2.5-vl"}]})
        if request.url.path.endswith("/chat/completions"):
            return httpx.Response(
                200, json={"choices": [{"message": {"content": "pong"}}], "usage": {}}
            )
        return httpx.Response(404)

    report = await probe(
        ep(kind="both", model="qwen2.5-vl"), transport=transport(handler)
    )
    assert report["ok"] is True
    assert report["models"] == ["qwen2.5-vl"]
    names = {c["name"]: c["ok"] for c in report["checks"]}
    assert names["参数"] and names["模型列表"] and names["对话端点"]


@pytest.mark.asyncio
async def test_probe_does_not_generate_images_by_default():
    hits: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        hits.append(request.url.path)
        return httpx.Response(200, json={"data": []})

    report = await probe(ep(kind="image"), transport=transport(handler))
    assert report["ok"] is True  # 图像端点不要求对话接口
    assert not any("/images/" in h for h in hits), "默认探针不得真的出图（会花钱）"


@pytest.mark.asyncio
async def test_probe_reports_auth_failure():
    report = await probe(
        ep(kind="planner"), transport=transport(lambda r: httpx.Response(401, json={}))
    )
    assert report["ok"] is False
    assert report["error"]["kind"] == "auth"


@pytest.mark.asyncio
async def test_probe_rejects_incomplete_config_without_network():
    calls = {"n": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        calls["n"] += 1
        return httpx.Response(200, json={})

    report = await probe(ep(base_url="", model=""), transport=transport(handler))
    assert report["ok"] is False
    assert calls["n"] == 0, "参数不全时不该发任何请求"
