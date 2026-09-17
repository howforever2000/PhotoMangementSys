"""模型下载脚本（开发/CLI）：python download_models.py --tasks face,ocr,clip

服务内的模型下载走 Rust model_dl.rs（后台 + 进度 + 官方/镜像择优）；
本脚本用于开发环境或离线部署一次拉齐。

v5：分类模型（yolov8*-cls）与 Places365 场景模型已下线，不再下载；
    语义档位 = b16（fp16，默认）/ b16-fp32（可走 DirectML）；L/14 已实测否决下架。
"""
import argparse
import os
import sys
import urllib.request
import zipfile

from vcr import config

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL_DIR = os.path.join(HERE, "models")
GITHUB = "https://github.com"
HF = "https://hf-mirror.com"

TASKS = {
    "face": {
        # InsightFace buffalo_sc（SCRFD det_500m 2.5MB + ArcFace w600k_mbf 13.6MB）
        "url": f"{GITHUB}/deepinsight/insightface/releases/download/v0.7/buffalo_sc.zip",
        "files": ["det_500m.onnx", "w600k_mbf.onnx"],
    },
    "ocr": {
        # PaddleOCR ch_PP-OCRv4 det（RapidOCR 转换版，社区维护，无需 paddle 环境）
        "url": "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/det/ch_PP-OCRv4_det_mobile.onnx",
        "files": ["paddleocr-det.onnx"],
    },
}

# 语义模型档位：**由 config.CLIP_MODEL_META 派生**（单一事实源 = 档位表，
# 避免 CLI 与 App 内下载/Rust model_dl.rs 三处各写一份落位约定而漂移）。
# 任务别名（CLI 习惯叫法）→ 档位 key
_CLIP_TASK_ALIAS = {"b16": "clip", "b16-fp32": "clip-fp32"}
for _tier in config.CLIP_MODELS:
    _meta = config.CLIP_MODEL_META[_tier]
    TASKS[_CLIP_TASK_ALIAS[_tier]] = {
        # 落位： <root>/onnx/<onnx> 放双塔整图；tokenizer.json/vocab.txt 放模型根目录 <root>/
        # （附带文件必须在根目录，见 BUG-2026-0920-002）
        # 下载后需拆图（python extract_clip_subgraphs.py <tier>，服务端首次使用也会自动拆）
        "repo": _meta["repo"],
        "root": _meta["dir"],
        "files": [f"onnx/{_meta['onnx']}", "tokenizer.json", "vocab.txt"],
    }


def dl(url: str, dest: str, mirror: str | None = None):
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    if mirror and url.startswith(GITHUB):
        url = f"{mirror}/{url}"
    print(f"[dl] {url} → {dest}")
    urllib.request.urlretrieve(url, dest)
    print(f"[dl] OK {os.path.getsize(dest) / 1e6:.1f} MB")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mirror", default="https://ghfast.top",
                        help="GitHub 加速镜像前缀（留空则直连）")
    parser.add_argument("--tasks", default="face,ocr,clip",
                        help="逗号分隔：face/ocr/clip/clip-fp32")
    args = parser.parse_args()
    os.makedirs(MODEL_DIR, exist_ok=True)
    mirror = args.mirror.strip() or None

    for task in args.tasks.split(","):
        spec = TASKS.get(task)
        if not spec:
            print(f"[skip] 未知任务 {task}", file=sys.stderr)
            continue
        if "repo" in spec:
            base = f"{HF}/{spec['repo']}/resolve/main/"
            d = os.path.join(MODEL_DIR, spec["root"])
            if all(os.path.isfile(os.path.join(d, f)) for f in spec["files"]):
                print(f"[skip] {task} 模型已存在")
                continue
            for f in spec["files"]:
                dest = os.path.join(d, f)
                if os.path.isfile(dest):
                    continue
                dl(base + f, dest)
            continue
        # face: zip 多文件
        if all(os.path.isfile(os.path.join(MODEL_DIR, f)) for f in spec["files"]):
            print(f"[skip] {task} 模型已存在")
            continue
        if task == "ocr":
            dl(spec["url"], os.path.join(MODEL_DIR, spec["files"][0]))
            continue
        zip_path = os.path.join(MODEL_DIR, os.path.basename(spec["url"].split("?")[0]))
        dl(spec["url"], zip_path, mirror)
        with zipfile.ZipFile(zip_path) as z:
            for f in spec["files"]:
                z.extract(f, MODEL_DIR)
                print(f"[dl] 解压 {f}")

    print("[done] 模型就绪。")
    print("[hint] 语义模型首次使用前会拆双塔；也可先跑 python extract_clip_subgraphs.py [b16|b16-fp32]")


if __name__ == "__main__":
    main()
