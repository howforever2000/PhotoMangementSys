"""本地 ONNX 模型推理（Real-ESRGAN 超分 / GFPGAN 人脸修复 / SCRFD 人脸检测）。

只做「模型会话管理 + 三种前向推理」这一件事，供 restore 工作流复用：
  - get_session    按路径缓存 onnxruntime 会话，优先 DirectML，失败回落 CPU
  - run_esrgan     分块超分（4× 模型），避免大图一次性占满内存
  - run_gfpgan     单张 512 对齐脸的修复
  - detect_faces   交给 facedetect 做 SCRFD 检测

onnxruntime 采用惰性导入：胶片/颜色/其它工具在没装 ORT 的环境也能正常跑，
只有真正调用超分/人脸时才要求它可用。
"""
from __future__ import annotations

import os
import threading
from typing import Optional

import numpy as np
from PIL import Image

from . import config, facedetect, gpu_lock
from .logutil import call_model

# 会话缓存 + 文件指纹：模型被重新下载/替换后自动重建（否则旧会话会一直用旧权重）。
# 键是 (绝对路径, intra 线程数, 是否强制 CPU)：同模型不同配置必须是不同会话，
# 否则后一种请求会静默拿到前一种的线程数 / 设备（见 get_session 的 docstring）。
_sessions: dict[tuple[str, int, bool], object] = {}
_session_stamp: dict[tuple[str, int, bool], tuple[float, int]] = {}
_sessions_lock = threading.Lock()


def _ort():
    import onnxruntime as ort  # 惰性导入

    return ort


def _providers() -> list[str]:
    ort = _ort()
    avail = set(ort.get_available_providers())
    pref = ["DmlExecutionProvider", "CUDAExecutionProvider", "CPUExecutionProvider"]
    chosen = [p for p in pref if p in avail]
    return chosen or ["CPUExecutionProvider"]


def _session_options(ort, intra: int | None = None) -> "object":
    """统一 SessionOptions：intra 按档位、inter=1（对齐 VCR 的线程口径）。

    为什么必须显式设：ai_studio 同时有「照片并行 P + 预览线程池」，
    ORT 默认按满核开 intra 线程，多会话叠加会 CPU 超订、互相抢核反而更慢。

    `intra` 缺省走 `config.ONNX_INTRA_OP`（大模型口径）。小模型传
    `config.ONNX_INTRA_OP_SMALL`：SCRFD/peppa/BiSeNet/rec 单次前向 3~60ms，
    线程铺满时调度开销占大头，实测反而更慢。
    """
    so = ort.SessionOptions()
    so.intra_op_num_threads = int(intra) if intra and intra > 0 else config.ONNX_INTRA_OP
    so.inter_op_num_threads = 1
    return so


def _file_stamp(path: str) -> tuple[float, int]:
    st = os.stat(path)
    return (st.st_mtime, st.st_size)


def get_session(path: str, intra: int | None = None, cpu_only: bool = False):
    """按绝对路径缓存会话；文件缺失时抛出带中文提示的 FileNotFoundError。

    并发首次加载会被 `_sessions_lock` 串行化：多个作业线程 + 预览线程同时要同一个
    模型时，只建一份会话（否则后建者覆盖前者，DML 显存要等 GC 才回收）。
    文件 mtime/size 变化（用户重新下载模型）时重建会话，避免拿旧权重跑到进程重启。

    两个可选参数都进了缓存键 —— 同一模型用不同线程数/设备建会话是**两个不同会话**，
    否则第二次请求会静默拿到第一次的配置：
      - `intra`：intra_op 线程数，缺省走全局大模型口径（见 `_session_options`）；
      - `cpu_only`：强制 CPU（OCR rec 专用：实测单框 CPU 5.8ms vs DML 16.1ms，
        且 CPU 会话不过 `gpu_lock`，不会把大模型的 GPU 队列堵住）。
    """
    if not os.path.isfile(path):
        raise FileNotFoundError(os.path.basename(path))
    key = os.path.abspath(path)
    intra_eff = int(intra) if intra and intra > 0 else config.ONNX_INTRA_OP
    cache_key = (key, intra_eff, bool(cpu_only))
    try:
        stamp = _file_stamp(key)
    except OSError:
        # isfile 与 stat 之间文件被移走 / 权限变化：统一报「缺失」而不是裸 OSError
        raise FileNotFoundError(os.path.basename(path)) from None
    with _sessions_lock:
        sess = _sessions.get(cache_key)
        if sess is not None and _session_stamp.get(cache_key) == stamp:
            return sess
        ort = _ort()
        providers = ["CPUExecutionProvider"] if cpu_only else _providers()
        try:
            sess = ort.InferenceSession(
                key,
                sess_options=_session_options(ort, intra_eff),
                providers=providers,
            )
        except Exception:
            # DML 建会话失败（驱动 / DX12 不可用）→ 回落 CPU，功能不受影响
            sess = ort.InferenceSession(
                key,
                sess_options=_session_options(ort, intra_eff),
                providers=["CPUExecutionProvider"],
            )
        _sessions[cache_key] = sess
        _session_stamp[cache_key] = stamp
        return sess


def esrgan_ready() -> bool:
    return os.path.isfile(config.local_model_path("real_esrgan_x4.onnx"))


def gfpgan_ready() -> bool:
    return os.path.isfile(config.local_model_path("gfpgan_1.4.onnx"))


def face_det_ready() -> bool:
    return os.path.isfile(config.FACE_DET_MODEL)


def model_ready(file_name: str) -> bool:
    """通用就绪判定：`<模型根>/local/<file>` 是否存在。

    新增本地模型一律走这里，避免每个模块各写一份路径拼接（口径必须与下载侧一致）。
    """
    return os.path.isfile(config.local_model_path(file_name))


