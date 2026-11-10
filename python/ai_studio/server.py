"""AI 创作微服务（FEAT-152）——接口层

分层（与 VCR 同构）：
  接口层  server.py（本文件，FastAPI 路由 + DTO 薄壳，无业务逻辑）
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

from . import config, jobs
from .logutil import log
from .providers import openai_compat
from .schemas import JobRequest, TestProviderRequest

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


@app.post("/providers/test")
def test_provider(req: TestProviderRequest) -> dict:
    p = req.provider
    return openai_compat.test_provider(req.kind, p.baseUrl, p.apiKey, p.model)


def main() -> None:
    port = config.port()
    log(f"启动 AI 创作服务 port={port}")
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")


if __name__ == "__main__":
    main()
