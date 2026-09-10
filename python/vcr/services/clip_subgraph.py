"""CLIP 单文件双塔 → vision/text 拆分子图（一次性，幂等）

Xenova/chinese-clip-vit-base-patch16 的 model_fp16.onnx 是单文件双塔
（输入 pixel_values / input_ids / attention_mask），直接 run 必须三输入齐喂
（会白白跑另一塔）。首次使用前拆成：
  clip_vision.onnx  输入 pixel_values → 输出 image_embeds (N,512)
  clip_text.onnx    输入 input_ids + attention_mask → 输出 text_embeds (N,512)

ensure_subgraphs() 幂等：拆分件存在且可加载则跳过；拆完用 onnxruntime 做
与整图的数值对齐校验（余弦 >0.999）。CLI：python extract_clip_subgraphs.py
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


def extract_subgraphs() -> dict:
    """从 fp16 整图拆出 vision/text 两个子图并落盘。返回 {vision_ok, text_ok, error}。"""
    import onnx
    from onnx.utils import extract_model

    os.makedirs(config.CLIP_DIR, exist_ok=True)
    out: dict = {"vision_ok": False, "text_ok": False, "error": ""}
    if not os.path.isfile(config.CLIP_FP16_PATH):
        out["error"] = f"整图缺失: {config.CLIP_FP16_PATH}"
        return out

    # onnx.load 校验 + 提取（external data 无需处理：fp16 单文件 <2GB）
    onnx.checker.check_model(
        onnx.load(config.CLIP_FP16_PATH, load_external_data=False),
        full_check=False,
    )  # 结构级校验，失败直接抛
    try:
        if not os.path.isfile(config.CLIP_VISION_PATH):
            extract_model(
                config.CLIP_FP16_PATH,
                config.CLIP_VISION_PATH,
                input_names=["pixel_values"],
                output_names=["image_embeds"],
            )
        out["vision_ok"] = os.path.isfile(config.CLIP_VISION_PATH)
        if not os.path.isfile(config.CLIP_TEXT_PATH):
            extract_model(
                config.CLIP_FP16_PATH,
                config.CLIP_TEXT_PATH,
                input_names=["input_ids", "attention_mask"],
                output_names=["text_embeds"],
            )
        out["text_ok"] = os.path.isfile(config.CLIP_TEXT_PATH)
        if not (out["vision_ok"] and out["text_ok"]):
            out["error"] = "拆分后文件缺失"
        return out
    except Exception as e:  # noqa: BLE001
        out["error"] = f"{type(e).__name__}: {e}"
        return out


def verify_subgraphs(providers: list[str] | None = None) -> dict:
    """拆分件 vs 整图数值对齐校验（余弦相似度，应 ≈1）。"""
    if providers is None:
        import onnxruntime as ort

        providers = ort.get_available_providers()
    whole = _load_ort(config.CLIP_FP16_PATH, providers)
    rng = np.random.default_rng(0)
    pv = rng.standard_normal((2, 3, config.CLIP_SIZE, config.CLIP_SIZE)).astype(np.float32)
    ids = rng.integers(100, 1000, (2, config.CLIP_MAX_LEN)).astype(np.int64)
    mask = np.ones_like(ids)
    ref_img = whole.run(["image_embeds"], {"pixel_values": pv, "input_ids": ids, "attention_mask": mask})[0]
    ref_txt = whole.run(["text_embeds"], {"pixel_values": pv, "input_ids": ids, "attention_mask": mask})[0]
    vis = _load_ort(config.CLIP_VISION_PATH, providers).run(None, {"pixel_values": pv})[0]
    txt = _load_ort(config.CLIP_TEXT_PATH, providers).run(
        None, {"input_ids": ids, "attention_mask": mask})[0]
    return {
        "vision_cos": round(_cos(ref_img, vis), 6),
        "text_cos": round(_cos(ref_txt, txt), 6),
        "pass": _cos(ref_img, vis) > 0.999 and _cos(ref_txt, txt) > 0.999,
    }


def ensure_subgraphs() -> str:
    """确保拆分件就绪；成功返回空串，失败返回错误描述（供 /health 与降级链）。"""
    if os.path.isfile(config.CLIP_VISION_PATH) and os.path.isfile(config.CLIP_TEXT_PATH):
        return ""
    r = extract_subgraphs()
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
