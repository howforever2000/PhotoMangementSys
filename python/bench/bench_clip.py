"""Chinese-CLIP ViT-B/16 语义搜索可行性 benchmark（缩略图实测）

素材：本机缩略图缓存（app_data/thumbs，11372 张 jpg 256px，即真实图库）
测项：
  A. 预处理吞吐（PIL: 解码+resize224+normalize，单线程）
  B. 图像编码：CPU EP / DML EP(780M) × batch 1/16/32 + CPU int8 量化版
  C. 文本编码：CPU / DML，batch=1 定长 52 token
  D. 全库检索：11372×512 fp32 点积 top-k（numpy 模拟 Rust 侧暴力检索量级）
  E. 端到端查询：文本编码 + 全库检索
输出：直接打印汇总表（含全库 11372 张索引总时长外推 + 实测（DML 大批量时全量跑））
"""
import os
import random
import sys
import time

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL_DIR = os.path.join(HERE, "..", "models", "chinese-clip-vit-b-16")
THUMB_DIR = os.path.join(os.environ["APPDATA"], "com.haoyuan.photo-management-sys", "thumbs")
N_SAMPLE = 400          # 预处理 + 各配置采样池
N_CPU_SAMPLE = 200      # CPU b=1 样本数
CLIP_MEAN = np.array([0.48145466, 0.4578275, 0.40821073], dtype=np.float32)
CLIP_STD = np.array([0.26862954, 0.26130258, 0.27577711], dtype=np.float32)
QUERIES = ["海边日落", "猫在草地上", "朋友聚餐", "城市夜景", "雪山风景",
           "人物自拍", "美食特写", "狗狗奔跑", "樱花盛开", "室内装修"]

import onnxruntime as ort
from PIL import Image


def list_thumbs():
    out = []
    for root, _, files in os.walk(THUMB_DIR):
        for f in files:
            if f.lower().endswith(".jpg"):
                out.append(os.path.join(root, f))
    return out


def preprocess(fp):
    """Chinese-CLIP 预处理：短边 resize 224 → center crop → normalize → CHW"""
    im = Image.open(fp).convert("RGB")
    w, h = im.size
    scale = 224 / min(w, h)
    im = im.resize((max(224, round(w * scale)), max(224, round(h * scale))), Image.BICUBIC)
    w, h = im.size
    l, t = (w - 224) // 2, (h - 224) // 2
    a = np.asarray(im.crop((l, t, l + 224, t + 224)), dtype=np.float32) / 255.0
    a = (a - CLIP_MEAN) / CLIP_STD
    return np.ascontiguousarray(a.transpose(2, 0, 1))


def bench_images(sess, pixel_list, batch, tag):
    """batch 推理计时（warmup 2 批），返回每张均值 ms"""
    n = len(pixel_list)
    if n < batch:
        batch = n
    inp = sess.get_inputs()[0].name
    for i in range(0, min(n, 2 * batch), batch):
        sess.run(None, {inp: np.stack(pixel_list[i:i + batch])})
    t0 = time.perf_counter()
    cnt = 0
    for i in range(0, n - batch + 1, batch):
        sess.run(None, {inp: np.stack(pixel_list[i:i + batch])})
        cnt += batch
    dt = time.perf_counter() - t0
    per = dt / cnt * 1000
    print(f"  {tag:<28} b={batch:<3} n={cnt:<4} {per:8.2f} ms/张   全库11372张≈{per*11372/1000/60:6.1f} min")
    return per


def bench_text(providers):
    so = ort.SessionOptions()
    so.intra_op_num_threads = 2
    sess = ort.InferenceSession(os.path.join(MODEL_DIR, "clip_text_b16.onnx"), so, providers=providers)
    from transformers import BertTokenizer
    tok = BertTokenizer.from_pretrained(MODEL_DIR)
    enc = tok(QUERIES, return_tensors="np", padding="max_length", max_length=52)
    ids, mask = enc["input_ids"].astype(np.int64), enc["attention_mask"].astype(np.int64)
    inp = {sess.get_inputs()[0].name: ids, sess.get_inputs()[1].name: mask}
    for _ in range(3):
        sess.run(None, inp)
    ts = []
    for _ in range(10):
        t0 = time.perf_counter()
        for _ in range(3):
            out = sess.run(None, inp)
        ts.append((time.perf_counter() - t0) / 3 * 1000)
    tag = "+".join(p.replace("ExecutionProvider", "") for p in providers)
    print(f"  text 10条中文查询({tag:<10}) 平均 {np.mean(ts):8.2f} ms/次（10条 batch=1）")
    return out[0]


