# -*- coding: utf-8 -*-
"""创意工坊微服务（FEAT-063 建立，FEAT-066 重构为算子注册表）
================================================================

职责：为「创意工坊」图片编辑小组件提供纯算法 HTTP 接口。
无模型、无持久化状态：进程一起监听即完全就绪（/health 立即可达），
生命周期由 Rust 侧 `studio.rs` 管理（探活 → 收养 → 启动，精简版 ensure）。

接口
----
GET  /health            探活（返回 {ok, service, api}）—— 不记日志
GET  /api/ops           算子注册表（可序列化部分，供前端渲染参数表单）—— 不记日志
POST /api/apply         统一处理入口
     multipart: op(算子 id) + params(JSON 字符串) + image(图片) + mask(灰度蒙版 PNG，白=选中，可选)
     返回: image/jpeg（处理后的整图；无蒙版时按整图处理）

设计要点
--------
- 算子 = 纯函数 `fn(img, **params) -> img`：不含 HTTP、不含日志、不含蒙版混合；
- 蒙版语义（羽化 + 强度加权 + 线性混合）由 `_apply_masked` 一处实现，所有算子共用；
- 新增算子 = 往 `OPS` 里加一项，不需要新增路由。

算法要点
--------
- 均衡化只在 LAB 亮度通道 L 上做，a/b 色度原样保留 → 不偏色；
- 蒙版按 feather 半径高斯羽化后与原图按像素线性混合，边缘过渡自然；
- strength 控制混合权重（0=原图，1=完全采用算子结果）。

启动：python python-studio/server.py（端口经 STUDIO_PORT 环境变量传入）
依赖：fastapi / uvicorn / opencv-python / numpy（与 python/requirements.txt 同源）
"""

import json
import logging
import os
import sys
import time
from typing import Any, Callable, Dict, List, Optional

import cv2
import numpy as np
import uvicorn
from fastapi import FastAPI, File, Form, HTTPException, Response, UploadFile
from fastapi.middleware.cors import CORSMiddleware

# ---------------------------------------------------------------------------
# 执行日志（BUG-2026-0918-007：此前无任何日志，服务起不来/处理失败全靠猜）
# 路径由 Rust 侧经 STUDIO_LOG 注入（app 数据目录/studio-server.log）；
# 直接手动运行时缺省落在脚本旁，便于开发排错。
# 注意：/health 与 /api/ops 不记日志——前者被 ensure 每 250ms 探活一次，
# 后者前端打开面板就调，记了全是噪音。
# ---------------------------------------------------------------------------
LOG_PATH = os.environ.get(
    "STUDIO_LOG", os.path.join(os.path.dirname(os.path.abspath(__file__)), "studio-server.log")
)
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s | %(levelname)s | %(message)s",
    handlers=[
        logging.FileHandler(LOG_PATH, encoding="utf-8"),
        logging.StreamHandler(sys.stderr),
    ],
)
log = logging.getLogger("pm-studio")

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

# 处理上限保护：单边超过该值的图先等比缩小再处理（内存/耗时可控）
MAX_SIDE = 6000
# 结果编码质量（与 FEAT-063 一致）
JPEG_QUALITY = 92


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


# ---------------------------------------------------------------------------
# 公共混合逻辑（蒙版语义只有这一处实现）
# ---------------------------------------------------------------------------
def _apply_masked(
    img: np.ndarray,
    mask: Optional[np.ndarray],
    strength: float,
    feather: float,
    fn_result: np.ndarray,
) -> np.ndarray:
    """把算子结果按蒙版混合回原图。

    蒙版缩放对齐 → 高斯羽化 → 强度加权 → 与原图按像素线性混合：
        out = img * (1 - m) + fn_result * m,  m = feather(mask) * strength

    fn_result：算子 `fn(img, **params)` 的返回值，须与 img 同尺寸同通道。
    mask 为 None 时按整图处理（m 恒为 strength）。
    """
    if mask is None:
        m = np.full(img.shape[:2], _clamp(float(strength), 0.0, 1.0), np.float32)
    else:
        if mask.shape[:2] != img.shape[:2]:
            mask = cv2.resize(
                mask, (img.shape[1], img.shape[0]), interpolation=cv2.INTER_NEAREST
            )
        m = mask.astype(np.float32) / 255.0
        f = int(_clamp(float(feather), 0, 60))
        if f > 0:
            k = f * 2 + 1
            m = cv2.GaussianBlur(m, (k, k), 0)
        m = m * _clamp(float(strength), 0.0, 1.0)

    m3 = m[:, :, None]
    out = img.astype(np.float32) * (1.0 - m3) + fn_result.astype(np.float32) * m3
    return out.clip(0, 255).astype(np.uint8)


