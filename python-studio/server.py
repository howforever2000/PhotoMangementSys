# -*- coding: utf-8 -*-
"""创意工坊微服务（FEAT-063，独立于 VCR 识别服务）
================================================================

职责：为「创意工坊」图片编辑小组件提供纯算法 HTTP 接口。
无模型、无持久化状态：进程一起监听即完全就绪（/health 立即可达），
生命周期由 Rust 侧 `studio.rs` 管理（探活 → 收养 → 启动，精简版 ensure）。

接口
----
GET  /health                  探活（返回 {ok, service, api}）
POST /api/region-equalize     区域直方图均衡化
     multipart: image(图片) + mask(灰度蒙版 PNG，白=选中)
                mode=global|clahe  strength=0~1  feather=0~60(px)
     返回: image/jpeg（处理后的整图，未选中区域保持原样）

算法要点
--------
- 只在 LAB 亮度通道 L 上做均衡，a/b 色度原样保留 → 不偏色；
- 蒙版按 feather 半径高斯羽化后与原图按像素线性混合，边缘过渡自然；
- strength 同时控制均衡强度与混合权重（0=原图，1=完全均衡）。

启动：python python-studio/server.py（端口经 STUDIO_PORT 环境变量传入）
依赖：fastapi / uvicorn / opencv-python / numpy（与 python/requirements.txt 同源）
"""

import os
import sys

import cv2
import numpy as np
import uvicorn
from fastapi import FastAPI, File, Form, HTTPException, Response, UploadFile
from fastapi.middleware.cors import CORSMiddleware

# 前端经 WebView2 fetch 直连本服务（tauri:// / http://tauri.localhost 等 origin），
# 统一放行跨域（服务只监听 127.0.0.1，无暴露面）
app = FastAPI(title="PM-Studio", docs_url=None, redoc_url=None)
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

API_VERSION = 1

# 均衡算法上限保护：单边超过该值的图先等比缩小再处理（内存/耗时可控）
MAX_SIDE = 6000


@app.get("/health")
def health():
    return {"ok": True, "service": "pm-studio", "api": API_VERSION}


def _decode(data: bytes, what: str, flags: int) -> np.ndarray:
    arr = np.frombuffer(data, np.uint8)
    img = cv2.imdecode(arr, flags)
    if img is None:
        raise HTTPException(status_code=400, detail=f"{what}解码失败")
    return img


def _clamp(v: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, v))


@app.post("/api/region-equalize")
async def region_equalize(
    image: UploadFile = File(...),
    mask: UploadFile = File(...),
    mode: str = Form("clahe"),
    strength: float = Form(0.8),
    feather: int = Form(6),
):
    """对蒙版选中的区域做直方图均衡化（LAB 亮度通道），返回整图 JPEG。"""
    raw = await image.read()
    img = _decode(raw, "图片", cv2.IMREAD_COLOR)

    mraw = await mask.read()
    mask_img = _decode(mraw, "蒙版", cv2.IMREAD_GRAYSCALE)
    if mask_img.shape[:2] != img.shape[:2]:
        mask_img = cv2.resize(
            mask_img, (img.shape[1], img.shape[0]), interpolation=cv2.INTER_NEAREST
        )

    # 参数整流
    mode = mode if mode in ("global", "clahe") else "clahe"
    strength = _clamp(float(strength), 0.0, 1.0)
    feather = int(_clamp(float(feather), 0, 60))

    # 超大图等比缩小（保持长宽比）
    h, w = img.shape[:2]
    if max(h, w) > MAX_SIDE:
        scale = MAX_SIDE / max(h, w)
        img = cv2.resize(img, (int(w * scale), int(h * scale)), interpolation=cv2.INTER_AREA)
        mask_img = cv2.resize(mask_img, (img.shape[1], img.shape[0]), interpolation=cv2.INTER_NEAREST)

    lab = cv2.cvtColor(img, cv2.COLOR_BGR2LAB)
    l_ch, a_ch, b_ch = cv2.split(lab)

    if mode == "global":
        eq = cv2.equalizeHist(l_ch)
    else:
        # clipLimit 随 strength 增强（1.0 附近≈轻处理，4.0 为常用上限）
        clip = 1.0 + strength * 3.0
        clahe = cv2.createCLAHE(clipLimit=clip, tileGridSize=(8, 8))
        eq = clahe.apply(l_ch)

    # 蒙版羽化 + 强度加权，与原亮度按像素线性混合
    m = mask_img.astype(np.float32) / 255.0
    if feather > 0:
        k = feather * 2 + 1
        m = cv2.GaussianBlur(m, (k, k), 0)
    m = m * strength
    l_out = (l_ch.astype(np.float32) * (1.0 - m) + eq.astype(np.float32) * m).clip(0, 255).astype(np.uint8)

    out = cv2.cvtColor(cv2.merge([l_out, a_ch, b_ch]), cv2.COLOR_LAB2BGR)
    ok, enc = cv2.imencode(".jpg", out, [int(cv2.IMWRITE_JPEG_QUALITY), 92])
    if not ok:
        raise HTTPException(status_code=500, detail="结果编码失败")
    return Response(content=enc.tobytes(), media_type="image/jpeg")


if __name__ == "__main__":
    port = int(os.environ.get("STUDIO_PORT", "8790"))
    print(f"[PM-Studio] 创意工坊微服务 http://127.0.0.1:{port}", file=sys.stderr)
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")
