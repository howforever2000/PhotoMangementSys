"""Chinese-CLIP ViT-B/16 → ONNX 导出（vision / text 分开导出，供语义搜索）

输入模型: python/models/chinese-clip-vit-b-16/{config.json, vocab.txt, pytorch_model.bin}
产出:
  clip_vision_b16.onnx  输入 pixel_values (N,3,224,224) → 输出 embeds (N,512)  [未归一化]
  clip_text_b16.onnx    输入 input_ids (N,L) + attention_mask (N,L) → embeds (N,512)  [未归一化]

导出后做 torch vs onnxruntime 数值对齐校验（余弦相似度应 ≈ 1）。
"""
import os
import sys

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL_DIR = os.path.join(HERE, "..", "models", "chinese-clip-vit-b-16")
BATCH = 8

from transformers import ChineseCLIPModel, BertTokenizer


def export():
    model = ChineseCLIPModel.from_pretrained(MODEL_DIR)
    model.eval()

    class VisionWrap(torch.nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m

        def forward(self, pv):
            return self.m.get_image_features(pixel_values=pv)

    class TextWrap(torch.nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m

        def forward(self, ids, mask):
            return self.m.get_text_features(input_ids=ids, attention_mask=mask)

    # ---------- vision ----------
    pv = torch.randn(BATCH, 3, 224, 224)
    vwrap = VisionWrap(model).eval()
    with torch.no_grad():
        ref_img = vwrap(pv)
    torch.onnx.export(
        vwrap,
        (pv,),
        os.path.join(MODEL_DIR, "clip_vision_b16.onnx"),
        input_names=["pixel_values"],
        output_names=["embeds"],
        dynamic_axes={
            "pixel_values": {0: "batch"},
            "embeds": {0: "batch"},
        },
        opset_version=17,
        do_constant_folding=True,
    )
    print("vision exported, torch ref:", tuple(ref_img.shape))

    # ---------- text ----------
    tok = BertTokenizer.from_pretrained(MODEL_DIR)
    enc = tok(["海边日落", "一只猫"], return_tensors="pt", padding="max_length", max_length=52)
    ids, mask = enc["input_ids"], enc["attention_mask"]
    twrap = TextWrap(model).eval()
    with torch.no_grad():
        ref_txt = twrap(ids, mask)
    torch.onnx.export(
        twrap,
        (ids, mask),
        os.path.join(MODEL_DIR, "clip_text_b16.onnx"),
        input_names=["input_ids", "attention_mask"],
        output_names=["embeds"],
        dynamic_axes={
            "input_ids": {0: "batch", 1: "seq"},
            "attention_mask": {0: "batch", 1: "seq"},
            "embeds": {0: "batch"},
        },
        opset_version=17,
        do_constant_folding=True,
    )
    print("text exported, torch ref:", tuple(ref_txt.shape))

    # ---------- 数值对齐校验 ----------
    import onnxruntime as ort

    so = ort.SessionOptions()
    sess_v = ort.InferenceSession(
        os.path.join(MODEL_DIR, "clip_vision_b16.onnx"), so, providers=["CPUExecutionProvider"])
    out_v = sess_v.run(None, {"pixel_values": pv.numpy()})[0]
    sim_v = _cos(ref_img.numpy(), out_v)
    print("vision  cos(torch vs ort):", sim_v)

    sess_t = ort.InferenceSession(
        os.path.join(MODEL_DIR, "clip_text_b16.onnx"), so, providers=["CPUExecutionProvider"])
    out_t = sess_t.run(None, {"input_ids": ids.numpy(), "attention_mask": mask.numpy()})[0]
    sim_t = _cos(ref_txt.numpy(), out_t)
    print("text    cos(torch vs ort):", sim_t)

    assert sim_v > 0.999 and sim_t > 0.999, "数值对齐校验失败"
    print("PASS: 导出数值对齐校验通过")


def _cos(a, b):
    a = a.astype(np.float32).reshape(a.shape[0], -1)
    b = b.astype(np.float32).reshape(b.shape[0], -1)
    a /= (np.linalg.norm(a, axis=1, keepdims=True) + 1e-9)
    b /= (np.linalg.norm(b, axis=1, keepdims=True) + 1e-9)
    return float((a * b).sum(axis=1).mean())


if __name__ == "__main__":
    export()
