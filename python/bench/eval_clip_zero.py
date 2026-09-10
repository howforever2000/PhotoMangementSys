"""Chinese-CLIP ViT-B/16 零样本分类 eval（对照 eval_vcr.py 的 yolov8m 76.0% 基线）

数据：python/ground_truth.json（53 张，Qwen3-VL 标注，11 类）
方法：11 类中文 prompt × 4 模板集成 → 类心归一化；原图 224 编码 → 余弦 argmax
对比：fp32 与 int8 动态量化两档准确率（决策门：int8 掉点 >2% 则默认 fp16）
"""
import json
import os
import sys
import time

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
MD = os.path.join(HERE, "..", "models", "chinese-clip-vit-b-16")
GT = json.load(open(os.path.join(HERE, "..", "ground_truth.json"), encoding="utf-8"))
TEST_DIR = GT["test_dir"]
CLIP_MEAN = np.array([0.48145466, 0.4578275, 0.40821073], dtype=np.float32)
CLIP_STD = np.array([0.26862954, 0.26130258, 0.27577711], dtype=np.float32)

CLASS_ZH = {
    "portrait": "人物特写人像", "street": "街道街景", "night_scene": "城市夜景",
    "plant_flower": "植物花朵", "food": "美食", "architecture": "建筑物",
    "vehicle": "车辆", "landscape_nature": "自然山水风景", "text": "文字文档屏幕",
    "other": "其他杂项", "animal": "动物",
}
TEMPLATES = ["一张{}的照片", "一张{}的图片", "这张照片展示的是{}", "{}"]

from PIL import Image  # noqa: E402
from transformers import BertTokenizer  # noqa: E402
import onnxruntime as ort  # noqa: E402


def preprocess(fp):
    im = Image.open(fp).convert("RGB")
    w, h = im.size
    scale = 224 / min(w, h)
    im = im.resize((max(224, round(w * scale)), max(224, round(h * scale))), Image.BICUBIC)
    w, h = im.size
    l, t = (w - 224) // 2, (h - 224) // 2
    a = np.asarray(im.crop((l, t, l + 224, t + 224)), dtype=np.float32) / 255.0
    a = (a - CLIP_MEAN) / CLIP_STD
    return np.ascontiguousarray(a.transpose(2, 0, 1))


def main():
    sess = ort.InferenceSession(os.path.join(MD, "clip_vision_b16.onnx"),
                                providers=["DmlExecutionProvider"])
    so = ort.SessionOptions()
    so.intra_op_num_threads = os.cpu_count()
    sess_i8 = ort.InferenceSession(os.path.join(MD, "clip_vision_b16.int8.onnx"), so,
                                   providers=["CPUExecutionProvider"])
    tsess = ort.InferenceSession(os.path.join(MD, "clip_text_b16.onnx"), so,
                                 providers=["CPUExecutionProvider"])
    tok = BertTokenizer.from_pretrained(MD)

    # 类心：模板集成均值
    classes = list(CLASS_ZH)
    prompts = [t.format(CLASS_ZH[c]) for c in classes for t in TEMPLATES]
    enc = tok(prompts, return_tensors="np", padding="max_length", max_length=52)
    feeds = {tsess.get_inputs()[0].name: enc["input_ids"].astype(np.int64),
             tsess.get_inputs()[1].name: enc["attention_mask"].astype(np.int64)}
    emb = tsess.run(None, feeds)[0].astype(np.float32)
    emb /= np.linalg.norm(emb, axis=1, keepdims=True)
    cent = np.vstack([emb[i * len(TEMPLATES):(i + 1) * len(TEMPLATES)].mean(0) for i in range(len(classes))])
    cent /= np.linalg.norm(cent, axis=1, keepdims=True)
    cls_idx = {c: i for i, c in enumerate(classes)}

    labels = GT["labels"]
    ms32, ms8, hit32, hit8 = [], [], 0, 0
    per_class = {c: [0, 0] for c in classes}
    for lab in labels:
        fp = os.path.join(TEST_DIR, lab["file"])
        px = preprocess(fp)[None]
        t0 = time.perf_counter()
        v32 = sess.run(None, {sess.get_inputs()[0].name: px})[0]
        ms32.append((time.perf_counter() - t0) * 1000)
        t0 = time.perf_counter()
        v8 = sess_i8.run(None, {sess_i8.get_inputs()[0].name: px})[0]
        ms8.append((time.perf_counter() - t0) * 1000)
        for v, hitf in ((v32, "f32"), (v8, "i8")):
            x = v.astype(np.float32) / np.linalg.norm(v)
            pred = classes[int(np.argmax(cent @ x[0]))]
            ok = pred == lab["gt"]
            if hitf == "f32":
                hit32 += ok
                per_class[lab["gt"]][0] += ok
                per_class[lab["gt"]][1] += 1
            else:
                hit8 += ok

    n = len(labels)
    print(f"样本 {n} 张 / {len(classes)} 类（中文 prompt 模板集成 {len(TEMPLATES)} 个）")
    print(f"fp32 (DML)  准确率 {hit32}/{n} = {hit32/n*100:.1f}%   平均 {np.mean(ms32):.0f} ms/张")
    print(f"int8 (CPU)  准确率 {hit8}/{n} = {hit8/n*100:.1f}%   平均 {np.mean(ms8):.0f} ms/张   掉点 {(hit32-hit8)/n*100:.1f}pp")
    print("基线: yolov8m-cls 现有管线 76.0% (eval_vcr.py)")
    print("\n各类 fp32 正确/总数:")
    for c in classes:
        k, t = per_class[c]
        if t:
            print(f"  {c:<18} {k}/{t}")


if __name__ == "__main__":
    main()
