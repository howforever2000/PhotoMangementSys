"""视觉内容识别微服务（VCR）—— 接口层

分层架构：
  接口层  本文件：FastAPI 路由 + DTO（薄壳，无业务逻辑）
  服务层  vcr/services/：classifier / detector / face_service / scene_service /
          arbitrator / pipeline
  持久层  vcr/persistence/：person_store（SQLite 人物注册表）
  基础设施 vcr/model_registry / preprocess / mapping / config

路由：
  GET  /health                 → 模型与人物注册表状态
  POST /classify               → 单张 {path}
  POST /classify_batch         → 批量 {paths: [...]}（≤ BATCH_CHUNK）
  GET  /persons                → 人物列表
  POST /persons/{id}/rename    → {name}
  POST /persons/merge          → {target, source}
  DELETE /persons/{id}         → 删除人物

启动: python server.py          （默认 127.0.0.1:8765）
"""
import os

import uvicorn
from fastapi import FastAPI, HTTPException

from vcr import config
from vcr.model_registry import get_registry
from vcr.persistence.person_store import get_store
from vcr.schemas import (
    ClassifyBatchRequest,
    ClassifyError,
    ClassifyRequest,
    ClassifyResult,
    PersonMergeRequest,
    TopItem,
)
from vcr.services.pipeline import classify_one
from vcr.taxonomy import get_taxonomy

app = FastAPI(title="VCR", docs_url=None, redoc_url=None)


def _fold_result(r) -> ClassifyResult:
    """输出前 taxonomy 折叠：保证 category ∈ 9 组（Phase 4 收敛）。"""
    tax = get_taxonomy()
    return ClassifyResult(
        path=r.path,
        file_name=r.file_name,
        category=tax.fold(r.category),
        sub_category=r.sub_category,
        label=r.label,
        confidence=r.confidence,
        top3=[TopItem(category=tax.fold(t.category), label=t.label, confidence=t.confidence) for t in r.top3],
        person_ids=r.person_ids,
        person_count=r.person_count,
        source=r.source,
        elapsed_ms=r.elapsed_ms,
    )


def _health_dict() -> dict:
    reg = get_registry()
    reg.status()  # 触发惰性加载，便于 /health 反映真实状态
    store = get_store()
    tax = get_taxonomy()
    return {
        "ok": reg.is_ready("cls"),
        "models": reg.status(),
        "categories": tax.groups(),
        "persons": len(store.list_persons()),
    }


@app.get("/health")
def health():
    reg = get_registry()
    ready = reg.status()["cls"]["ready"]   # status() 先强制加载再判断
    d = _health_dict()
    return {
        "ok": ready,
        "model": config.CLS_MODELS[0] if ready else "none",
        "det_ready": reg.is_ready("det"),
        "face_ready": reg.is_ready("face_det") and reg.is_ready("face_rec"),
        "scene_ready": reg.is_ready("scene"),
        "ocr_ready": reg.is_ready("ocr"),
        "flower_ready": reg.is_ready("flower"),
        "food_ready": reg.is_ready("food"),
        "classes": 1000 if ready else 0,
        "categories": d["categories"],
        "persons": d["persons"],
        "gpu": reg.gpu_info(),
        "batch_max": config.BATCH_CHUNK_MAX,
    }


@app.get("/gpu")
def gpu():
    """GPU 加速可行性探测（R3）：可用提供方 + 当前是否走 GPU + 提供方。"""
    reg = get_registry()
    reg.status()  # 触发模型加载（提供方在加载时选定）
    info = reg.gpu_info()
    info["batch_max"] = config.BATCH_CHUNK_MAX
    return info


@app.post("/classify")
def classify(req: ClassifyRequest):
    if not get_registry().is_ready("cls"):
        raise HTTPException(503, "分类模型未就绪")
    r = classify_one(req.path, get_registry())
    if r is None:
        raise HTTPException(400, f"无法读取图片: {req.path}")
    return _fold_result(r)


@app.post("/classify_batch")
def classify_batch(req: ClassifyBatchRequest):
    if not get_registry().is_ready("cls"):
        raise HTTPException(503, "分类模型未就绪")
    # 批次由客户端控制（R3），此处仅做安全封顶，避免单次超大请求
    paths = req.paths[: config.BATCH_CHUNK_MAX]
    results: list = []
    for p in paths:
        r = classify_one(p, get_registry())
        if r is None:
            results.append(
                ClassifyError(path=p, file_name=os.path.basename(p), error="无法读取图片").model_dump()
            )
        else:
            results.append(_fold_result(r).model_dump())
    return {"results": results}


# ---------------------------------------------------------------------------
# 人物注册表管理
# ---------------------------------------------------------------------------
@app.get("/persons")
def list_persons():
    return {"persons": get_store().list_persons()}


@app.post("/persons/{pid}/rename")
def rename_person(pid: str, body: dict):
    name = (body.get("name") or "").strip()
    if not name:
        raise HTTPException(400, "name 不能为空")
    if not get_store().rename(pid, name):
        raise HTTPException(404, f"人物不存在: {pid}")
    return {"ok": True}


@app.post("/persons/merge")
def merge_persons(body: PersonMergeRequest):
    if not get_store().merge(body.target, body.source):
        raise HTTPException(400, "合并失败：人物不存在或相同")
    return {"ok": True}


@app.delete("/persons/{pid}")
def delete_person(pid: str):
    if not get_store().delete(pid):
        raise HTTPException(404, f"人物不存在: {pid}")
    return {"ok": True}


if __name__ == "__main__":
    import sys

    port = int(os.environ.get("VCR_PORT", "8765"))
    from vcr.model_registry import get_registry

    print(f"[VCR] 启动模型加载…", file=sys.stderr)
    st = get_registry().status()
    print(f"[VCR] 模型状态: {st}", file=sys.stderr)
    print(f"[VCR] 接口层启动 http://127.0.0.1:{port}", file=sys.stderr)
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")
