"""数据模型（接口层 DTO）"""
from pydantic import BaseModel, Field


class TopItem(BaseModel):
    category: str
    label: str
    confidence: float


class ClassifyResult(BaseModel):
    path: str
    file_name: str
    category: str                # portrait / street / night_scene / document / other
    sub_category: str = ""       # closeup / group / street / night / paper / screenshot / other
    label: str
    confidence: float
    # v5：分类模型下线后不再有 ImageNet 细类候选，保留字段（单条 = 最终结论）兼容旧前端
    top3: list[TopItem] = Field(default_factory=list)
    person_ids: list[str] = Field(default_factory=list)
    person_count: int = 0
    source: str
    elapsed_ms: float


class ClassifyError(BaseModel):
    path: str
    file_name: str
    error: str


class ClassifyRequest(BaseModel):
    path: str


class ClassifyBatchRequest(BaseModel):
    paths: list[str]


class PersonInfo(BaseModel):
    id: str
    name: str
    face_count: int
    created_at: str


class PersonMergeRequest(BaseModel):
    target: str          # 保留的人
    source: str          # 被合并进 target 的人


# ---- 语义（Chinese-CLIP embedding）----
class EmbedTextRequest(BaseModel):
    text: str


class EmbedTextBatchRequest(BaseModel):
    texts: list[str]


class EmbedBatchRequest(BaseModel):
    paths: list[str]