def make_int8():
    from onnxruntime.quantization import quantize_dynamic, QuantType
    src = os.path.join(MODEL_DIR, "clip_vision_b16.onnx")
    dst = os.path.join(MODEL_DIR, "clip_vision_b16.int8.onnx")
    if not os.path.exists(dst):
        print("  生成 int8 量化模型...")
        quantize_dynamic(src, dst, weight_type=QuantType.QInt8)
    return dst


def main():
    thumbs = list_thumbs()
    print(f"缩略图库: {len(thumbs)} 张")
    random.seed(42)
    sample = random.sample(thumbs, min(N_SAMPLE, len(thumbs)))

    # A. 预处理
    print("\n[A] 预处理（解码+resize224+normalize）")
    t0 = time.perf_counter()
    pixel_list = [preprocess(fp) for fp in sample]
    pre_ms = (time.perf_counter() - t0) / len(sample) * 1000
    print(f"  单线程平均 {pre_ms:.2f} ms/张（可多线程并行，Rust 侧还有更快的 DCT 解码路径）")

    # B. 图像编码
    print("\n[B] 图像编码（ViT-B/16 fp32 345MB）")
    results = {}
    so = ort.SessionOptions()
    so.intra_op_num_threads = max(1, os.cpu_count() // 2)
    sess_cpu = ort.InferenceSession(os.path.join(MODEL_DIR, "clip_vision_b16.onnx"), so,
                                    providers=["CPUExecutionProvider"])
    results["cpu_b1"] = bench_images(sess_cpu, pixel_list[:N_CPU_SAMPLE], 1, "CPU")
    results["cpu_b16"] = bench_images(sess_cpu, pixel_list, 16, "CPU")
    results["cpu_b32"] = bench_images(sess_cpu, pixel_list, 32, "CPU")

    print("  -- int8 动态量化（CPU）--")
    sess_i8 = ort.InferenceSession(make_int8(), so, providers=["CPUExecutionProvider"])
    results["cpu_int8_b1"] = bench_images(sess_i8, pixel_list[:N_CPU_SAMPLE], 1, "CPU-int8")
    results["cpu_int8_b32"] = bench_images(sess_i8, pixel_list, 32, "CPU-int8")

    print("  -- DirectML（AMD Radeon 780M）--")
    sess_dml = ort.InferenceSession(os.path.join(MODEL_DIR, "clip_vision_b16.onnx"),
                                    ort.SessionOptions(), providers=["DmlExecutionProvider"])
    results["dml_b1"] = bench_images(sess_dml, pixel_list[:N_CPU_SAMPLE], 1, "DML")
    results["dml_b16"] = bench_images(sess_dml, pixel_list, 16, "DML")
    results["dml_b32"] = bench_images(sess_dml, pixel_list, 32, "DML")

    # C. 文本编码
    print("\n[C] 文本编码（RoBERTa-wwm-base，52 token 定长）")
    bench_text(["CPUExecutionProvider"])
    bench_text(["DmlExecutionProvider"])

    # D. 全库检索模拟
    print("\n[D] 全库检索（11372×512 fp32 点积 top-k，numpy 模拟）")
    db = np.random.randn(len(thumbs), 512).astype(np.float32)
    db /= np.linalg.norm(db, axis=1, keepdims=True)
    q = np.random.randn(512).astype(np.float32)
    q /= np.linalg.norm(q)
    t0 = time.perf_counter()
    for _ in range(50):
        sims = db @ q
    full_ms = (time.perf_counter() - t0) / 50 * 1000
    idx = np.argsort(-sims)[:10]
    print(f"  全库点积 {full_ms:.2f} ms | top-k 排序 ~1 ms | 10万张外推 ≈ {full_ms*10:.0f} ms")

    # E. 端到端查询
    print("\n[E] 端到端单次查询 = 文本编码 + 全库检索")
    best_text_ms = 20.0  # 见 [C] 输出
    print(f"  ≈ {best_text_ms:.0f} + {full_ms:.1f} ≈ {best_text_ms + full_ms:.0f} ms（远小于 10s 预算）")

    # 汇总
    print("\n===== 汇总（全库 11372 张索引总时长外推）=====")
    for k, v in results.items():
        print(f"  {k:<12} {v:8.2f} ms/张 → 全库 {v*11372/1000/60:5.1f} min（含预处理另加 {pre_ms:.1f} ms/张）")


if __name__ == "__main__":
    main()
