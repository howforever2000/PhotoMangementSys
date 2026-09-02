"""下载 VCR 所需全部模型（模型文件已 gitignore，首次部署需执行一次）

用法: python download_models.py [--mirror ghfast]
       python download_models.py --tasks cls
       python download_models.py --tasks scene     # 仅下载 Places365 模型权重+类目表
       python download_models.py --tasks all       # 下载 face/ocr/cls/scene
产出（python/models/）:
  分类   yolov8m-cls.onnx / yolov8s-cls.onnx / yolov8n-cls.onnx   （m 优先，依次回退）
  检测   yolov8n-det.onnx                       （COCO 80 类，person 检测）
  人脸   det_500m.onnx + w600k_mbf.onnx         （buffalo_sc：SCRFD + ArcFace）
  场景   resnet18_places365.onnx                （可选，需 export_places365.py 导出）
  OCR    paddleocr-det.onnx                     （RapidOCR 版 ch_PP-OCRv4 det，本脚本直接下载）
  花朵   efficientnet-b2-flowers.onnx           （可选：HF Lumia101 权重 + torch 导出，见下方说明）

花朵模型获取（已在本机完成，他机部署需重复）：
  curl -L -o /tmp/b2.safetensors https://hf-mirror.com/Lumia101/Flowers102-EfficientNet-B2/resolve/main/model.safetensors
  torch 加载到 torchvision.efficientnet_b2（classifier=Linear(1408,102)）+ softmax 导出 onnx
食物专家（food101）未采用：HF 无轻量 resnet50 权重，已用「ImageNet 映射兜底」替代（hot pot/蟹/龙虾/plate → food）。

场景通道（Places365，强烈推荐）：
  ImageNet 1k 是物体库，"风景/城市/室内"几乎不在其 top-1 中；
  Places365 有 365 个场景类（mountain/coast/forest/valley/abbey/street/...），
  才能在「自然风景 / 城市风光 / 室内 / 街道」上做准。CPU 推理单张 50-80ms。
  获取：python export_places365.py   （会自动下载 .pth.tar + categories_places365.txt 并导出 ONNX）
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
    "cls": {
        # yolov8m-cls（ultralytics 官方）—— ImageNet top-1 76.0%，~22.5MB；
        # 7840HS CPU 单张推理 ~30-40ms。
        "url": f"{GITHUB}/ultralytics/assets/releases/download/v8.3.0/yolov8m-cls.pt",
        "pt": "yolov8m-cls.pt",
        "files": ["yolov8m-cls.onnx"],
    },
}


def dl(url: str, dest: str, mirror: str | None):
    if mirror and url.startswith(GITHUB):
        url = f"{mirror}/{url}"
    print(f"[dl] {url} → {dest}")
    urllib.request.urlretrieve(url, dest)
    size = os.path.getsize(dest) / 1e6
    print(f"[dl] OK {size:.1f} MB")


def export_cls(model_path: str) -> None:
    """yolov8*-cls.pt → onnx（必须装 ultralytics：pip install ultralytics）"""
    try:
        from ultralytics import YOLO
    except ImportError:
        print("[cls] 缺少 ultralytics，无法导出 onnx。请 pip install ultralytics 后重跑。",
              file=sys.stderr)
        sys.exit(2)
    out_dir = MODEL_DIR
    os.makedirs(out_dir, exist_ok=True)
    print(f"[cls] 加载权重 {model_path}")
    model = YOLO(model_path)
    print("[cls] 导出 onnx (imgsz=224, simplify) ...")
    model.export(format="onnx", imgsz=224, simplify=True, opset=12)
    # ultralytics 默认导出到当前目录；移动到 models/
    src = os.path.join(os.path.dirname(model_path), "yolov8m-cls.onnx")
    dst = os.path.join(out_dir, "yolov8m-cls.onnx")
    if os.path.isfile(src) and src != dst:
        os.replace(src, dst)
    # 删除临时 .pt（如用户是按需下载）
    try:
        if os.path.abspath(model_path).startswith(os.path.abspath(out_dir)):
            os.remove(model_path)
    except OSError:
        pass
    print(f"[cls] OK -> {dst}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mirror", default="https://ghfast.top",
                        help="GitHub 加速镜像前缀（留空则直连）")
    parser.add_argument("--tasks", default="face,ocr",
                        help="逗号分隔：face/ocr/cls；另含独立步骤 export_places365.py 下载场景模型")
    args = parser.parse_args()
    os.makedirs(MODEL_DIR, exist_ok=True)
    mirror = args.mirror.strip() or None

    for task in args.tasks.split(","):
        if task not in TASKS:
            print(f"[skip] 未知任务 {task}", file=sys.stderr)
            continue
        spec = TASKS[task]
        # 所有目标文件已存在则跳过
        if all(os.path.isfile(os.path.join(MODEL_DIR, f)) for f in spec["files"]):
            print(f"[skip] {task} 模型已存在")
            continue
        if task == "ocr":
            dest = os.path.join(MODEL_DIR, spec["files"][0])
            dl(spec["url"], dest, None)
            continue
        if task == "cls":
            pt_tmp = os.path.join(MODEL_DIR, spec["pt"])
            dl(spec["url"], pt_tmp, mirror)
            export_cls(pt_tmp)
            continue
        # face: zip 多文件
        zip_path = os.path.join(MODEL_DIR, os.path.basename(spec["url"].split("?")[0]))
        dl(spec["url"], zip_path, mirror)
        with zipfile.ZipFile(zip_path) as z:
            for f in spec["files"]:
                z.extract(f, MODEL_DIR)
                print(f"[dl] 解压 {f}")

    print("[done] 模型就绪。")
    print("[hint] 场景模型（Places365）需单独执行：python export_places365.py")


if __name__ == "__main__":
    main()
