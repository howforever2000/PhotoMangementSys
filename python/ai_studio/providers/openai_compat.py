"""OpenAI 兼容 provider 适配（FEAT-152）

四个工具最终都落在这 3 个调用原语上：
  - chat_vision   → /chat/completions（多模态：看图理解 / 写配文）
  - images_edit   → /images/generations + image 字段（图生图：风格化 / 美颜 / 滤镜迁移 / 超分）
  - images_generate → /images/generations（文生图，预留）

只做协议适配与错误归一，不含任何业务排版逻辑（那在 compose / workflows）。
"""
from __future__ import annotations

import base64
import time
from io import BytesIO
from typing import List, Optional

import requests
from PIL import Image

from .. import config


class ProviderError(RuntimeError):
    """provider 调用失败（网络 / 鉴权 / 协议返回异常）。"""


def _norm(base: str) -> str:
    return (base or "").rstrip("/")


def _headers(api_key: str) -> dict:
    return {"Authorization": f"Bearer {api_key}"} if api_key else {}


def _png_data_url(data: bytes) -> str:
    return "data:image/png;base64," + base64.b64encode(data).decode("ascii")


def _image_data_url(data: bytes) -> str:
    """把任意图片字节包成带正确 MIME 的 data URL（按真实格式推断，不写死 png）。"""
    try:
        fmt = (Image.open(BytesIO(data)).format or "PNG").upper()
    except Exception:
        fmt = "PNG"
    mime = {
        "JPEG": "image/jpeg", "JPG": "image/jpeg", "PNG": "image/png",
        "WEBP": "image/webp", "BMP": "image/bmp", "GIF": "image/gif", "TIFF": "image/tiff",
    }.get(fmt, "image/png")
    return f"data:{mime};base64," + base64.b64encode(data).decode("ascii")


def _image_dims(data: bytes):
    try:
        return Image.open(BytesIO(data)).size
    except Exception:
        return (0, 0)


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


def _norm_edit_error(r: requests.Response) -> str:
    """把图生图的 HTTP 错误归一成用户能看懂的中文提示。"""
    txt = _err_text(r)
    low = txt.lower()
    if r.status_code == 404:
        return "图像编辑服务返回 HTTP 404：该 baseUrl 未实现图生图接口（应走 /images/generations + image 字段）"
    if r.status_code == 401:
        return f"图像编辑鉴权失败（401），请检查 API Key：{txt}"
    if r.status_code == 403 and "insufficient_quota" in low:
        return "该模型免费额度已用尽（403 insufficient_quota），请在「模型设置」改用其它模型（如 qwen-image-3.0-pro）或充值后再试"
    if r.status_code == 403 and ("unpurchased" in low or "accessdenied" in low or "access to model denied" in low):
        return "该模型未开通 / 无使用权限（403 AccessDenied.Unpurchased），请在「模型设置」改用已开通的模型，或到阿里云百炼控制台开通、充值后再试"
    if r.status_code == 403:
        return f"图像编辑无权限（403），请确认模型已开通或额度充足：{txt}"
    if r.status_code == 429:
        return "请求过于频繁（429 rate limit），已自动重试仍失败，请稍后再试"
    if r.status_code == 400 and "area" in low and ("between" in low or "must be" in low):
        return (
            "图像分辨率不满足图生图要求（面积需在 512×512 ~ 2560×2560 之间），"
            f"已自动缩放仍失败，请换一张原图再试：{txt}"
        )
    if r.status_code == 400 and "size" in low:
        return f"size 参数不合法（需 \"宽x高\"，如 1024x768）：{txt}"
    return f"图像编辑服务返回 HTTP {r.status_code}: {txt}"


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
    steps: Optional[int] = None,
) -> List[bytes]:
    """图生图：把 1~N 张图 + 提示词发给 `/images/generations`（本网关实测协议），返回结果图字节。

    实测（网关 maas.qianwenaiapi.com，qwen-image-3.0）：
      - `/images/edits` 路由不存在（404），大包 multipart 被网关 RST → 10054；
      - 正确协议：`POST {base}/images/generations`，JSON body
        `{model, prompt, n:1, size:"宽x高", image:"data:image/...;base64,..."}`；
        多图用 `images` 数组；返回 `data[0].url`（OSS，24h 有效）。
    `size` 缺省时取首图尺寸（网关要求小写 `x` 分隔、正整数，如 1024x768）。
    """
    if not _norm(base) or not model:
        raise ProviderError("图像编辑 provider 未配置（baseUrl / model 为空）")
    if not images:
        raise ProviderError("图像编辑需要至少一张输入图")

    if not size:
        w, h = _image_dims(images[0])
        if w and h:
            size = f"{w}x{h}"
    body: dict = {"model": model, "prompt": prompt, "n": 1}
    if size:
        body["size"] = size
    if steps:
        body["steps"] = int(steps)  # 可选采样步数：不填不传，保持对既有网关的兼容
    data_urls = [_image_data_url(b) for b in images]
    if len(data_urls) == 1:
        body["image"] = data_urls[0]
    else:
        body["images"] = data_urls

    url = f"{_norm(base)}/images/generations"
    headers = {**_headers(api_key), "Content-Type": "application/json"}
    r: Optional[requests.Response] = None
    last_err: Optional[Exception] = None
    for attempt in range(3):
        try:
            r = requests.post(url, headers=headers, json=body, timeout=timeout)
        except requests.exceptions.ConnectionError as e:
            # 10054 / 连接被重置：多半是上传体过大或网关前置代理拒绝
            raise ProviderError(
                "上传图片过大或端点不可达（连接被重置 10054），请减小输入分辨率或检查 baseUrl"
            ) from e
        except Exception as e:
            last_err = e
            if attempt < 2:
                time.sleep(2 * (attempt + 1))
                continue
            raise ProviderError(f"调用图像编辑服务失败: {e}") from e
        if r.status_code == 200:
            break
        if r.status_code in (429, 500, 502, 503, 504) and attempt < 2:
            time.sleep(3 * (attempt + 1))
            continue
        raise ProviderError(_norm_edit_error(r))
    if r is None or r.status_code != 200:
        raise ProviderError(f"图像编辑服务重试后仍失败: {last_err}")
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
    steps: Optional[int] = None,
) -> List[bytes]:
    """文生图：/images/generations（预留，本期工具未直接使用）。"""
    if not _norm(base) or not model:
        raise ProviderError("文生图 provider 未配置（baseUrl / model 为空）")
    body = {"model": model, "prompt": prompt, "n": 1}
    if size:
        body["size"] = size
    if steps:
        body["steps"] = int(steps)  # 可选采样步数：不填不传
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
