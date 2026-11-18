"""AI 创作微服务（FEAT-152）——接口层

分层（与 VCR 同构）：
  接口层  server.py（本文件，FastAPI 路由 + DTO 薄壳，无业务逻辑）
  预览    preview.py（单张同步预览：只跑本机算子 → 内存 JPEG data URL，不落盘）
  工作流  workflows/（journal / beauty / style_transfer / upscale）
  原语    providers/openai_compat.py（OpenAI 兼容调用）+ compose.py（Pillow 排版/PDF）
  预设    presets.py

启动：`python -m ai_studio.server`（由 Rust 拉进程，端口经 AI_STUDIO_PORT 注入）
健康：GET /health → {"ok":true,"api_version":1,"service":"ai-studio"}
"""
from __future__ import annotations

import os
import threading
import time

import uvicorn
from fastapi import FastAPI, HTTPException
from fastapi.concurrency import run_in_threadpool

from . import config, jobs
from . import preview as preview_service
from .logutil import elog, log
from .providers import openai_compat
from .schemas import JobRequest, PreviewRequest, TestProviderRequest

app = FastAPI(title="AI Studio", docs_url=None, redoc_url=None)


@app.get("/health")
def health() -> dict:
    return {"ok": True, "api_version": config.API_VERSION, "service": config.SERVICE_NAME}


@app.post("/shutdown")
def shutdown() -> dict:
    def _bye() -> None:
        time.sleep(0.3)
        os._exit(0)

    threading.Thread(target=_bye, daemon=True).start()
    return {"ok": True}


@app.post("/jobs")
def create_job(req: JobRequest) -> dict:
    job_id = jobs.create(req)
    return {"jobId": job_id}


@app.get("/jobs/{job_id}")
def job_status(job_id: str) -> dict:
    view = jobs.get(job_id)
    if view is None:
        raise HTTPException(status_code=404, detail="job not found")
    return view


@app.post("/jobs/{job_id}/cancel")
def cancel_job(job_id: str) -> dict:
    return {"ok": jobs.cancel(job_id)}


@app.post("/preview")
async def preview_image(req: PreviewRequest) -> dict:
    """单张同步预览：本机算子 → 内存 JPEG data URL（不落盘）。

    必须 `run_in_threadpool`：本服务是单进程 uvicorn，图像处理是同步 CPU 活，
    在事件循环里直接跑会把并发的批量作业（提交 / 轮询 / 取消）一起卡住。
    业务编排全在 `preview.render`，本路由只做装配与 400 错误翻译。
    """
    try:
        return await run_in_threadpool(
            preview_service.render,
            req.path,
            req.recipeId,
            req.paramsOverride,
            req.plan,
            req.maxEdge,
        )
    except preview_service.PreviewError as e:
        raise HTTPException(status_code=400, detail=str(e)) from None
    except HTTPException:
        raise
    except Exception as e:  # 兜底：接口层绝不把裸栈丢给 Rust/前端
        elog(f"预览失败 path={req.path} err={e}")
        raise HTTPException(status_code=400, detail=f"预览失败: {e}") from None


@app.post("/providers/test")
def test_provider(req: TestProviderRequest) -> dict:
    p = req.provider
    return openai_compat.test_provider(req.kind, p.baseUrl, p.apiKey, p.model)


@app.get("/local/models")
def local_models() -> dict:
    """本地增强资源状态：大模型是否就绪 + 下载地址、内置 LUT 清单。

    供前端「本地模型」面板展示下载链接与「模型未就绪」提示。
    """
    models = []
    for m in config.LOCAL_MODELS:
        p = config.local_model_path(m["file"])
        exists = os.path.isfile(p)
        models.append(
            {
                **m,
                "path": p,
                "exists": exists,
                "sizeBytes": os.path.getsize(p) if exists else 0,
            }
        )
    luts = []
    for lut in config.LUTS:
        luts.append({**lut, "exists": os.path.isfile(config.lut_path(lut["file"]))})
    return {
        "models": models,
        "luts": luts,
        "modelsDir": config.LOCAL_MODELS_DIR,
        "lutsDir": config.LUTS_DIR,
    }


def main() -> None:
    port = config.port()
    log(f"启动 AI 创作服务 port={port}")
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")


if __name__ == "__main__":
    main()