# ---------------------------------------------------------------------------
# 算子实现：纯函数 fn(img, **params) -> img（BGR uint8，同尺寸同通道）
# ---------------------------------------------------------------------------
def _odd(v: float, lo: int, hi: int) -> int:
    """钳制到 [lo, hi] 并保证为奇数（中值/高斯类核的要求）。"""
    k = int(round(float(v)))
    k = int(_clamp(k, lo, hi))
    return k if k % 2 == 1 else k + 1 if k + 1 <= hi else k - 1


def op_region_equalize(img: np.ndarray, mode: str = "clahe", strength: float = 0.8, **_) -> np.ndarray:
    """直方图均衡化：只在 LAB 的 L 通道上做，a/b 原样保留 → 不偏色。"""
    lab = cv2.cvtColor(img, cv2.COLOR_BGR2LAB)
    l_ch, a_ch, b_ch = cv2.split(lab)
    if mode == "global":
        eq = cv2.equalizeHist(l_ch)
    else:
        # clipLimit 随 strength 增强（1.0 附近≈轻处理，4.0 为常用上限）
        clip = 1.0 + _clamp(float(strength), 0.0, 1.0) * 3.0
        clahe = cv2.createCLAHE(clipLimit=clip, tileGridSize=(8, 8))
        eq = clahe.apply(l_ch)
    return cv2.cvtColor(cv2.merge([eq, a_ch, b_ch]), cv2.COLOR_LAB2BGR)


def op_blur_gaussian(img: np.ndarray, radius: int = 12, **_) -> np.ndarray:
    k = _odd(radius * 2 + 1, 3, 121)
    return cv2.GaussianBlur(img, (k, k), 0)


def op_blur_box(img: np.ndarray, radius: int = 12, **_) -> np.ndarray:
    k = _odd(radius * 2 + 1, 3, 121)
    return cv2.blur(img, (k, k))


def op_blur_median(img: np.ndarray, radius: int = 9, **_) -> np.ndarray:
    k = _odd(radius, 3, 31)
    return cv2.medianBlur(img, k)


def op_blur_bilateral(
    img: np.ndarray, d: int = 9, sigmaColor: float = 75.0, sigmaSpace: float = 75.0, **_
) -> np.ndarray:
    d = int(_clamp(int(round(float(d))), 3, 15))
    sc = _clamp(float(sigmaColor), 1.0, 150.0)
    ss = _clamp(float(sigmaSpace), 1.0, 150.0)
    return cv2.bilateralFilter(img, d, sc, ss)


# ---------------------------------------------------------------------------
# 算子注册表：新增算子只需在此追加一项（前端参数表单由 params schema 自动生成）
# ---------------------------------------------------------------------------
def _num(name: str, label: str, default: Any, min_v: float, max_v: float, step: float) -> Dict[str, Any]:
    return {
        "name": name,
        "label": label,
        "type": "number",
        "min": min_v,
        "max": max_v,
        "step": step,
        "default": default,
    }


def _enum(name: str, label: str, default: str, choices: List[Dict[str, str]]) -> Dict[str, Any]:
    return {"name": name, "label": label, "type": "enum", "choices": choices, "default": default}


#: 通用参数（所有算子共用：强度控制混合权重，羽化控制蒙版边缘过渡）
COMMON_PARAMS = [
    _num("strength", "强度", 1.0, 0.1, 1.0, 0.05),
    _num("feather", "羽化(px)", 8, 0, 60, 1),
]

OPS: List[Dict[str, Any]] = [
    {
        "id": "region-equalize",
        "label": "区域直方图均衡化",
        "supportsMask": True,
        "params": [
            _enum(
                "mode",
                "均衡方式",
                "clahe",
                [
                    {"value": "clahe", "label": "CLAHE（局部自适应）"},
                    {"value": "global", "label": "全局均衡"},
                ],
            ),
            _num("strength", "强度", 0.8, 0.1, 1.0, 0.05),
            _num("feather", "羽化(px)", 8, 0, 60, 1),
        ],
        "fn": op_region_equalize,
    },
    {
        "id": "blur-gaussian",
        "label": "高斯模糊",
        "supportsMask": True,
        "params": [_num("radius", "半径", 12, 1, 60, 1)] + COMMON_PARAMS,
        "fn": op_blur_gaussian,
    },
    {
        "id": "blur-box",
        "label": "方框/均值模糊",
        "supportsMask": True,
        "params": [_num("radius", "半径", 12, 1, 60, 1)] + COMMON_PARAMS,
        "fn": op_blur_box,
    },
    {
        "id": "blur-median",
        "label": "中值模糊",
        "supportsMask": True,
        "params": [_num("radius", "核大小(奇数)", 9, 3, 31, 2)] + COMMON_PARAMS,
        "fn": op_blur_median,
    },
    {
        "id": "blur-bilateral",
        "label": "双边滤波（磨皮）",
        "supportsMask": True,
        "params": [
            _num("d", "邻域直径", 9, 3, 15, 2),
            _num("sigmaColor", "颜色sigma", 75, 1, 150, 1),
            _num("sigmaSpace", "空间sigma", 75, 1, 150, 1),
        ]
        + COMMON_PARAMS,
        "fn": op_blur_bilateral,
    },
]

