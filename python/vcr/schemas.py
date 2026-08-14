"""数据模型（接口层 DTO）"""
from pydantic import BaseModel, Field


class TopItem(BaseModel):
    category: str
    label: str
    confidence: float


class ClassifyResult(BaseModel):
    path: str
    file_name: str
    category: str
    sub_category: str = ""        # 子类：动物→狗/猫/鸟/其他动物；自然风景→自然
    label: str
    confidence: float
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
