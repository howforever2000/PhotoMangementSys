"""下载 VCR 所需全部模型（模型文件已 gitignore，首次部署需执行一次）

用法: python download_models.py [--mirror ghfast]
产出（python/models/）:
  分类   yolov8s-cls.onnx / yolov8n-cls.onnx   （二选一即可，s 优先）
  检测   yolov8n-det.onnx                       （COCO 80 类，person 检测）
  人脸   det_500m.onnx + w600k_mbf.onnx         （buffalo_sc：SCRFD + ArcFace）
  场景   resnet18_places365.onnx                （可选，需 export_places365.py 导出）
  OCR    paddleocr-det.onnx                     （RapidOCR 版 ch_PP-OCRv4 det，本脚本直接下载）
  花朵   efficientnet-b2-flowers.onnx           （可选：HF Lumia101 权重 + torch 导出，见下方说明）

花朵模型获取（已在本机完成，他机部署需重复）：
  curl -L -o /tmp/b2.safetensors https://hf-mirror.com/Lumia101/Flowers102-EfficientNet-B2/resolve/main/model.safetensors
  torch 加载到 torchvision.efficientnet_b2（classifier=Linear(1408,102)）+ softmax 导出 onnx
食物专家（food101）未采用：HF 无轻量 resnet50 权重，已用「ImageNet 映射兜底」替代（hot pot/蟹/龙虾/plate → food）。
"""
import argparse
import os
import sys
import urllib.request
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL_DIR = os.path.join(HERE, "models")
GITHUB = "https://github.com"

TASKS = {
    "face": {
        "url": f"{GITHUB}/deepinsight/insightface/releases/download/v0.7/buffalo_sc.zip",
        "files": ["det_500m.onnx", "w600k_mbf.onnx"],
    },
    "ocr": {
        # PaddleOCR ch_PP-OCRv4 det（RapidOCR 转换版，社区维护，无需 paddle 环境）
        "url": "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/det/ch_PP-OCRv4_det_mobile.onnx",
        "files": ["paddleocr-det.onnx"],
    },
}


def dl(url: str, dest: str, mirror: str | None):
    if mirror and not url.startswith("http"):
        url = f"{mirror}/{url}"
    print(f"[dl] {url} → {dest}")
    urllib.request.urlretrieve(url, dest)
    size = os.path.getsize(dest) / 1e6
    print(f"[dl] OK {size:.1f} MB")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mirror", default="https://ghfast.top",
                        help="GitHub 加速镜像前缀（留空则直连）")
    parser.add_argument("--tasks", default="face,ocr", help="face/ocr 或逗号分隔多个")
    args = parser.parse_args()
    os.makedirs(MODEL_DIR, exist_ok=True)
    mirror = args.mirror.strip() or None

    for task in args.tasks.split(","):
        if task not in TASKS:
            print(f"[skip] 未知任务 {task}", file=sys.stderr)
            continue
        spec = TASKS[task]
        if task == "ocr":
            # 单文件直接下载（modelscope 不需要镜像）
            dest = os.path.join(MODEL_DIR, spec["files"][0])
            if not os.path.isfile(dest):
                dl(spec["url"], dest, None)
            else:
                print(f"[skip] {task} 模型已存在")
            continue
        zip_path = os.path.join(MODEL_DIR, os.path.basename(spec["url"].split("?")[0]))
        if not all(os.path.isfile(os.path.join(MODEL_DIR, f)) for f in spec["files"]):
            dl(spec["url"], zip_path, mirror)
            with zipfile.ZipFile(zip_path) as z:
                for f in spec["files"]:
                    z.extract(f, MODEL_DIR)
                    print(f"[dl] 解压 {f}")
        else:
            print(f"[skip] {task} 模型已存在")

    print("[done] 模型就绪。")


if __name__ == "__main__":
    main()
