"""官方 Chinese-CLIP（OFA-Sys PyTorch 权重）→ 双塔 ONNX 导出 + 数值自校验

为什么需要它：官方全家桶（RN50 / L/14 / H/14）**没有现成 ONNX**，只有 PyTorch 权重；
而本项目的语义链路只吃「双塔 ONNX」。本工具把 §3 准入流程的第 1 步自动化：
  下载权重 → 用 transformers 分塔导出 → torch vs onnxruntime 数值对齐校验（余弦应 ≈1）

用法：
  # L/14@224（候选强机档）
  python python/bench/export_clip_torch.py \\
      --repo OFA-Sys/chinese-clip-vit-large-patch14 \\
      --out  python/models/chinese-clip-l14-224 --size 224

  # 复用已有目录（跳过下载）
  python python/bench/export_clip_torch.py --repo ... --out ... --size 224 --skip-download

产出（<out>/ 下）：
  clip_vision.onnx   输入 pixel_values (N,3,S,S)  → 输出 embeds (N,Dim)  [未归一化]
  clip_text.onnx     输入 input_ids/attention_mask (N,L) → embeds (N,Dim)
  tokenizer.json / vocab.txt（fast 词表，缺失时由 vocab.txt 生成）
  权重文件默认导出后删除（--keep-weights 可保留）

注意：导出的两个塔与线上「整图拆图」产物**接口完全一致**（输入名 pixel_values /
input_ids / attention_mask，输出 embeds），因此可直接当作档位目录用
（照 <root>/clip_vision.onnx + <root>/clip_text.onnx + tokenizer.json 落位即可）。
"""
import argparse
import os
import shutil
import sys

import numpy as np
import torch

def log(*a) -> None:
    print(*a, flush=True)

sys.stdout.reconfigure(encoding="utf-8")
os.environ.setdefault("HF_ENDPOINT", "https://hf-mirror.com")  # hf-mirror 镜像（国内直连超时）

HERE = os.path.dirname(os.path.abspath(__file__))   # <repo>/python/bench
PY_DIR = os.path.dirname(HERE)                       # <repo>/python（sys.path 根）
REPO_ROOT = os.path.dirname(PY_DIR)                  # <repo>（--out 相对此目录解析）
WEIGHT_FILES = ("model.safetensors", "pytorch_model.bin")


def download(repo: str, out: str) -> None:
    from huggingface_hub import hf_hub_download

    os.makedirs(out, exist_ok=True)
    need = ["config.json", "vocab.txt"]
    for f in need:
        log(f"[dl] {f}")
        hf_hub_download(repo, f, local_dir=out)
    for f in WEIGHT_FILES:
        try:
            log(f"[dl] {f}（约 1~4GB，耐心等）")
            p = hf_hub_download(repo, f, local_dir=out)
            log("[dl] ok:", p, f"{os.path.getsize(p) / 1e6:.0f}MB")
            return
        except Exception as e:  # noqa: BLE001
            log(f"[dl] {f} 不可用（{type(e).__name__}），尝试下一个")
    raise SystemExit("权重下载失败")


def make_tokenizer(out: str) -> None:
    """尽量产出 fast tokenizer（tokenizer.json）；失败保留 vocab.txt（MiniBertTok 兜底）"""
    try:
        from transformers import ChineseCLIPProcessor

        proc = ChineseCLIPProcessor.from_pretrained(out)
        proc.tokenizer.save_pretrained(out)
        log("[tok] tokenizer.json 已生成:", os.path.isfile(os.path.join(out, "tokenizer.json")))
    except Exception as e:  # noqa: BLE001
        log(f"[tok] fast tokenizer 生成失败（{type(e).__name__}），保留 vocab.txt 兜底")