def run_esrgan(img: Image.Image, tile: int = 256, overlap: int = 16, cancel=None) -> Image.Image:
    """4× 超分（分块推理，块间留 overlap 并只写核心区以避免接缝）。返回 PIL RGB。"""
    from .workflows.base import check_cancel

    sess = get_session(config.local_model_path("real_esrgan_x4.onnx"))
    iname = sess.get_inputs()[0].name
    img = img.convert("RGB")
    w, h = img.size
    arr = np.asarray(img, dtype=np.float32) / 255.0
    out = np.empty((h * 4, w * 4, 3), dtype=np.uint8)

    t = max(64, int(tile))
    o = max(0, min(int(overlap), t // 2 - 1))
    stride = max(1, t - 2 * o)
    # 单个工具函数内可能跑几十上百块：整段记一行（带块数），不逐块刷日志。
    with call_model("onnx", "real_esrgan_x4.onnx", f"{w}x{h} tile={t} stride={stride}"):
        _esrgan_tiles(arr, out, iname, sess, h, w, t, o, stride, check_cancel, cancel)
    return Image.fromarray(out)


def _esrgan_tiles(arr, out, iname, sess, h, w, t, o, stride, check_cancel, cancel):
    """ESRGAN 分块推理：块间留 overlap 并只写核心区以避免接缝。"""
    for y in range(0, h, stride):
        y0 = min(y, h - 1)
        y1 = min(y0 + t, h)
        for x in range(0, w, stride):
            x0 = min(x, w - 1)
            x1 = min(x0 + t, w)
            check_cancel(cancel)
            sub = arr[y0:y1, x0:x1]
            xin = sub.transpose(2, 0, 1)[None]
            res = gpu_lock.run(sess, {iname: xin})[0][0]  # (3, 4h, 4w)
            res = np.clip(res.transpose(1, 2, 0), 0.0, 1.0)
            res_u8 = (res * 255.0 + 0.5).astype(np.uint8)
            # 只写核心区（边缘块保留到图像边界）
            ly0 = o if y0 > 0 else 0
            lx0 = o if x0 > 0 else 0
            ly1 = (sub.shape[0] - o) if y1 < h else sub.shape[0]
            lx1 = (sub.shape[1] - o) if x1 < w else sub.shape[1]
            out[(y0 + ly0) * 4 : (y0 + ly1) * 4, (x0 + lx0) * 4 : (x0 + lx1) * 4] = res_u8[
                ly0 * 4 : ly1 * 4, lx0 * 4 : lx1 * 4
            ]


def run_gfpgan(face512: Image.Image) -> Image.Image:
    """GFPGAN 1.4：输入 512×512 人脸，输出同尺寸修复结果。"""
    sess = get_session(config.local_model_path("gfpgan_1.4.onnx"))
    iname = sess.get_inputs()[0].name
    face = face512.convert("RGB").resize((512, 512), Image.LANCZOS)
    x = np.asarray(face, dtype=np.float32) / 255.0
    x = (x - 0.5) / 0.5
    x = x.transpose(2, 0, 1)[None]
    with call_model("onnx", "gfpgan_1.4.onnx", "512x512 face"):
        out = gpu_lock.run(sess, {iname: x})[0][0]
    y = np.clip(out.transpose(1, 2, 0) * 0.5 + 0.5, 0.0, 1.0)
    return Image.fromarray((y * 255.0 + 0.5).astype(np.uint8))


def detect_faces(img: Image.Image) -> list[dict]:
    """SCRFD 检测，返回 [{bbox,kps,score}]。

    小模型档位：单次 640 前向只有 10ms 级，铺满线程是纯亏（见 config.ONNX_INTRA_OP_SMALL）。
    """
    sess = get_session(config.FACE_DET_MODEL, intra=config.ONNX_INTRA_OP_SMALL)
    return facedetect.detect(sess, img)


def restore_faces(upscaled: Image.Image, faces: list[dict], ratio: float) -> Image.Image:
    """把每张检测到的人脸从「已超分图」裁出、GFPGAN 修复后羽化贴回。"""
    out = upscaled.copy()
    cw, ch = out.size
    for f in faces:
        x1, y1, x2, y2 = f["bbox"]
        bw, bh = x2 - x1, y2 - y1
        if bw <= 0 or bh <= 0:
            continue
        # 映射到超分图坐标 + 外扩边距（给 GFPGAN 更多上下文）
        mx, my = int(bw * 0.35 * ratio), int(bh * 0.35 * ratio)
        bx1 = max(0, int(x1 * ratio) - mx)
        by1 = max(0, int(y1 * ratio) - my)
        bx2 = min(cw, int(x2 * ratio) + mx)
        by2 = min(ch, int(y2 * ratio) + my)
        if bx2 - bx1 < 16 or by2 - by1 < 16:
            continue
        crop = out.crop((bx1, by1, bx2, by2))
        fixed = run_gfpgan(crop).resize((bx2 - bx1, by2 - by1), Image.LANCZOS)
        mask = _feather_mask(bx2 - bx1, by2 - by1)
        out.paste(fixed, (bx1, by1), mask)
    return out


def _feather_mask(w: int, h: int) -> Image.Image:
    ys = np.linspace(-1.0, 1.0, h, dtype=np.float32)[:, None]
    xs = np.linspace(-1.0, 1.0, w, dtype=np.float32)[None, :]
    d = np.sqrt(xs * xs + ys * ys) / np.sqrt(2.0)
    alpha = np.clip((1.0 - d) / 0.35, 0.0, 1.0)  # 中心 1，边缘渐隐
    return Image.fromarray((alpha * 255).astype(np.uint8), mode="L")
