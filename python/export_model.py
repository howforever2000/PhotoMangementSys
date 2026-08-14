"""导出 YOLOv8n-cls ONNX 模型（零训练，官方预训练权重）

用法: python export_model.py
产出: python/models/yolov8n-cls.onnx (~5.5MB)
若 GitHub 下载 yolov8n-cls.pt 失败（网络问题），可先手动放置
yolov8n-cls.pt 到本目录，脚本会直接复用。
"""
import os
import sys

PT_PATH = os.path.join(os.path.dirname(__file__), "yolov8n-cls.pt")
ONNX_PATH = os.path.join(os.path.dirname(__file__), "models", "yolov8n-cls.onnx")


def main() -> int:
    from ultralytics import YOLO

    if os.path.exists(ONNX_PATH):
        print(f"[OK] 已存在: {ONNX_PATH}")
        return 0

    pt = PT_PATH if os.path.exists(PT_PATH) else "yolov8n-cls.pt"
    print(f"[1/2] 加载权重: {pt}")
    model = YOLO(pt)

    print("[2/2] 导出 ONNX (imgsz=224, simplify)...")
    model.export(format="onnx", imgsz=224, simplify=True, opset=12)
    print(f"[OK] 导出完成 -> {ONNX_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
