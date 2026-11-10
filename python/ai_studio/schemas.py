"""请求/响应 DTO（FEAT-152）

字段名用 camelCase，与 Rust serde(rename_all="camelCase")、前端 types/ai.ts 三方一一对应。
"""
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field


class ProviderConfig(BaseModel):
    """一个 OpenAI 兼容服务商的一个能力配置。"""

    baseUrl: str = ""
    apiKey: str = ""
    model: str = ""


class Providers(BaseModel):
    """按能力划分的三个 provider：图像理解 / 图像编辑 / 超分。"""

    vision: Optional[ProviderConfig] = None
    edit: Optional[ProviderConfig] = None
    upscale: Optional[ProviderConfig] = None


class JobRequest(BaseModel):
    """创建一个作业。paths 为本地绝对路径（同机直读，不走 base64 上传）。"""

    tool: str
    paths: List[str] = Field(default_factory=list)
    prompt: str = ""
    outputDir: str = ""
    options: Dict[str, Any] = Field(default_factory=dict)
    providers: Providers = Field(default_factory=Providers)


class TestProviderRequest(BaseModel):
    """连通性测试：kind ∈ vision/edit/upscale。"""

    kind: str = "vision"
    provider: ProviderConfig = Field(default_factory=ProviderConfig)
