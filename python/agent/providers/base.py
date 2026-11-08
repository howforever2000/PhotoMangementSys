"""Provider 契约：请求/结果/错误分类。

错误必须分类，不能一律 `Exception`：三类失败给用户的下一步完全不同。
- `auth`      → Key 错/过期 → "去设置里检查 Key"
- `rate_limit`→ 429 → "稍等再试"
- `bad_shape` → 返回体不符合预期 → "地址可能不是 OpenAI 兼容端点"
- `network`   → 连不上 → "检查地址/网络/本地服务是否启动"
"""
from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field

# 错误种类（给 UI 决定文案与动作）
ERR_AUTH = "auth"
ERR_RATE_LIMIT = "rate_limit"
ERR_BAD_SHAPE = "bad_shape"
ERR_NETWORK = "network"
ERR_TIMEOUT = "timeout"
ERR_SERVER = "server"


class ProviderError(RuntimeError):
    """模型端点调用失败（带分类与可展示文案）。"""

    def __init__(self, kind: str, message: str, status: int | None = None):
        super().__init__(message)
        self.kind = kind
        self.status = status

    def to_dict(self) -> dict:
        return {"kind": self.kind, "message": str(self), "status": self.status}


@dataclass
class EditRequest:
    """一次图像编辑请求。"""

    images: list[bytes] = field(default_factory=list)
    prompt: str = ""
    size: str | None = None
    negative: str = ""


@dataclass
class EditResult:
    image_bytes: bytes = b""
    cost: float | None = None
    model: str = ""
    latency_ms: int = 0
    # 实际用的路径（edits / generations / 异步轮询），写进轨迹便于排障
    route: str = ""


class ImageProvider(ABC):
    """图像生成/编辑通道。"""

    @abstractmethod
    async def edit(self, req: EditRequest) -> EditResult: ...


# ------------------------------------------------------------------ 端点配置


@dataclass
class Endpoint:
    """用户填的一个端点（图像或规划）。

    没有"预设型号"这一层：model 由用户填，size 由档位决定（见 registry.py）。
    """

    base_url: str = ""
    api_key: str = ""
    model: str = ""
    # 每张图单价（用户自填，未知填 0）。预算闸用它做金额估算。
    unit_price: float = 0.0
    # 端点类型：image | planner | both
    kind: str = "image"
    # 先按同步调，失败/形状不符时探针会提示是否要异步轮询
    async_poll: bool = False

    def normalize(self) -> "Endpoint":
        """规整用户输入：补协议、去尾斜杠、去多余的 /v1 重复。"""
        base = (self.base_url or "").strip().rstrip("/")
        if base and not base.lower().startswith(("http://", "https://")):
            # 本地服务常被写成 `127.0.0.1:11434/v1`，补 http 比让用户重填友好
            base = "http://" + base
        return Endpoint(
            base_url=base,
            api_key=(self.api_key or "").strip(),
            model=(self.model or "").strip(),
            unit_price=float(self.unit_price or 0.0),
            kind=(self.kind or "image").strip(),
            async_poll=bool(self.async_poll),
        )

    def validate(self) -> None:
        if not self.base_url:
            raise ProviderError(ERR_BAD_SHAPE, "没有填写模型地址")
        if not self.model:
            raise ProviderError(ERR_BAD_SHAPE, "没有填写模型名（如 seedream-5-0-pro / qwen2.5-vl）")
        # 本地服务通常不要 Key；只有指向公网时才强制
        if not self.api_key and not self._looks_local():
            raise ProviderError(ERR_AUTH, "没有填写 API Key")

    def _looks_local(self) -> bool:
        b = self.base_url.lower()
        return any(h in b for h in ("127.0.0.1", "localhost", "0.0.0.0", "[::1]"))

    @property
    def masked_key(self) -> str:
        k = self.api_key
        if not k:
            return ""
        if len(k) <= 10:
            return "*" * len(k)
        return f"{k[:6]}…{k[-4:]}"
