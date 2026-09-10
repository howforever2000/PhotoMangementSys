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
  GET  /persons/{id}/avatar    → 人物头像（代表脸 bbox 裁剪，JPEG）
  POST /persons/{id}/rename    → {name}
  POST /persons/merge          → {target, source}
  DELETE /persons/{id}         → 删除人物
  POST /benchmark              → FEAT-053：cls 通道固定张量测速（CPU/GPU 加速比对比）

启动: python server.py          （默认 127.0.0.1:8765）
"""
import os
import threading

import uvicorn
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

from vcr import config
from vcr.model_registry import get_registry
from vcr.persistence.person_store import get_store
from vcr.schemas import (
    ClassifyBatchRequest,
    ClassifyError,
    ClassifyRequest,
    ClassifyResult,
    EmbedBatchRequest,
    EmbedTextRequest,
    PersonMergeRequest,
    TopItem,
)
from vcr.services.pipeline import classify_one
from vcr.services.embed_service import get_embed_service
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
    # 只读快照，严禁触发加载：模型由启动时的后台线程预加载。
    # 此前在这里 reg.status() 强制同步加载，首个 /health 会被阻塞数分钟，
    # 宿主健康探测（2s 超时）误判「端口不可达」→ 反复杀进程重启（10054 刷屏）。
    store = get_store()
    tax = get_taxonomy()
    return {
        "categories": tax.groups(),
        "persons": len(store.list_persons()),
    }


# FEAT-051：API 版本（GPU 开关 + 模型切换能力）。宿主检测到运行中服务版本过旧时
# 会 POST /shutdown 自动重启到新版本。
# v3（FEAT-053）：/benchmark 端点 + /gpu /models /health 新增会话实测字段。
# v4：语义搜索（Chinese-CLIP fp16）—— /embed_text /embed_batch /health.clip_ready。
VCR_API_VERSION = 4


@app.get("/health")
def health():
    # 只读状态（绝不触发加载，保证探测毫秒级返回）：模型未加载完时 ok=false，
    # 宿主据此进入 Loading 等待而非误判「不可达」而杀进程。
    reg = get_registry()
    ready = reg.is_ready("cls")
    d = _health_dict()
    # FEAT-053 修复：此前写死 config.CLS_MODELS[0]，用户切换模型后该字段
    # 永远显示默认候选 —— 误导「模型没变」。改为登记生效的实际选择。
    model = reg.current_cls_name()
    return {
        "ok": ready,
        "model": model if model else "none",
        "api_version": VCR_API_VERSION,
        "det_ready": reg.is_ready("det"),
        "face_ready": reg.is_ready("face_det") and reg.is_ready("face_rec"),
        "scene_ready": reg.is_ready("scene"),
        "ocr_ready": reg.is_ready("ocr"),
        "flower_ready": reg.is_ready("flower"),
        "food_ready": reg.is_ready("food"),
        "clip_ready": get_embed_service().ready(),
        "classes": 1000 if ready else 0,
        "categories": d["categories"],
        "persons": d["persons"],
        "gpu": reg.gpu_info(),
        "batch_max": config.BATCH_CHUNK_MAX,
    }


@app.post("/shutdown")
def shutdown():
    """FEAT-051：宿主检测到服务版本过旧时调用，自退以便宿主拉起新版本。

    本服务仅监听 127.0.0.1，无外部暴露风险。
    """
    os._exit(0)


@app.get("/gpu")
def gpu():
    """GPU 加速可行性探测（R3）：可用提供方 + 当前是否走 GPU + 提供方。"""
    # provider 选择不依赖会话，无需触发模型加载（探测请求保持毫秒级）
    info = get_registry().gpu_info()
    info["batch_max"] = config.BATCH_CHUNK_MAX
    return info


class GpuRequest(BaseModel):
    enabled: bool


class ModelRequest(BaseModel):
    name: str


@app.post("/gpu")
def set_gpu(req: GpuRequest):
    """FEAT-051：GPU 加速开关（开 = GPU 优先 / 关 = 强制 CPU）。

    会话已随切换清空，由后台线程重建（加载耗时不可预估，不能阻塞本请求，
    否则宿主 15s HTTP 超时）；重建期间 /health 返回 ok=false，宿主会等待就绪。
    """
    try:
        info = get_registry().set_gpu_enabled(req.enabled)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(status_code=500, detail=str(e))
    info["batch_max"] = config.BATCH_CHUNK_MAX
    # 会话已清空：主链路后台重建（加载耗时不可预估，不能阻塞本请求 —— 宿主 15s 超时）；
    # 专家通道（face/ocr 等）下次使用时按新 provider 惰性重建
    threading.Thread(target=_rebuild_main_chain, name="vcr-rebuild", daemon=True).start()
    return {"ok": True, **info}


@app.get("/models")
def models():
    """FEAT-051：分类模型候选清单（含是否已下载 / 当前生效）。"""
    return get_registry().cls_models_info()


@app.post("/model")
def set_model(req: ModelRequest):
    """FEAT-051：切换分类模型（文件未下载 / 未知名称返回 400）。

    校验并登记目标后立即返回（对齐 /gpu 的异步模式）：大模型 CPU 加载可达
    数十秒，同步加载会撞宿主 15s HTTP 超时。加载期间 is_ready("cls")=False →
    /health ok=false，宿主会等待就绪；加载失败自动回退默认候选。
    """
    try:
        info = get_registry().begin_cls_model_switch(req.name)
    except (ValueError, FileNotFoundError, RuntimeError) as e:
        raise HTTPException(status_code=400, detail=str(e))
    threading.Thread(
        target=_finish_cls_switch, args=(req.name,), name="vcr-cls-switch", daemon=True
    ).start()
    return {"ok": True, "loading": True, **info}


def _finish_cls_switch(name: str) -> None:
    """后台完成分类模型会话加载（成功持久化 / 失败回退默认候选）。"""
    import sys

    ok = get_registry().finish_cls_model_switch(name)
    print(f"[VCR] 分类模型切换 {name}: {'完成' if ok else '失败，已回退默认候选'}", file=sys.stderr)


class BenchmarkRequest(BaseModel):
    runs: int = 10
    warmup: int = 2


@app.post("/benchmark")
def benchmark(req: BenchmarkRequest):
    """FEAT-053：cls 通道固定张量测速 —— CPU/GPU 真实加速比一键对比。

    可能触发模型加载与数十次推理（秒级耗时），宿主健康探测不经过此端点；
    Rust 侧对该调用使用独立长超时（默认 HTTP 客户端 15s 可能不够）。
    返回实测 provider（sess.get_providers()）+ 平均/最快/最慢毫秒。
    """
    try:
        return get_registry().benchmark("cls", runs=req.runs, warmup=req.warmup)
    except RuntimeError as e:
        raise HTTPException(status_code=503, detail=str(e))


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
# 语义搜索（Chinese-CLIP fp16，可选通道；模型缺失时 503，宿主降级纯关键词）
# ---------------------------------------------------------------------------
@app.get("/embed_status")
def embed_status():
    """CLIP 子系统状态（含拆分件/tokenizer 就绪详情，供诊断；不触发加载）。"""
    return get_embed_service().status()


@app.post("/embed_text")
def embed_text(req: EmbedTextRequest):
    try:
        vec = get_embed_service().embed_text(req.text)
    except RuntimeError as e:
        raise HTTPException(503, str(e))
    return {"dim": int(vec.shape[0]), "embedding": [round(float(x), 6) for x in vec]}


@app.post("/embed_batch")
def embed_batch(req: EmbedBatchRequest):
    svc = get_embed_service()
    if not svc.ready():
        # 未就绪时不阻塞首请求：ensure 同步拆图+加载（首次数十秒），仍失败则明确 503
        try:
            svc.ensure()
        except Exception as e:  # noqa: BLE001
            raise HTTPException(503, f"CLIP 未就绪: {e}")
        if not svc.ready():
            raise HTTPException(503, "CLIP 未就绪")
    paths = req.paths[: config.BATCH_CHUNK_MAX]
    return {"results": svc.embed_images(paths)}


# ---------------------------------------------------------------------------
# 人物注册表管理
# ---------------------------------------------------------------------------
@app.get("/persons")
def list_persons():
    return {"persons": get_store().list_persons()}


@app.get("/persons/{pid}/avatar")
def person_avatar(pid: str, size: int = 96):
    """人物头像：取代表脸按 bbox 裁剪原图并缩放到 size×size JPEG。

    - 代表脸 = 该人物最早登记的一张脸（稳定不跳变）
    - 原图文件已被删除/无法读取 → 404（前端回退到编号占位样式）
    """
    import io
    import re

    import cv2
    from fastapi import Response

    face = get_store().representative_face(pid)
    if face is None:
        raise HTTPException(404, f"人物无登记人脸: {pid}")
    photo_path, bbox_raw = face
    if not os.path.isfile(photo_path):
        raise HTTPException(404, f"代表脸原图不存在: {photo_path}")
    nums = [int(v) for v in re.findall(r"-?\d+", bbox_raw)]
    if len(nums) < 4:
        raise HTTPException(500, f"bbox 格式异常: {bbox_raw}")
    x1, y1, x2, y2 = nums[:4]
    # 轻微外扩 12%，避免裁得太贴五官；并夹紧到图内
    bw, bh = max(x2 - x1, 1), max(y2 - y1, 1)
    dx, dy = int(bw * 0.12), int(bh * 0.12)
    img = cv2.imread(photo_path)
    if img is None:
        raise HTTPException(404, f"代表脸原图无法读取: {photo_path}")
    h, w = img.shape[:2]
    x1, y1 = max(0, x1 - dx), max(0, y1 - dy)
    x2, y2 = min(w, x2 + dx), min(h, y2 + dy)
    crop = img[y1:y2, x1:x2]
    if crop.size == 0:
        raise HTTPException(500, "人脸裁剪结果为空")
    side = max(size, 32)
    crop = cv2.resize(crop, (side, side), interpolation=cv2.INTER_AREA)
    ok, buf = cv2.imencode(".jpg", crop, [int(cv2.IMWRITE_JPEG_QUALITY), 88])
    if not ok:
        raise HTTPException(500, "头像编码失败")
    return Response(content=buf.tobytes(), media_type="image/jpeg",
                    headers={"Cache-Control": "no-store"})


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


def _rebuild_main_chain() -> None:
    """后台预热主链路模型（启动预加载与 GPU 切换后重建共用）。

    仅 cls/det/scene（每张图必经）；face/ocr/flower/food 专家通道按需惰性
    重建。全量预热会把服务就绪拖到数十秒并放大宿主等待窗口。
    """
    import sys

    try:
        st = get_registry().preload_main()
        print(f"[VCR] 主链路模型状态: {st}", file=sys.stderr)
    except Exception as e:  # noqa: BLE001
        print(f"[VCR] 主链路模型加载失败: {e}", file=sys.stderr)


def _quiet_proactor_noise() -> None:
    """Windows Proactor 事件循环在客户端 abrupt 断开（宿主健康探测短超时、
    进程被宿主收管结束）时会刷 ConnectionResetError(10054) 的 ERROR 日志。
    这类断连属正常现象，过滤以免刷屏（保留其他 asyncio 错误）。"""
    import logging

    class _ResetFilter(logging.Filter):
        def filter(self, record: logging.LogRecord) -> bool:
            if record.exc_info and record.exc_info[0] is ConnectionResetError:
                return False
            try:
                return "ConnectionResetError" not in record.getMessage()
            except Exception:  # noqa: BLE001
                return True

    logging.getLogger("asyncio").addFilter(_ResetFilter())


if __name__ == "__main__":
    import sys
    import threading

    port = int(os.environ.get("VCR_PORT", "8765"))

    # 模型后台预热（仅主链路 cls/det/scene）：uvicorn 先绑定端口（/health 立即可达，
    # 加载中返回 ok=false，宿主进入 Loading 等待），模型在后台线程加载。此前同步
    # 预加载阻塞在 uvicorn.run 之前，端口迟迟不监听 → 宿主误判「不可达」→ 杀进程
    # 重启死循环；全量预热 8 通道则会把就绪窗口拖到数十秒。
    _quiet_proactor_noise()
    threading.Thread(target=_rebuild_main_chain, name="vcr-preload", daemon=True).start()
    print(f"[VCR] 接口层启动 http://127.0.0.1:{port}", file=sys.stderr)
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")