OPS_BY_ID: Dict[str, Dict[str, Any]] = {o["id"]: o for o in OPS}

_SUPPORTED = ", ".join(OPS_BY_ID)


@app.get("/api/ops")
def list_ops():
    """算子注册表（不含 fn，保证可 JSON 序列化）→ 供前端渲染参数表单。"""
    return {
        "ops": [
            {"id": o["id"], "label": o["label"], "supportsMask": o["supportsMask"], "params": o["params"]}
            for o in OPS
        ]
    }


def _coerce_params(entry: Dict[str, Any], raw: Dict[str, Any]) -> Dict[str, Any]:
    """按 schema 整流参数：类型归一 + 范围钳制 + 枚举回落默认值。"""
    values: Dict[str, Any] = {}
    for p in entry["params"]:
        name = p["name"]
        v = raw.get(name, p["default"])
        t = p["type"]
        if t == "number":
            try:
                v = float(v)
            except (TypeError, ValueError):
                v = float(p["default"])
            v = _clamp(v, float(p["min"]), float(p["max"]))
            # schema 默认值为整数 → 该参数按整数使用（半径/羽化等）
            if isinstance(p["default"], int) and not isinstance(p["default"], bool):
                v = int(round(v))
        elif t == "bool":
            v = bool(v)
        elif t == "enum":
            v = str(v)
            if v not in [c["value"] for c in p["choices"]]:
                v = p["default"]
        values[name] = v
    return values


@app.post("/api/apply")
async def apply(
    image: UploadFile = File(...),
    mask: Optional[UploadFile] = File(None),
    op: str = Form(...),
    params: str = Form("{}"),
):
    """统一处理入口：op 指定算子，params 为 JSON 字符串，蒙版可选（无蒙版=整图）。"""
    t0 = time.perf_counter()
    entry = OPS_BY_ID.get(op)
    if entry is None:
        log.warning("apply 拒绝：未知算子 | op=%s | 支持：%s", op, _SUPPORTED)
        raise HTTPException(status_code=400, detail=f"未知算子：{op}；支持：{_SUPPORTED}")

    try:
        try:
            raw_params = json.loads(params) if params else {}
            if not isinstance(raw_params, dict):
                raise ValueError("params 必须是 JSON 对象")
        except ValueError as e:
            raise HTTPException(status_code=400, detail=f"params 解析失败：{e}")

        values = _coerce_params(entry, raw_params)

        img = _decode(await image.read(), "图片", cv2.IMREAD_COLOR)
        mask_img = _decode(await mask.read(), "蒙版", cv2.IMREAD_GRAYSCALE) if mask is not None else None

        # 超大图等比缩小（保持长宽比；蒙版在 _apply_masked 里按新尺寸对齐）
        h, w = img.shape[:2]
        if max(h, w) > MAX_SIDE:
            scale = MAX_SIDE / max(h, w)
            img = cv2.resize(img, (int(w * scale), int(h * scale)), interpolation=cv2.INTER_AREA)

        fn: Callable[..., np.ndarray] = entry["fn"]
        fn_result = fn(img, **values)
        out = _apply_masked(
            img,
            mask_img,
            float(values.get("strength", 1.0)),
            float(values.get("feather", 0)),
            fn_result,
        )

        ok, enc = cv2.imencode(".jpg", out, [int(cv2.IMWRITE_JPEG_QUALITY), JPEG_QUALITY])
        if not ok:
            raise HTTPException(status_code=500, detail="结果编码失败")

        cover = float((mask_img > 127).mean()) * 100.0 if mask_img is not None else 100.0
        log.info(
            "%s ok | %dx%d | %s | 蒙版%s 选中%.1f%% | 耗时%.0fms | 输出%dKB",
            op,
            img.shape[1],
            img.shape[0],
            " ".join(f"{k}={v}" for k, v in values.items()),
            "无(整图)" if mask_img is None else "有",
            cover,
            (time.perf_counter() - t0) * 1000.0,
            enc.nbytes // 1024,
        )
        return Response(content=enc.tobytes(), media_type="image/jpeg")
    except HTTPException as e:
        log.warning(
            "%s 拒绝请求（%d）：%.200s | 耗时%.0fms",
            op,
            e.status_code,
            e.detail,
            (time.perf_counter() - t0) * 1000.0,
        )
        raise
    except Exception:
        log.exception("%s 处理异常 | 耗时%.0fms", op, (time.perf_counter() - t0) * 1000.0)
        raise


if __name__ == "__main__":
    port = int(os.environ.get("STUDIO_PORT", "8790"))
    log.info("PM-Studio 启动 | http://127.0.0.1:%d | python=%s | 日志=%s",
             port, sys.executable, LOG_PATH)
    uvicorn.run(app, host="127.0.0.1", port=port, log_level="warning")
