"""视觉内容识别微服务（VCR）—— 接口层

分层架构：
  接口层  本文件：FastAPI 路由 + DTO（薄壳，无业务逻辑）
  服务层  vcr/services/：detector / face_service / ocr_service / tone_service /
          arbitrator / pipeline / embed_service / clip_subgraph
  持久层  vcr/persistence/：person_store（SQLite 人物注册表）
  基础设施 vcr/model_registry / preprocess / config

v5（语义分类）：分类模型（yolov8*-cls）、Places365 场景、花朵/食物专家通道
**已全部下线**；内容分类由宿主侧「语义分类」（Chinese-CLIP 关键词匹配）承担。
本服务只保留人物/夜景/文档三条规则通道 + CLIP 双塔（语义向量与关键词编码）。

路由：
  GET  /health                    → 模型状态（含语义档位 / clip_ready）
  POST /classify                  → 单张 {path}（人物/夜景/文档规则）
  POST /classify_batch            → 批量 {paths: [...]}（≤ BATCH_CHUNK_MAX）
  GET  /persons                   → 人物列表
  GET  /persons/{id}/avatar       → 人物头像（代表脸 bbox 裁剪，JPEG）
  POST /persons/{id}/rename       → {name}
  POST /persons/merge             → {target, source}
  DELETE /persons/{id}            → 删除人物
  GET  /gpu  POST /gpu            → GPU 加速状态 / 开关
  GET  /models  POST /model       → 语义模型档位清单 / 切换（b16 / b16-fp32）
  GET  /threads POST /threads     → CPU 线程数现状 / 设置（性能设置里可调）
  POST /benchmark                 → 固定张量测速（CPU/GPU 加速比对比，可临时指定线程数）
  POST /benchmark_sweep           → 线程数扫档（一次测多个档位，UI 选最优）
  GET  /embed_status              → CLIP 子系统诊断
  POST /embed_text                → 单条文本 → 向量
  POST /embed_text_batch          → 批量文本 → 向量（语义分类关键词一次编码）
  POST /embed_batch               → 批量图片 → 向量（语义索引扫描）

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
    EmbedTextBatchRequest,
    EmbedTextRequest,
    PersonMergeRequest,
)
from vcr.services.embed_service import get_embed_service
from vcr.services.pipeline import classify_one

app = FastAPI(title="VCR", docs_url=None, redoc_url=None)


def _health_dict() -> dict:
    # 只读快照，严禁触发加载：模型由启动时的后台线程预加载。
    store = get_store()
    return {"persons": len(store.list_persons())}


# FEAT-051：API 版本（GPU 开关 + 模型切换能力）。宿主检测到运行中服务版本过旧时
# 会 POST /shutdown 自动重启到新版本。
# v3（FEAT-053）：/benchmark 端点 + /gpu /models /health 新增会话实测字段。
# v4：语义搜索（Chinese-CLIP fp16）—— /embed_text /embed_batch /health.clip_ready。
# v5：语义分类 —— 下线分类模型/场景/专家通道；语义模型档位选择（/models /model，
#     B/16 ↔ L/14-336）；新增 /embed_text_batch。
# v6：CPU 线程数可调（/threads 读写 + /benchmark 支持临时线程覆盖 + /benchmark_sweep 扫档），
#     供「⚙ 性能设置」按不同硬件实测选优。
VCR_API_VERSION = 6


@app.get("/health")
def health():
    # 只读状态（绝不触发加载，保证探测毫秒级返回）：模型未加载完时 ok=false，
    # 宿主据此进入 Loading 等待而非误判「不可达」而杀进程。
    reg = get_registry()
    d = _health_dict()
    embed = get_embed_service()
    tier = embed.tier_info()
    return {
        "ok": reg.is_ready("det"),
        "model": tier["name"],
        "model_id": tier["id"],
        "api_version": VCR_API_VERSION,
        "det_ready": reg.is_ready("det"),
        "face_ready": reg.is_ready("face_det") and reg.is_ready("face_rec"),
        "ocr_ready": reg.is_ready("ocr"),
        "clip_ready": embed.ready(),
        "persons": d["persons"],
        "threads": config.threads(),
        "gpu": reg.gpu_info(),
        "batch_max": config.BATCH_CHUNK_MAX,
        # 各通道最近一次加载失败原因：宿主据此在日志里直接写明「为何 ok=false」
        # （面板转圈排查要点——BUG-2026-0921-003）
        "load_errors": reg.load_errors(),
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
    threading.Thread(target=_rebuild_main_chain, name="vcr-rebuild", daemon=True).start()
    return {"ok": True, **info}


@app.get("/models")
def models():
    """语义模型档位候选清单（含是否已下载 / 当前生效 / 会话实测事实）。"""
    return get_registry().clip_models_info()


@app.post("/model")
def set_model(req: ModelRequest):
    """切换语义模型档位（未下载 / 未知档位返回 400）。

    校验并登记目标后立即返回（对齐 /gpu 的异步模式）：模型可达数百 MB~1.5GB，
    同步加载会撞宿主 15s HTTP 超时。加载期间 clip_ready=false。
    """
    try:
        info = get_registry().begin_clip_switch(req.name)
    except (ValueError, FileNotFoundError, RuntimeError) as e:
        raise HTTPException(status_code=400, detail=str(e))
    threading.Thread(
        target=_finish_clip_switch, args=(req.name,), name="vcr-clip-switch", daemon=True
    ).start()
    return {"ok": True, "loading": True, **info}


def _finish_clip_switch(name: str) -> None:
    """后台完成档位切换（成功加载双塔 / 失败回退默认档）。"""
    import sys

    ok = get_registry().finish_clip_switch(name)
    print(f"[VCR] 语义模型切换 {name}: {'完成' if ok else '失败，已回退默认档'}", file=sys.stderr)


@app.get("/threads")
def get_threads():
    """CPU 线程数现状（当前/默认/物理核推测/可选档），供「性能设置」展示。"""
    return config.threads_info()


class ThreadsRequest(BaseModel):
    threads: int


@app.post("/threads")
def set_threads(req: ThreadsRequest):
    """设置 CPU 线程数（越界自动夹紧）→ 持久化 + 清空会话 → 后台重建。

    线程数在会话创建时绑定，必须重建会话才生效；重建期间 /health ok=false，
    宿主会等待就绪（与 /gpu 切换同一套异步模式）。
    """
    try:
        info = get_registry().set_threads(req.threads)
    except Exception as e:  # noqa: BLE001
        raise HTTPException(status_code=500, detail=str(e))
    threading.Thread(target=_rebuild_main_chain, name="vcr-threads-rebuild", daemon=True).start()
    return {"ok": True, "loading": True, **info}


class BenchmarkRequest(BaseModel):
    runs: int = 10
    warmup: int = 2
    channel: str = "det"
    # 可选：临时用该线程数测速（不改变当前设置，也不写入会话槽位）
    threads: int | None = None


@app.post("/benchmark")
def benchmark(req: BenchmarkRequest):
    """FEAT-053：固定张量测速 —— CPU/GPU 真实加速比一键对比。

    channel 可选 det / face_det / face_rec / ocr / clip_vision / clip_text。
    可能触发模型加载与数十次推理（秒级耗时），宿主健康探测不经过此端点；
    Rust 侧对该调用使用独立长超时（默认 HTTP 客户端 15s 可能不够）。
    """
    try:
        return get_registry().benchmark(req.channel, runs=req.runs, warmup=req.warmup,
                                       threads=req.threads)
    except RuntimeError as e:
        raise HTTPException(status_code=503, detail=str(e))


class SweepRequest(BaseModel):
    channel: str = "clip_vision"
    options: list[int] | None = None
    runs: int = 8
    warmup: int = 2


@app.post("/benchmark_sweep")
def benchmark_sweep(req: SweepRequest):
    """线程数扫档：一次请求把若干线程数都测一遍，返回 [{threads, avg_ms, best}]。

    可能触发多次会话创建与推理（秒级到十几秒），用长超时（Rust 侧独立 180s）。
    """
    try:
        return {"results": get_registry().benchmark_sweep(
            req.channel, req.options, runs=req.runs, warmup=req.warmup)}
    except RuntimeError as e:
        raise HTTPException(status_code=503, detail=str(e))


@app.post("/classify")
def classify(req: ClassifyRequest):
    r = classify_one(req.path, get_registry())
    if r is None:
        raise HTTPException(400, f"无法读取图片: {req.path}")
    return r


@app.post("/classify_batch")
def classify_batch(req: ClassifyBatchRequest):
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
            results.append(r.model_dump())
    return {"results": results}


# ---------------------------------------------------------------------------
# 语义（Chinese-CLIP 双塔，可选通道；模型缺失时 503，宿主侧降级）
# ---------------------------------------------------------------------------
@app.get("/embed_status")
def embed_status():
    """CLIP 子系统状态（含档位 / 拆分件 / tokenizer 就绪详情，不触发加载）。"""
    return get_embed_service().status()


@app.post("/embed_text")
def embed_text(req: EmbedTextRequest):
    try:
        vec = get_embed_service().embed_text(req.text)
    except HTTPException:
        raise
    except Exception as e:  # noqa: BLE001
        raise HTTPException(503, f"CLIP 文本编码失败（{type(e).__name__}: {e}）")
    return {"dim": int(vec.shape[0]), "embedding": [round(float(x), 6) for x in vec]}


@app.post("/embed_text_batch")
def embed_text_batch(req: EmbedTextBatchRequest):
    """批量文本 → 向量（语义分类：用户分类关键词 + 中性基线提示词一次编码）。

    单次封顶 64 条（一次前向即可完成），返回顺序与请求一致；
    单条失败不影响整批（错误内联）。
    """
    texts = [t for t in req.texts][:64]
    if not texts:
        return {"dim": 0, "results": []}
    svc = get_embed_service()
    try:
        vecs = svc.embed_texts(texts)
    except HTTPException:
        raise
    except Exception as e:  # noqa: BLE001
        # 必须变成「带 detail 的 JSON 错误」：裸 500 的 text/plain 响应体会让宿主
        # reqwest 解析失败（error decoding response body），用户侧只看到语义静默失效
        # （BUG-2026-0920-005）
        raise HTTPException(503, f"CLIP 文本编码失败（{type(e).__name__}: {e}）")
    return {
        "dim": int(vecs[0].shape[0]),
        "model": svc.tier_info()["id"],
        "results": [
            {"text": t, "embedding": [round(float(x), 6) for x in v]}
            for t, v in zip(texts, vecs)
        ],
    }


@app.post("/embed_batch")
def embed_batch(req: EmbedBatchRequest):
    svc = get_embed_service()
    try:
        if not svc.ready():
            # 未就绪时不阻塞首请求：ensure 同步拆图+加载（首次数十秒），仍失败则明确 503
            svc.ensure()
        if not svc.ready():
            raise HTTPException(503, f"CLIP 未就绪: {svc.status().get('error') or '模型缺失'}")
        paths = req.paths[: config.BATCH_CHUNK_MAX]
        return {"results": svc.embed_images(paths)}
    except HTTPException:
        raise
    except Exception as e:  # noqa: BLE001
        raise HTTPException(503, f"CLIP 图像编码失败（{type(e).__name__}: {e}）")


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
    """后台预热规则主链路（启动预加载与 GPU 切换后重建共用）。

    仅 det（每张图必经）；face/ocr 按需惰性加载，CLIP 由首次语义请求触发。
    """
    import sys

    try:
        st = get_registry().preload_main()
        print(f"[VCR] 规则主链路模型状态: {st}", file=sys.stderr)
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

    # 模型后台预热（仅 det）：uvicorn 先绑定端口（/health 立即可达，加载中返回
    # ok=false，宿主进入 Loading 等待），模型在后台线程加载。
    _quiet_proactor_noise()
    threading.Thread(target=_rebuild_main_chain, name="vcr-preload", daemon=True).start()
    print(f"[VCR] 接口层启动 http://127.0.0.1:{port}", file=sys.stderr)
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")
