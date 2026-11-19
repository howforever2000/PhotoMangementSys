"""人脸解析（BiSeNet 19 类语义分割）：给一张对齐/裁切好的人脸，返回各类别掩码。

为什么单独一个模块：
    解析只做「这是皮肤 / 这是嘴唇 / 这是眉毛」的判断，不碰任何像素加工；
    磨皮、妆容等效果各自实现。这样两者可以独立测试，也可以把分区掩码导出复用。

模型：BiSeNet ResNet-34（MIT，yakhyo/face-parsing 发布的 ONNX）
    输入  input   float32  [1,3,512,512]，RGB、ImageNet 归一化
    输出  output  float32  [1,19,512,512]  logits（通道 argmax 即类别图）

注意「紧裁人脸」的坑：与本项目 SCRFD 的既有经验一致（`.dsh/knowledge/local-enhance.md` §2），
贴边人脸偏离训练分布，解析结果会碎。调用方应先按 bbox 外扩一定边距再送进来。
"""
from __future__ import annotations

import numpy as np
from PIL import Image

from . import config, gpu_lock, localmodels
from .logutil import call_model

# 19 类标签（顺序即模型输出通道，不可改）
LABELS = (
    "background", "skin", "l_brow", "r_brow", "l_eye", "r_eye", "eye_g",
    "l_ear", "r_ear", "ear_r", "nose", "mouth", "u_lip", "l_lip",
    "neck", "neck_l", "cloth", "hair", "hat",
)

IDX = {name: i for i, name in enumerate(LABELS)}

# 磨皮作用区：皮肤 + 颈 + 耳（`ear_r` 是耳饰，不属于皮肤）。
# 不用「只剩 skin」是因为耳朵与脖子的噪点同样显眼，而不做它们会留下明显的边界。
SKIN_CLASSES = (IDX["skin"], IDX["neck"], IDX["l_ear"], IDX["r_ear"], IDX["neck_l"])

# 五官保护：这些区域即使被误判成皮肤也不能磨，否则眼神、唇纹会被抹平。
PROTECT_CLASSES = (
    IDX["l_eye"], IDX["r_eye"], IDX["eye_g"], IDX["l_brow"], IDX["r_brow"],
    IDX["u_lip"], IDX["l_lip"], IDX["mouth"],
)

_INPUT_SIZE = 512
_MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
_STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)

MODEL_FILE = "face_parsing_bisenet_r34.onnx"


def ready() -> bool:
    """解析模型是否就位。"""
    return localmodels.model_ready(MODEL_FILE)


def predict(face_rgb: Image.Image) -> np.ndarray:
    """对一张人脸图做解析，返回 [H,W] 的 uint8 类别图（原图尺寸）。"""
    sess = localmodels.get_session(config.local_model_path(MODEL_FILE))
    src = face_rgb.convert("RGB")
    w, h = src.size
    small = src.resize((_INPUT_SIZE, _INPUT_SIZE), Image.LANCZOS)
    x = np.asarray(small, dtype=np.float32) / 255.0
    x = (x - _MEAN) / _STD
    x = x.transpose(2, 0, 1)[None]                     # HWC → 1CHW

    iname = sess.get_inputs()[0].name
    with call_model("onnx", "face_parsing_bisenet_r34.onnx", f"{w}x{h} face"):
        out = gpu_lock.run(sess, {iname: x})[0]                # [1,19,512,512]
    cls = np.argmax(out[0], axis=0).astype(np.uint8)   # [512,512]

    # 回原尺寸用最近邻：类别图不能插值，否则会造出不存在的类别编号
    back = Image.fromarray(cls, mode="L").resize((w, h), Image.NEAREST)
    return np.asarray(back, dtype=np.uint8)


def skin_mask(cls_map: np.ndarray) -> np.ndarray:
    """由类别图得到 [H,W] 的 float32 皮肤权重（0~1），已排除五官与衣物/头发。"""
    mask = np.isin(cls_map, SKIN_CLASSES)
    protect = np.isin(cls_map, PROTECT_CLASSES)
    mask = np.logical_and(mask, ~protect)
    return mask.astype(np.float32)