def export(out: str, size: int, max_len: int) -> None:
    from transformers import ChineseCLIPModel

    log(f"[exp] 加载模型（size={size}）…")
    model = ChineseCLIPModel.from_pretrained(out).eval()

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
    pv = torch.randn(2, 3, size, size)
    vwrap = VisionWrap(model).eval()
    with torch.no_grad():
        ref_img = vwrap(pv).numpy()
    vpath = os.path.join(out, "clip_vision.onnx")
    torch.onnx.export(
        vwrap, (pv,), vpath,
        input_names=["pixel_values"], output_names=["embeds"],
        dynamic_axes={"pixel_values": {0: "batch"}, "embeds": {0: "batch"}},
        opset_version=17, do_constant_folding=True,
    )
    log(f"[exp] vision → {vpath} ({os.path.getsize(vpath) / 1e6:.0f}MB) ref={ref_img.shape}")

    # ---------- text ----------
    from transformers import BertTokenizer

    tok = BertTokenizer.from_pretrained(out)
    enc = tok(["雪山", "海边日落"], return_tensors="pt", padding="max_length", max_length=max_len)
    ids, mask = enc["input_ids"], enc["attention_mask"]
    twrap = TextWrap(model).eval()
    with torch.no_grad():
        ref_txt = twrap(ids, mask).numpy()
    tpath = os.path.join(out, "clip_text.onnx")
    torch.onnx.export(
        twrap, (ids, mask), tpath,
        input_names=["input_ids", "attention_mask"], output_names=["embeds"],
        dynamic_axes={"input_ids": {0: "batch", 1: "seq"}, "attention_mask": {0: "batch", 1: "seq"},
                      "embeds": {0: "batch"}},
        opset_version=17, do_constant_folding=True,
    )
    log(f"[exp] text   → {tpath} ({os.path.getsize(tpath) / 1e6:.0f}MB) ref={ref_txt.shape}")

    # ---------- 数值对齐 ----------
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = 4
    sv = ort.InferenceSession(vpath, so, providers=["CPUExecutionProvider"])
    st = ort.InferenceSession(tpath, so, providers=["CPUExecutionProvider"])
    cos_v = _cos(ref_img, sv.run(None, {"pixel_values": pv.numpy()})[0])
    cos_t = _cos(ref_txt, st.run(None, {"input_ids": ids.numpy(), "attention_mask": mask.numpy()})[0])
    log(f"[chk] cos(torch vs ort): vision={cos_v:.6f} text={cos_t:.6f}  dim={ref_img.shape[1]}")
    if not (cos_v > 0.999 and cos_t > 0.999):
        raise SystemExit("数值对齐校验失败")


def _cos(a, b) -> float:
    a = a.astype(np.float32).reshape(a.shape[0], -1)
    b = b.astype(np.float32).reshape(b.shape[0], -1)
    a /= np.linalg.norm(a, axis=1, keepdims=True) + 1e-9
    b /= np.linalg.norm(b, axis=1, keepdims=True) + 1e-9
    return float((a * b).sum(axis=1).mean())


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", required=True, help="HF 仓库，如 OFA-Sys/chinese-clip-vit-large-patch14")
    ap.add_argument("--out", required=True, help="输出目录（相对仓库根或绝对路径）")
    ap.add_argument("--size", type=int, required=True, help="视觉输入边长（224 / 336）")
    ap.add_argument("--max-len", type=int, default=52, help="文本最大长度（CN-CLIP 官方 52）")
    ap.add_argument("--skip-download", action="store_true")
    ap.add_argument("--keep-weights", action="store_true", help="保留 PyTorch 权重（默认导出后删除）")
    args = ap.parse_args()

    out = args.out if os.path.isabs(args.out) else os.path.join(REPO_ROOT, args.out)
    if not args.skip_download or not os.path.isfile(os.path.join(out, "config.json")):
        download(args.repo, out)
    make_tokenizer(out)
    export(out, args.size, args.max_len)
    if not args.keep_weights:
        for f in WEIGHT_FILES:
            p = os.path.join(out, f)
            if os.path.isfile(p):
                os.remove(p)
                log("[clean] 删除权重", f)
    log("[done] =", out)
    for root, _dirs, files in os.walk(out):
        for f in sorted(files):
            fp = os.path.join(root, f)
            log("   ", os.path.relpath(fp, out), f"{os.path.getsize(fp) / 1e6:.1f}MB")


if __name__ == "__main__":
    main()
