"""CLIP 单文件双塔 → vision/text 拆分子图（一次性，幂等）

Xenova/chinese-clip-* 的整图（model_fp16.onnx / model.onnx）是单文件双塔（输入 pixel_values /
input_ids / attention_mask），直接 run 必须三输入齐喂（会白白跑另一塔）。
首次使用前拆成：
  clip_vision.onnx  输入 pixel_values → 输出 image_embeds (N,dim)
  clip_text.onnx    输入 input_ids + attention_mask → 输出 text_embeds (N,dim)

v5：路径与超参随「当前语义模型档位」变化（B/16 = 224/512，B/16 fp32 = 224/512，L/14-336 = 336/768）。
ensure_subgraphs() 幂等：拆分件存在且可加载则跳过；拆完用 onnxruntime 做与整图的
数值对齐校验（余弦 >0.999）。CLI：python extract_clip_subgraphs.py
"""
import os

import numpy as np

from .. import config


def _cos(a: np.ndarray, b: np.ndarray) -> float:
    a = a.astype(np.float32).reshape(len(a), -1)
    b = b.astype(np.float32).reshape(len(b), -1)
    a /= np.linalg.norm(a, axis=1, keepdims=True) + 1e-9
    b /= np.linalg.norm(b, axis=1, keepdims=True) + 1e-9
    return float((a * b).sum(axis=1).mean())


def _load_ort(path: str, providers: list[str]):
    import onnxruntime as ort

    return ort.InferenceSession(path, providers=providers)


def extract_subgraphs(model: str | None = None) -> dict:
    """从双塔整图（fp16/fp32 按档位）拆出 vision/text 两个子图并落盘。返回 {vision_ok, text_ok, error}。"""
    p = config.clip_paths(model)
    # onnx 只在「首次拆图」时需要；打包版必须把它打进包内，否则这里会以
    # ModuleNotFoundError 抛到 HTTP 层变成裸 500（BUG-2026-0920-005）
    try:
        import onnx
        from onnx.utils import extract_model
    except Exception as e:  # noqa: BLE001
        return {
            "vision_ok": False,
            "text_ok": False,
            "error": f"缺少拆图依赖 onnx（{type(e).__name__}: {e}）—— "
                     f"请 pip install onnx，或确认打包版已把 onnx 打进 exe",
        }

    os.makedirs(p["dir"], exist_ok=True)
    out: dict = {"vision_ok": False, "text_ok": False, "error": ""}
    if not os.path.isfile(p["whole"]):
        out["error"] = f"整图缺失: {p['whole']}"
        return out

    # onnx.load 校验 + 提取（external data 无需处理：整图单文件 <2GB）
    onnx.checker.check_model(
        onnx.load(p["whole"], load_external_data=False),
        full_check=False,
    )  # 结构级校验，失败直接抛
    try:
        if not os.path.isfile(p["vision"]):
            extract_model(
                p["whole"], p["vision"],
                input_names=["pixel_values"], output_names=["image_embeds"],
            )
        out["vision_ok"] = os.path.isfile(p["vision"])
        if not os.path.isfile(p["text"]):
            extract_model(
                p["whole"], p["text"],
                input_names=["input_ids", "attention_mask"], output_names=["text_embeds"],
            )
        out["text_ok"] = os.path.isfile(p["text"])
        if not (out["vision_ok"] and out["text_ok"]):
            out["error"] = "拆分后文件缺失"
        return out
    except Exception as e:  # noqa: BLE001
        out["error"] = f"{type(e).__name__}: {e}"
        return out


def verify_subgraphs(model: str | None = None, providers: list[str] | None = None) -> dict:
    """拆分件 vs 整图数值对齐校验（余弦相似度，应 ≈1）。"""
    p = config.clip_paths(model)
    if providers is None:
        import onnxruntime as ort

        providers = ort.get_available_providers()
    whole = _load_ort(p["whole"], providers)
    rng = np.random.default_rng(0)
    size, max_len = int(p["size"]), int(p["max_len"])
    pv = rng.standard_normal((2, 3, size, size)).astype(np.float32)
    ids = rng.integers(100, 1000, (2, max_len)).astype(np.int64)
    mask = np.ones_like(ids)
    ref_img = whole.run(["image_embeds"], {"pixel_values": pv, "input_ids": ids, "attention_mask": mask})[0]
    ref_txt = whole.run(["text_embeds"], {"pixel_values": pv, "input_ids": ids, "attention_mask": mask})[0]
    vis = _load_ort(p["vision"], providers).run(None, {"pixel_values": pv})[0]
    txt = _load_ort(p["text"], providers).run(
        None, {"input_ids": ids, "attention_mask": mask})[0]
    return {
        "vision_cos": round(_cos(ref_img, vis), 6),
        "text_cos": round(_cos(ref_txt, txt), 6),
        "pass": _cos(ref_img, vis) > 0.999 and _cos(ref_txt, txt) > 0.999,
    }


def ensure_subgraphs(model: str | None = None) -> str:
    """确保当前档位拆分件就绪；成功返回空串，失败返回错误描述（供 /health 与降级链）。"""
    p = config.clip_paths(model)
    if os.path.isfile(p["vision"]) and os.path.isfile(p["text"]):
        return ""
    r = extract_subgraphs(model)
    return "" if r["vision_ok"] and r["text_ok"] else r["error"]


if __name__ == "__main__":
    import sys

    sys.stdout.reconfigure(encoding="utf-8")
    r = extract_subgraphs()
    print("extract:", r)
    if r["vision_ok"] and r["text_ok"]:
        v = verify_subgraphs()
        print("verify:", v)
        sys.exit(0 if v["pass"] else 1)
    sys.exit(1)
