"""图像预处理：分类 / 检测 / 人脸对齐

统一入口接收 PIL.Image（各服务只解码一次图片）。
"""
import cv2
import numpy as np
from PIL import Image

from . import config

# ArcFace 112x112 对齐模板（标准 5 点）
ARC_DST = np.array(
    [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ],
    dtype=np.float32,
)


def open_image(path: str) -> Image.Image | None:
    try:
        with Image.open(path) as img:
            return img.convert("RGB")
    except Exception:
        return None


def cls_tensor(img: Image.Image) -> np.ndarray:
    """分类预处理：等比缩放短边=224 + 中心裁剪（不拉伸）。

    yolov8s-cls 导出 ONNX 已内置归一化，输入只需 [0,1] CHW。
    """
    w, h = img.size
    r = config.CLS_SIZE / min(w, h)
    img = img.resize((max(1, round(w * r)), max(1, round(h * r))), Image.BILINEAR)
    w2, h2 = img.size
    l = (w2 - config.CLS_SIZE) // 2
    t = (h2 - config.CLS_SIZE) // 2
    img = img.crop((l, t, l + config.CLS_SIZE, t + config.CLS_SIZE))
    arr = np.asarray(img, dtype=np.float32) / 255.0
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0)


def det_tensor(img: Image.Image) -> tuple[np.ndarray, float, int, int]:
    """检测预处理：letterbox 640 灰底填充。返回 (tensor, scale, pad_x, pad_y)。

    scale = 原图→letterbox 的缩放；pad 为左/上偏移。
    """
    w, h = img.size
    r = config.DET_SIZE / max(w, h)
    nw, nh = max(1, round(w * r)), max(1, round(h * r))
    img2 = img.resize((nw, nh), Image.BILINEAR)
    canvas = Image.new("RGB", (config.DET_SIZE, config.DET_SIZE), (114, 114, 114))
    pad_x = (config.DET_SIZE - nw) // 2
    pad_y = (config.DET_SIZE - nh) // 2
    canvas.paste(img2, (pad_x, pad_y))
    arr = np.asarray(canvas, dtype=np.float32) / 255.0
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0), r, pad_x, pad_y


def face_det_tensor(img: Image.Image) -> tuple[np.ndarray, float, int, int]:
    """SCRFD 预处理：letterbox 640 黑边填充，归一化 (x/128 - 127.5)。

    与 YOLO 不同：SCRFD 训练使用 input_mean=127.5, input_std=128，黑边 0。
    """
    w, h = img.size
    r = config.DET_SIZE / max(w, h)
    nw, nh = max(1, round(w * r)), max(1, round(h * r))
    img2 = img.resize((nw, nh), Image.BILINEAR)
    canvas = Image.new("RGB", (config.DET_SIZE, config.DET_SIZE), (0, 0, 0))
    pad_x = (config.DET_SIZE - nw) // 2
    pad_y = (config.DET_SIZE - nh) // 2
    canvas.paste(img2, (pad_x, pad_y))
    arr = np.asarray(canvas, dtype=np.float32)
    arr = (arr - 127.5) / 128.0
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0), r, pad_x, pad_y


def face_align(img: Image.Image, kps: np.ndarray, size: int = 112) -> np.ndarray:
    """根据 5 个关键点对人脸做相似变换对齐，返回模型输入 tensor。

    kps: (5,2) 原始图像坐标。使用 cv2.estimateAffinePartial2D（4 自由度
    相似变换：缩放+旋转+平移），不依赖 skimage（与 numpy 2.x 二进制不兼容）。
    """
    M, _ = cv2.estimateAffinePartial2D(
        np.asarray(kps, dtype=np.float32),
        ARC_DST.astype(np.float32),
        method=cv2.LMEDS,
    )
    if M is None:
        # 退化情况：直接取质心平移到模板中心
        c = kps.mean(axis=0)
        M = np.array(
            [[1.0, 0.0, ARC_DST[:, 0].mean() - c[0]],
             [0.0, 1.0, ARC_DST[:, 1].mean() - c[1]]],
            dtype=np.float32,
        )
    rgb = np.asarray(img, dtype=np.uint8)
    warped = cv2.warpAffine(rgb, M, (size, size), borderValue=0.0)
    warped = (warped - 127.5) / 127.5
    return np.expand_dims(warped.transpose(2, 0, 1).astype(np.float32), axis=0)


def scene_tensor(img: Image.Image) -> np.ndarray:
    """Places365 预处理：resize 短边 256 + 中心裁剪 224 + ImageNet 均值方差归一。"""
    w, h = img.size
    r = 256 / min(w, h)
    img = img.resize((max(1, round(w * r)), max(1, round(h * r))), Image.BILINEAR)
    w2, h2 = img.size
    l, t = (w2 - 224) // 2, (h2 - 224) // 2
    img = img.crop((l, t, l + 224, t + 224))
    arr = np.asarray(img, dtype=np.float32) / 255.0
    arr = (arr - np.array([0.485, 0.456, 0.406], dtype=np.float32)) / np.array(
        [0.229, 0.224, 0.225], dtype=np.float32
    )
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0)


def ocr_tensor(img: Image.Image) -> tuple[np.ndarray, float, int, int]:
    """PaddleOCR ch_PP-OCRv4 det 预处理：letterbox 640 灰底 114。

    与 det_tensor 一致（PP-OCRv4 det 训练用 DetResizeForTest(limit_side_len=640)
    + NormalizeImage(scale=1/255, mean=[0.485,0.456,0.406], std=[0.229,0.224,0.225])）。
    返回 (tensor, scale, pad_x, pad_y)。模型后补时如需对齐实测可微调。
    """
    w, h = img.size
    r = config.DET_SIZE / max(w, h)
    nw, nh = max(1, round(w * r)), max(1, round(h * r))
    img2 = img.resize((nw, nh), Image.BILINEAR)
    canvas = Image.new("RGB", (config.DET_SIZE, config.DET_SIZE), (114, 114, 114))
    pad_x = (config.DET_SIZE - nw) // 2
    pad_y = (config.DET_SIZE - nh) // 2
    canvas.paste(img2, (pad_x, pad_y))
    arr = np.asarray(canvas, dtype=np.float32) / 255.0
    arr = (arr - np.array([0.485, 0.456, 0.406], dtype=np.float32)) / np.array(
        [0.229, 0.224, 0.225], dtype=np.float32
    )
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0), r, pad_x, pad_y


def flower_tensor(img: Image.Image) -> np.ndarray:
    """花朵专家（efficientnet-b2 102 类）预处理：短边 256 + 中心裁剪 224
    + ImageNet 均值方差归一（实测优于拉伸，置信度更高）。"""
    w, h = img.size
    r = 256 / min(w, h)
    img = img.resize((max(1, round(w * r)), max(1, round(h * r))), Image.BILINEAR)
    w2, h2 = img.size
    l, t = (w2 - 224) // 2, (h2 - 224) // 2
    img = img.crop((l, t, l + 224, t + 224))
    arr = np.asarray(img, dtype=np.float32) / 255.0
    arr = (arr - np.array([0.485, 0.456, 0.406], dtype=np.float32)) / np.array(
        [0.229, 0.224, 0.225], dtype=np.float32
    )
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0)


def food_tensor(img: Image.Image) -> np.ndarray:
    """食物专家（resnet50 101 类）预处理：与 flower_tensor 相同的 ImageNet 协议。"""
    return flower_tensor(img)
