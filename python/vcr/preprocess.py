"""图像预处理：解码 / 一次缩放 / 检测 / 人脸 / OCR / 语义向量

统一入口接收 PIL.Image（各服务只解码一次图片）；点 3 起进一步用 letterbox()
**只缩放一次**，det/ocr/face 三通道与影调都从同一份缩放结果派生。
v5：分类模型与场景/专家通道已下线，cls_tensor / scene_tensor / flower_tensor /
food_tensor 一并移除；clip_tensor 的尺寸与归一化改为读「当前语义模型档位」。
"""
from typing import NamedTuple

import cv2
import numpy as np
from PIL import Image, ImageOps

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


def open_image_ex(path: str) -> tuple[Image.Image | None, tuple[int, int] | None]:
    """解码 + 按 EXIF Orientation 转正，同时返回**真实原图像素尺寸**。

    为什么必须同时回尺寸（P-DEC1）：解码走 draft() 降采样后（见 config.
    DECODE_MIN_LONG），返回的图比真实原图小，而下游有两个口径必须保持真值：
      · bbox / kps 要落在**真实原图坐标系**（Rust 侧按它去裁头像、生成镶略图）
      · FACE_MIN_PIX / AREA 阈值要按**真实原图**像素算
    所以尺寸不能从返回图的 .size 推，必须由这里带出去
    （转正会交换宽高，所以尺寸在 exif_transpose **之后**取）。
    """
    try:
        with Image.open(path) as img:
            orig_before = img.size
            # draft 必须在真正加载前调（exif_transpose / convert 会触发加载）。
            # 只对 JPEG 生效；PNG/HEIC 等是空操作，此时下面算出的 dec_scale = 1。
            floor = config.DECODE_MIN_LONG
            if floor > 0:
                w, h = orig_before
                long_side = max(w, h)
                denom = 1
                while denom < 8 and long_side / (denom * 2) >= floor:
                    denom *= 2
                if denom > 1:
                    img.draft("RGB", (max(1, w // denom), max(1, h // denom)))
            out = ImageOps.exif_transpose(img).convert("RGB")
            # 转正会交换宽高（竖拍相机片）。解码图的宽高比是否与转正前“同向”
            # 能无歧义地判断有没有被交换 —— 不能用 out.size 当原图尺寸（那是降采样后的）。
            ow, oh = orig_before
            if (out.size[0] > out.size[1]) != (ow > oh):
                ow, oh = oh, ow
            return out, (ow, oh)
    except Exception:
        return None, None


def open_image(path: str) -> Image.Image | None:
    """解码并**按 EXIF Orientation 转正**（不关心真实原图尺寸时的入口）。

    要参与坐标换算的调用方（pipeline.classify_one）必须用 open_image_ex，
    否则会把「降采样后的尺寸」当成原图尺寸，导致 bbox 与尺度阈值全部错位。

    # 为什么必须显式转正（BUG-2026-1014-002）

    相机传感器横向固定，竖持拍摄时像素是躺倒的，文件里靠 EXIF Orientation
    标签声明「看图时请顺时针转 90°」。**JPEG 自己不会转** —— Orientation 只是给
    读取方的一条指令，而各读取方是否执行并不统一：

        看图软件 / Chrome 81+   ✅ 自动转正
        Pillow（默认）          ❌ 不转，需显式 ImageOps.exif_transpose()
        Rust image crate        ❌ 不转

    因此横拍照片会以「侧脸」进入检测模型：ArcFace/SCRFD 的训练集全是正脸，
    它判据是眼-鼻-嘴的上下空间关系，转 90° 后该关系彻底颠倒 → 置信度极低 →
    被阈值过滤 → 明明有人却报「未检测到人脸」。同一入口的分类/OCR/影调/语义向量
    同样受损（它们都从本函数取图）。

    转正后，bbox 处于**显示坐标系**（与浏览器看原图的朝向一致）；Rust 侧裁头像
    与生成缩略图也必须转正到同一坐标系，否则会按「转正后的坐标」去裁「躺倒的
    图」，裁出背景 —— 见 src-tauri/src/exif_orient.rs。

    注意 exif_transpose 会**删除** Orientation 标签（避免下游二次转正），并交换
    宽高；无 EXIF 或 Orientation=1 时是零成本的恒等操作。
    """
    img, _ = open_image_ex(path)
    return img


# ---------------------------------------------------------------------------
# 点 3：一次解码 + 一次缩放，供 det / ocr / face 三通道与影调共用
#
# 为什么要专门抽出来（实测，勿改回「各通道自己 resize」）：
#   det_tensor / ocr_tensor / face_det_tensor 的缩放参数完全相同
#   （最长边 DET_SIZE 等比、BILINEAR），但旧实现每个通道都从**原图**重新
#   resize 一次。一张 4096x5568 的照片：4096→640 的 BILINEAR 实测 37.7ms、
#   4096→256（影调）实测 35.6ms —— 同一次解码上排了 4 遍缩放，其中 3 遍纯重复。
#
# 等价性保证（点 2 的教训：这类改动必须能逐位比对）：
#   - base 与旧实现逐位相同（同样的源图 / 同样的目标尺寸 / 同样的滤波器）；
#   - 几何换算继续用从**原图**算出的 r，而不是从 base 反推，故 bbox / kps
#     与旧实现完全一致；
#   - 越界过滤与面积比用原图尺寸 `lb.size`，与旧实现同口径。
# ---------------------------------------------------------------------------
class Letterbox(NamedTuple):
    """一次解码 + 一次缩放的产物；三通道与影调都从它派生。

    P-DEC1：解码可能已走 draft() 降采样，所以三个坐标口径要分清 ——
      · base     ：解码图再缩到最长边 DET_SIZE（模型吃这个，坐标在 base 空间）
      · r        ：base → **真实原图** 的等比缩放比（box/kps/area 一律换算到原图）
      · size     ：**真实原图**像素尺寸（越界过滤 / 面积比 / FACE_MIN_PIX 的真值口径）
      · dec_scale：解码图长边 / 真实原图长边（≤1）。只有 face_align 需要它 ——
                  它直接在**解码图**上裁切，所以要把「原图坐标的关键点」
                  乘回 dec_scale 变成解码图坐标。
    旧实现里三者相等只是因为解码 == 原图；拆开后语义不变，逐位等价性只在 base 上。
    """

    base: Image.Image            # 最长边 = DET_SIZE 的等比缩放图（共用同一份）
    r: float                     # 真实原图 → base 的等比缩放比（几何换算永远用它）
    size: tuple[int, int]        # 真实原图像素尺寸（越界过滤 / 面积比口径）
    dec_scale: float = 1.0       # 解码图长边 / 真实原图长边（draft 降采样后 <1）


def letterbox(img: Image.Image, orig_size: tuple[int, int] | None = None) -> Letterbox:
    """把已解码的图缩放一次，供 det/ocr/face 三通道与影调共用。

    orig_size 必须是**真实原图**尺寸（open_image_ex 的第二个返回值）。
    不传 = 视 img 为全分辨率解码（旧行为，供独立入口 / bench 使用）。
    """
    w, h = img.size
    ow, oh = orig_size if orig_size else (w, h)
    # base 从**解码图**缩（这是真正要算的那一步），r 按**原图**算（换算口径）
    r = config.DET_SIZE / max(ow, oh)
    bs = config.DET_SIZE / max(w, h)
    base = img.resize((max(1, round(w * bs)), max(1, round(h * bs))), Image.BILINEAR)
    return Letterbox(base=base, r=r, size=(ow, oh), dec_scale=max(w, h) / max(ow, oh))


def det_tensor(lb: Letterbox) -> tuple[np.ndarray, float, int, int]:
    """检测预处理：letterbox 640 灰底填充。返回 (tensor, scale, pad_x, pad_y)。

    scale = 原图→letterbox 的缩放；pad 为左/上偏移。缩放只由 letterbox() 做一次。
    """
    img2 = lb.base
    pad_x = (config.DET_SIZE - img2.size[0]) // 2
    pad_y = (config.DET_SIZE - img2.size[1]) // 2
    canvas = Image.new("RGB", (config.DET_SIZE, config.DET_SIZE), (114, 114, 114))
    canvas.paste(img2, (pad_x, pad_y))
    arr = np.asarray(canvas, dtype=np.float32) / 255.0
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0), lb.r, pad_x, pad_y


def face_det_tensor(lb: Letterbox) -> tuple[np.ndarray, float, int, int]:
    """SCRFD 预处理：letterbox 640 黑边填充，归一化 (x-127.5)/128。

    与 YOLO 不同：SCRFD 训练使用 input_mean=127.5, input_std=128，黑边 0。
    """
    img2 = lb.base
    pad_x = (config.DET_SIZE - img2.size[0]) // 2
    pad_y = (config.DET_SIZE - img2.size[1]) // 2
    canvas = Image.new("RGB", (config.DET_SIZE, config.DET_SIZE), (0, 0, 0))
    canvas.paste(img2, (pad_x, pad_y))
    arr = np.asarray(canvas, dtype=np.float32)
    arr = (arr - 127.5) / 128.0
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0), lb.r, pad_x, pad_y


def face_align(img: Image.Image, kps: np.ndarray, size: int = 112) -> np.ndarray:
    """根据 5 个关键点对人脸做相似变换对齐，返回模型输入 tensor。

    kps: (5,2) 原始图像坐标。使用 cv2.estimateAffinePartial2D（4 自由度
    相似变换：缩放+旋转+平移），不依赖 skimage（与 numpy 2.x 二进制不兼容）。

    采样区裁剪（点 3）：旧实现先把**整张原图** np.asarray 成 20MP 的 RGB 数组
    （4096x5568 实测 25.24 ms/次）再 warp，但目的图只有 112x112，实际只会采样
    脸周那一小块；同一张图有几张脸就要整图转换几遍。现在先算出「112x112 输出
    画布经 M⁻¹ 映射回源图的外接框」，裁出那块再 warp：25.24 → 1.54 ms/次（16.3x），
    脸越多省得越多（实测 48 张相册 88 张脸，省 ~88ms/张）。

    两个容易写错的点（改之前先跑一遍等价性）：
      - 裁剪范围必须用**整张 112x112 画布**的逆映射，不能用 ARC_DST 的外接框 ——
        画布上 ARC_DST 之外的像素同样会从源图取样，少裁了会变成黑边；
      - 原图→裁剪图的坐标系偏移要经线性部分 A 才能落到平移项上
        （M2 = [A | t + A·origin]），写成 t - origin 会整块采错位置。

    等价性：像素级 0.06% 的通道值会因 cv2 内部 float32 求逆的 ULP 舍入换一个
    插值邻居（实测单图最大 217）。下游已实测：88 张脸的 ArcFace 嵌入余弦
    min 0.99961 / mean 0.99999，距 FACE_SIM（现为 0.55）仍有充足余量。
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
    w, h = img.size
    # estimateAffinePartial2D(kps → ARC_DST) 返回的是 src→dst 的 M；
    # warpAffine 默认会先求逆再采样（dst(x,y) = src(M⁻¹·(x,y))）。
    Minv = cv2.invertAffineTransform(M)
    # 采样点带半像素偏移（INTER_LINEAR 取 dst 像素中心），再各留 2px 插值余量
    corners = np.array([[0.5, 0.5], [size - 0.5, 0.5],
                        [size - 0.5, size - 0.5], [0.5, size - 0.5]], dtype=np.float32)
    pts = cv2.transform(corners.reshape(-1, 1, 2), Minv).reshape(-1, 2)
    x0 = max(0, int(np.floor(pts[:, 0].min())) - 2)
    y0 = max(0, int(np.floor(pts[:, 1].min())) - 2)
    x1 = min(w, int(np.ceil(pts[:, 0].max())) + 3)
    y1 = min(h, int(np.ceil(pts[:, 1].max())) + 3)

    if x1 > x0 and y1 > y0:
        rgb = np.asarray(img.crop((x0, y0, x1, y1)), dtype=np.uint8)
        # sub = full.crop(origin) ⇒ 需要 warpAffine(sub, M2) ≡ warpAffine(full, M)
        # ⇒ M2⁻¹ = shift(-origin) ∘ M⁻¹ ⇒ M2 = [A | t + A·origin]
        M2 = M.copy()
        M2[0, 2] += M[0, 0] * x0 + M[0, 1] * y0
        M2[1, 2] += M[1, 0] * x0 + M[1, 1] * y0
        warped = cv2.warpAffine(rgb, M2, (size, size), borderValue=0.0)
    else:
        # 极端退化（关键点全在图外）：回退整图，保持与旧实现同一结果
        warped = cv2.warpAffine(np.asarray(img, dtype=np.uint8), M, (size, size),
                                borderValue=0.0)
    warped = (warped - 127.5) / 127.5
    return np.expand_dims(warped.transpose(2, 0, 1).astype(np.float32), axis=0)


def ocr_tensor(lb: Letterbox) -> tuple[np.ndarray, float, int, int]:
    """PaddleOCR ch_PP-OCRv4 det 预处理：letterbox 640 灰底 114。

    与 det_tensor 一致（PP-OCRv4 det 训练用 DetResizeForTest(limit_side_len=640)
    + NormalizeImage(scale=1/255, mean=[0.485,0.456,0.406], std=[0.229,0.224,0.225])）。
    返回 (tensor, scale, pad_x, pad_y)。
    """
    img2 = lb.base
    pad_x = (config.DET_SIZE - img2.size[0]) // 2
    pad_y = (config.DET_SIZE - img2.size[1]) // 2
    canvas = Image.new("RGB", (config.DET_SIZE, config.DET_SIZE), (114, 114, 114))
    canvas.paste(img2, (pad_x, pad_y))
    arr = np.asarray(canvas, dtype=np.float32) / 255.0
    arr = (arr - np.array([0.485, 0.456, 0.406], dtype=np.float32)) / np.array(
        [0.229, 0.224, 0.225], dtype=np.float32
    )
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0), lb.r, pad_x, pad_y


def clip_tensor(img: Image.Image) -> np.ndarray:
    """Chinese-CLIP 预处理：短边 resize 到档位尺寸 + 中心裁剪 + CLIP mean/std 归一，CHW。

    尺寸随「当前语义模型档位」变化（B/16 = 224，L/14-336 = 336）。
    """
    p = config.clip_paths()
    size = int(p["size"])
    w, h = img.size
    r = size / min(w, h)
    img = img.resize((max(1, round(w * r)), max(1, round(h * r))), Image.BICUBIC)
    w2, h2 = img.size
    l, t = (w2 - size) // 2, (h2 - size) // 2
    img = img.crop((l, t, l + size, t + size))
    arr = np.asarray(img, dtype=np.float32) / 255.0
    arr = (arr - np.array(p["mean"], dtype=np.float32)) / np.array(p["std"], dtype=np.float32)
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0)  # (1,3,S,S)
