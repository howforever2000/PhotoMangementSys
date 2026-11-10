"""OpenAI 兼容 provider 适配（FEAT-152）

四个工具最终都落在这 3 个调用原语上：
  - chat_vision   → /chat/completions（多模态：看图理解 / 写配文）
  - images_edit   → /images/edits（图生图：风格化 / 美颜 / 滤镜迁移 / 超分）
  - images_generate → /images/generations（文生图，预留）

只做协议适配与错误归一，不含任何业务排版逻辑（那在 compose / workflows）。
"""
from __future__ import annotations

import base64
import time
from typing import List, Optional

import requests

from .. import config


class ProviderError(RuntimeError):
    """provider 调用失败（网络 / 鉴权 / 协议返回异常）。"""


def _norm(base: str) -> str:
    return (base or "").rstrip("/")


def _headers(api_key: str) -> dict:
    return {"Authorization": f"Bearer {api_key}"} if api_key else {}


def _png_data_url(data: bytes) -> str:
    return "data:image/png;base64," + base64.b64encode(data).decode("ascii")


def _decode_image_items(payload: dict, api_key: str) -> List[bytes]:
    """把 /images/* 返回体里的 data[] 解成图片字节：兼容 b64_json 与 url 两种。"""
    out: List[bytes] = []
    for item in payload.get("data") or []:
        if not isinstance(item, dict):
            continue
        b64 = item.get("b64_json") or item.get("b64")
        if b64:
            try:
                out.append(base64.b64decode(b64))
                continue
            except Exception:
                pass
        url = item.get("url")
        if url:
            r = requests.get(url, timeout=config.REQUEST_TIMEOUT)
            if r.status_code == 200:
                out.append(r.content)
    return out


def _err_text(r: requests.Response) -> str:
    try:
        return r.text[:300]
    except Exception:
        return f"<HTTP {r.status_code}>"


def chat_vision(
    base: str,
    api_key: str,
    model: str,
    images: List[bytes],
    prompt: str,
    timeout: int = config.REQUEST_TIMEOUT,
) -> str:
    """多模态对话：把若干图片 + 提示词发给视觉模型，返回文本。"""
    if not _norm(base) or not model:
        raise ProviderError("图像理解 provider 未配置（baseUrl / model 为空）")
    content: List[dict] = [{"type": "text", "text": prompt}]
    for img in images:
        content.append({"type": "image_url", "image_url": {"url": _png_data_url(img)}})
    body = {
        "model": model,
        "messages": [{"role": "user", "content": content}],
        "temperature": 0.6,
    }
    try:
        r = requests.post(
            f"{_norm(base)}/chat/completions",
            headers={**_headers(api_key), "Content-Type": "application/json"},
            json=body,
            timeout=timeout,
        )
    except Exception as e:  # 网络层
        raise ProviderError(f"调用图像理解服务失败: {e}") from e
    if r.status_code != 200:
        raise ProviderError(f"图像理解服务返回 HTTP {r.status_code}: {_err_text(r)}")
    try:
        j = r.json()
        msg = j["choices"][0]["message"]["content"]
    except Exception as e:
        raise ProviderError(f"图像理解返回结构异常: {e}") from e
    if isinstance(msg, list):  # 少数实现在 content 里返回分段
        return "".join(part.get("text", "") for part in msg if isinstance(part, dict))
    return str(msg or "")


def images_edit(
    base: str,
    api_key: str,
    model: str,
    images: List[bytes],
    prompt: str,
    timeout: int = config.REQUEST_TIMEOUT,
    size: Optional[str] = None,
) -> List[bytes]:
    """图生图：把 1~N 张图 + 提示词发给 /images/edits，返回结果图片字节。

    字段名兼容：OpenAI 官方与多数兼容服务用 `image`（单张）或重复 `image`；
    个别实现用 `image[]`。先按 `image` 提交，400/422 时自动改用 `image[]` 重试一次。
    """
    if not _norm(base) or not model:
        raise ProviderError("图像编辑 provider 未配置（baseUrl / model 为空）")
    if not images:
        raise ProviderError("图像编辑需要至少一张输入图")

    def _attempt(field: str):
        files = [(field, (f"image{i}.png", b, "image/png")) for i, b in enumerate(images)]
        data = {"model": model, "prompt": prompt, "n": "1"}
        if size:
            data["size"] = size
        return requests.post(
            f"{_norm(base)}/images/edits",
            headers=_headers(api_key),
            files=files,
            data=data,
            timeout=timeout,
        )

    try:
        r = _attempt("image")
    except Exception as e:
        raise ProviderError(f"调用图像编辑服务失败: {e}") from e
    if r.status_code in (400, 422):
        try:
            r2 = _attempt("image[]")
            if r2.status_code == 200:
                r = r2
        except Exception:
            pass
    if r.status_code != 200:
        raise ProviderError(f"图像编辑服务返回 HTTP {r.status_code}: {_err_text(r)}")
    try:
        imgs = _decode_image_items(r.json(), api_key)
    except Exception as e:
        raise ProviderError(f"图像编辑返回结构异常: {e}") from e
    if not imgs:
        raise ProviderError("图像编辑服务未返回图片")
    return imgs


def images_generate(
    base: str,
    api_key: str,
    model: str,
    prompt: str,
    timeout: int = config.REQUEST_TIMEOUT,
    size: Optional[str] = None,
) -> List[bytes]:
    """文生图：/images/generations（预留，本期工具未直接使用）。"""
    if not _norm(base) or not model:
        raise ProviderError("文生图 provider 未配置（baseUrl / model 为空）")
    body = {"model": model, "prompt": prompt, "n": 1}
    if size:
        body["size"] = size
    try:
        r = requests.post(
            f"{_norm(base)}/images/generations",
            headers={**_headers(api_key), "Content-Type": "application/json"},
            json=body,
            timeout=timeout,
        )
    except Exception as e:
        raise ProviderError(f"调用文生图服务失败: {e}") from e
    if r.status_code != 200:
        raise ProviderError(f"文生图服务返回 HTTP {r.status_code}: {_err_text(r)}")
    return _decode_image_items(r.json(), api_key)


def test_provider(kind: str, base: str, api_key: str, model: str) -> dict:
    """连通性测试：先探 /models（鉴权 + 可达），对 vision 再补一次极简 chat。"""
    t0 = time.time()
    if not _norm(base):
        return {"ok": False, "message": "baseUrl 为空", "latencyMs": 0}
    # 1) /models —— 大多数 OpenAI 兼容服务都实现，开销最小
    try:
        r = requests.get(f"{_norm(base)}/models", headers=_headers(api_key), timeout=config.PROBE_TIMEOUT)
        if r.status_code == 200:
            return {
                "ok": True,
                "message": "连接正常（/models 可用）",
                "latencyMs": int((time.time() - t0) * 1000),
            }
        models_err = f"HTTP {r.status_code}: {_err_text(r)}"
    except Exception as e:
        models_err = str(e)

    # 2) /models 不可用时：对视觉能力补一次 chat 探测
    if kind == "vision" and model:
        try:
            txt = chat_vision(base, api_key, model, [], "reply with ok", timeout=config.PROBE_TIMEOUT)
            return {
                "ok": True,
                "message": f"连接正常（chat 可用）：{txt[:40]}",
                "latencyMs": int((time.time() - t0) * 1000),
            }
        except Exception as e:
            return {"ok": False, "message": f"/models 失败({models_err})；chat 失败({e})", "latencyMs": int((time.time() - t0) * 1000)}
    return {"ok": False, "message": f"连接失败：{models_err}", "latencyMs": int((time.time() - t0) * 1000)}
