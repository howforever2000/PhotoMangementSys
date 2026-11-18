"""人脸液化形变（瘦脸 / 大眼）—— 纯 numpy + PIL，不依赖 cv2

## 为什么不用 cv2

`ai_studio` 刻意不带 cv2（见 `python/ai-studio.spec` 的 `excludes` 与
`knowledge/local-enhance.md` §7）。cv2 只提供两样我们要的东西：`warpAffine`
与 `remap`，两者都能用 numpy 反向双线性采样等价实现，且少了 60MB 依赖。

## 为什么不用 `PIL.Image.MESH`

`Image.MESH` 看着最省事，但它的 quad 角点顺序是 **NW, SW, SE, NE**（左上、左下、
右下、右上），不是直觉的顺时针。写错时不会报错，只是把整张脸切成方块马赛克
（`__transformer` 内部转成 `Transform.QUAD`，顺序错就是逐块错位）。恒等变换自检
`maxdiff` 必须为 0 —— 实测第一版写错时是 255。
本模块改用 numpy 反向双线性，无顺序陷阱，还能同时处理 x/y 位移（大眼需要）。

## 位移场为什么要「归一化 RBF + 固定锚点」

只把下颌线往里推、靠一个全局高斯扩散位移，会把**嘴一起横向拉宽**（实测 sweep：
sigma 大时嘴明显变形）。原因是位移场会无差别扩散到面部所有区域。

解法：除了「下颌点向内」这些**动点**，再声明一批**位移为 0 的锚点**——眼中心、
鼻尖、嘴中心、额头、下巴尖、两耳外侧。用归一化 RBF（每个控制点高斯权重除以权重
总和）插值，就得到「下颌收、五官纹丝不动」的位移场。归一化 RBF 在控制点上不复现
精确位移（会略弱），但对「自然、不出错」优先的懒人场景是更好的取舍。

## 强度边界（实测）

无锚点时 `slim` 到 0.35 就崩（嘴被拉宽）；带锚点后 0.4 / 0.6 / 0.8 三档均自然。
默认取 `slim=0.5`、`eyes=0.30`，上限 0.8 / 0.5 —— 与
`knowledge/local-enhance.md` §9 的「宁可欠一点，不要过火」同一条纪律。
"""
from __future__ import annotations

from typing import Dict, Tuple

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

from . import landmark

# 位移场粗网格步长：先在 1/COARSE 分辨率上算高斯权重再放大。
# 位移场本身是低频的，放大几乎无损；直接把 12MP 全图逐控制点算高斯会慢一个量级。
_COARSE = 4
# RBF 核宽度（相对脸宽）。太小 → 只能动到点附近、出现凹陷；太大 → 扩散到五官。
_SIGMA_RATIO = 0.20
# 只变形这种人脸以上的图：再小的脸变形也看不出来，白花时间
_MIN_PIXELS = 120
# 大眼衰减窗宽度（相对 eye radius）。旧值 0.4 使位移在 ~7px 内骤降，
# 局部应变 0.6~1.1，肉眼即「拉扯畸变」；加宽到 1.6 后位移平缓过渡。
_EYE_FALLOFF = 1.6
# 大眼强度系数（乘在 eyes 上）。旧值 0.5 在 eyes=0.3 时峰值位移 3.2px，
# 现在 0.35 配合加宽窗，观感更自然（BUG-2026-1117-004）。
_EYE_STRENGTH = 0.35
# 局部应变上限：|∇d| 超过它就会看到像素被拉伸。0.18 是实测「看不出畸变」的上限。
_MAX_STRAIN = 0.18


def _clamp01(value: float, default: float, cap: float) -> float:
    try:
        v = float(value)
    except Exception:
        return default
    return float(max(0.0, min(cap, v)))


def _jaw_strength(index: int) -> float:
    """下颌线上每个点的权重：下巴尖(8)最强，往耳侧递减。

    纯粹收窄会变成「锥子脸」，所以靠近耳朵的两端几乎不动 —— 那才是人脸的
    真实轮廓（耳前是咬肌，不是下颌角）。
    """
    return (1.0 - abs(index - 8) / 8.0) ** 1.2


def _controls(
    points: np.ndarray, metrics: Dict[str, float], slim: float
) -> Tuple[np.ndarray, np.ndarray]:
    """构造 (控制点, 位移) 两个数组 —— 只负责瘦脸；大眼走 `_eye_bump`。

    **位移语义（BUG-2026-1114-002）：这里的 offset 是「目标像素 → 源图采样位置」
    的偏移（`_remap` 是反向映射：`sx = xx + dx`）。要把下颌内容推向中心，
    边缘处的反向位移必须指向**外侧**，所以动点 offset 取 `(px - center_x)` 正号 ——
    旧版写反成 `(center_x - px)`，实测 slim 0.5 时下颌 span +3.2%（变胖）、
    眼睛 -4.9%（缩小），方向与意图完全相反；恒等自检（diff=0）抓不住符号错误。

    动点：下颌线 17 点向内收。
    锚点：眼中心 / 鼻尖 / 鼻梁 / 嘴中心 / 额头 / 下巴尖 / 两耳外侧 —— 位移 0。
    """
    controls: list[Tuple[float, float]] = []
    offsets: list[Tuple[float, float]] = []

    center_x = metrics["center_x"]
    top_y = metrics["top_y"]
    chin_y = float(points[8][1])
    span = max(chin_y - top_y, 1e-6)

    if slim > 0:
        for index in landmark.JAW:
            px, py = float(points[index][0]), float(points[index][1])
            vertical = float(np.clip((py - top_y) / span, 0.0, 1.0)) ** 0.8
            weight = _jaw_strength(index) * vertical
            controls.append((px, py))
            # 反向映射：目标处往外采样 = 内容往内收。见函数头注释。
            offsets.append(((px - center_x) * slim * weight, 0.0))

    # 固定锚点：五官与轮廓上的「不能动」位置。
    # 少了它们，位移场会渗进嘴与鼻子（实测的翻车点）。
    anchors = [
        metrics["eye_left"],
        metrics["eye_right"],
        points[30],                      # 鼻尖
        points[33],                      # 鼻底
        points[27],                      # 鼻梁上端
        points[19],                      # 左眉
        points[24],                      # 右眉
        (points[0] + points[16]) / 2.0,  # 两耳中点
        points[8],                       # 下巴尖（自身不横向位移）
        points[51],                      # 上唇中点
        points[57],                      # 下唇中点
    ]
    for anchor in anchors:
        controls.append((float(anchor[0]), float(anchor[1])))
        offsets.append((0.0, 0.0))

    return np.asarray(controls, dtype=np.float32), np.asarray(offsets, dtype=np.float32)


def _eye_bump(
    metrics: Dict[str, float], eyes: float, origin: np.ndarray, width: int, height: int
) -> Tuple[np.ndarray, np.ndarray]:
    """大眼位移场：以眼中心为圆心的**加性径向凸起**（不走归一化 RBF）。

    为什么不用控制点环：眼中心锚点（位移 0）与鼻梁/耳中点等 0 锚点都在一个
    sigma（0.2 脸宽）内，归一化 RBF 会把环点的意图位移稀释到 ~12%
    （BUG-2026-1114-002 的稀释分支：合成图 +15% 意图实测只有 +0.8%）。
    加性凸起没有稀释：中心位移恰为 0（无需锚点）、越靠边缘越弱、
    在 1.4×radius 外自然衰减到 0，与下颌场直接相加即可。

    反向映射语义：目标处向眼中心采样 = 内容向外放大（眼睛变大）。
    """
    dx = np.zeros((height, width), dtype=np.float32)
    dy = np.zeros((height, width), dtype=np.float32)
    if eyes <= 0:
        return dx, dy

    radius = float(metrics["face_width"]) * 0.11
    gy, gx = np.mgrid[0:height, 0:width].astype(np.float32)
    gx += origin[0]
    gy += origin[1]
    strength = eyes * _EYE_STRENGTH
    for eye in (metrics["eye_left"], metrics["eye_right"]):
        ex, ey = float(eye[0]), float(eye[1])
        vx = gx - ex
        vy = (gy - ey) / 0.7                      # 椭圆纵向压扁（与旧环一致）
        dist = np.sqrt(vx * vx + vy * vy)
        # 衰减窗：radius 内全强度，到 2×radius 才衰减到 0。
        # 旧窗只有 0.4×radius（约 7px），位移在极短距离内骤降，
        # 局部应变 0.6~1.1 → 用户感知为「脸被拉扯变形」（BUG-2026-1117-004）。
        t = np.clip(1.0 - (dist - radius) / (_EYE_FALLOFF * radius), 0.0, 1.0)
        bump = t * t * (3.0 - 2.0 * t)
        # 单位向量指向眼中心（= 反向映射下的放大方向），幅值 ∝ 距离（仿射缩放）
        safe = np.maximum(dist, 1e-3)
        scale = bump * strength * np.clip(dist / radius, 0.0, 1.0)
        dx += (-vx / safe) * radius * scale
        dy += (-vy / safe) * radius * scale * 0.7
    return dx, dy


def _displacement(
    controls: np.ndarray,
    offsets: np.ndarray,
    origin: np.ndarray,
    width: int,
    height: int,
    sigma: float,
) -> Tuple[np.ndarray, np.ndarray]:
    """在粗网格上算归一化 RBF 位移场，再双线性放大到 ROI 尺寸。"""
    grid_w = width // _COARSE + 1
    grid_h = height // _COARSE + 1
    gy, gx = np.mgrid[0:grid_h, 0:grid_w].astype(np.float32)
    gx *= _COARSE
    gy *= _COARSE

    weight_sum = np.zeros_like(gx)
    dx = np.zeros_like(gx)
    dy = np.zeros_like(gy)
    denominator = 2.0 * sigma * sigma
    for (cx, cy), (ux, uy) in zip(controls - origin, offsets):
        kernel = np.exp(-((gx - cx) ** 2 + (gy - cy) ** 2) / denominator)
        weight_sum += kernel
        dx += ux * kernel
        dy += uy * kernel

    # 归一化：远处权重总和趋 0，此时分子也趋 0，除以 eps 得 0（不会爆）
    weight_sum = np.maximum(weight_sum, 1e-6)
    dx /= weight_sum
    dy /= weight_sum

    def up(field: np.ndarray) -> np.ndarray:
        return np.asarray(
            Image.fromarray(field).resize((width, height), Image.BILINEAR), dtype=np.float32
        )

    return up(dx), up(dy)


def _remap(roi: np.ndarray, dx: np.ndarray, dy: np.ndarray) -> np.ndarray:
    """按位移场做反向双线性采样（目标像素去源图取色）。"""
    height, width = roi.shape[:2]
    yy, xx = np.mgrid[0:height, 0:width].astype(np.float32)
    sx = np.clip(xx + dx, 0.0, width - 1.001)
    sy = np.clip(yy + dy, 0.0, height - 1.001)
    ix = np.floor(sx).astype(np.int32)
    iy = np.floor(sy).astype(np.int32)
    fx = (sx - ix)[..., None]
    fy = (sy - iy)[..., None]
    out = (
        roi[iy, ix] * (1 - fx) * (1 - fy)
        + roi[iy, ix + 1] * fx * (1 - fy)
        + roi[iy + 1, ix] * (1 - fx) * fy
        + roi[iy + 1, ix + 1] * fx * fy
    )
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


def _limit_strain(
    dx: np.ndarray, dy: np.ndarray, max_strain: float = _MAX_STRAIN, iters: int = 6
) -> Tuple[np.ndarray, np.ndarray]:
    """把位移场的局部梯度夹到 `max_strain` 以下，杜绝「像素被拉扯」的畸变观感。

    为什么用扩散式混合而不是乘性缩放：
        乘性缩放（`dx *= s`）在限制器自身的边界处会引入 |dx|×|∇s| 的新梯度，
        实测反而把应变从 0.53 推到 1.97 并稳定在那里（越限制越糟）。
        向「模糊版位移场」混合不产生新梯度，迭代单调收敛。
    """
    dx, dy = dx.copy(), dy.copy()
    for _ in range(iters):
        g = np.maximum.reduce(
            [
                np.abs(np.diff(dx, axis=1, prepend=dx[:, :1])),
                np.abs(np.diff(dx, axis=0, prepend=dx[:1, :])),
                np.abs(np.diff(dy, axis=1, prepend=dy[:, :1])),
                np.abs(np.diff(dy, axis=0, prepend=dy[:1, :])),
            ]
        )
        # 超标程度 → 混合权重；权重本身再模糊，避免在边界处造出新的梯度突变
        w = np.clip((g - max_strain) / max(max_strain, 1e-6), 0.0, 1.0).astype(np.float32)
        w = np.asarray(
            Image.fromarray((w * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(3)),
            dtype=np.float32,
        ) / 255.0
        dx = dx * (1.0 - w) + _box_blur(dx, 4) * w
        dy = dy * (1.0 - w) + _box_blur(dy, 4) * w
    return dx, dy


def _box_blur(a: np.ndarray, r: int) -> np.ndarray:
    """积分图式盒式模糊（比 GaussianBlur 快，且对位移场足够平滑）。"""
    k = 2 * r + 1
    pad = np.pad(a, r, mode="edge")
    c = np.cumsum(pad, axis=0)
    c = np.vstack([np.zeros((1, c.shape[1]), dtype=c.dtype), c])
    a2 = (c[k:] - c[:-k]) / k
    c = np.cumsum(a2, axis=1)
    c = np.hstack([np.zeros((c.shape[0], 1), dtype=c.dtype), c])
    return (c[:, k:] - c[:, :-k]) / k


def _roi_box(points: np.ndarray, pad: int, width: int, height: int) -> Tuple[int, int, int, int]:
    x1 = max(0, int(points[:, 0].min()) - pad)
    y1 = max(0, int(points[:, 1].min()) - pad)
    x2 = min(width, int(points[:, 0].max()) + pad)
    y2 = min(height, int(points[:, 1].max()) + pad)
    return x1, y1, x2, y2


def _border_falloff(dx: np.ndarray, dy: np.ndarray, inset: int) -> Tuple[np.ndarray, np.ndarray]:
    """让位移在 ROI 边界处衰减到 0，保证贴回时没有接缝。

    为什么必需：位移场是归一化 RBF 的产物，它在 ROI 边缘**不为零**（实测左右边界
    仍有 14~16px 位移）。而贴回只靠羽化 alpha 混合 —— alpha 在边界处已经衰减，
    但被混合的两侧像素本身有位移差，结果就是一圈可见的重影/错位。
    把位移压到 0 后，「ROI 内变形、ROI 外不动」在边界处严格连续。

    inset 取 ROI 短边的 1/8 并夹到 [16, 96]：太窄会切掉下颌位移，太宽等于没变形。
    """
    height, width = dx.shape
    edge = int(max(16, min(96, min(width, height) // 8))) if inset <= 0 else inset
    # 到四边的最小归一化距离（0 = 贴边，1 = 已进入安全区）
    gx = np.minimum(np.arange(width, dtype=np.float32), np.arange(width, 0, -1, dtype=np.float32))
    gy = np.minimum(np.arange(height, dtype=np.float32), np.arange(height, 0, -1, dtype=np.float32))
    ramp_x = np.clip(gx / edge, 0.0, 1.0)
    ramp_y = np.clip(gy / edge, 0.0, 1.0)
    # smoothstep：端点一阶导为 0，避免衰减带里出现折线
    ramp_x = ramp_x * ramp_x * (3.0 - 2.0 * ramp_x)
    ramp_y = ramp_y * ramp_y * (3.0 - 2.0 * ramp_y)
    falloff = ramp_y[:, None] * ramp_x[None, :]
    return dx * falloff, dy * falloff


def reshape(
    img: Image.Image,
    points: np.ndarray,
    slim: float = 0.5,
    eyes: float = 0.30,
) -> Image.Image:
    """按 68 点做瘦脸 + 大眼，返回新图（原图不改动）。

    参数都夹在安全区间内（slim ≤ 0.8、eyes ≤ 0.5）；传 0 表示不做该项。
    """
    slim = _clamp01(slim, 0.30, 0.8)
    eyes = _clamp01(eyes, 0.15, 0.5)
    if slim <= 0 and eyes <= 0:
        return img

    metrics = landmark.face_metrics(points)
    face_width = float(metrics["face_width"])
    if face_width < _MIN_PIXELS:
        return img

    pad = int(face_width * 0.30)
    x1, y1, x2, y2 = _roi_box(points, pad, img.width, img.height)
    width, height = x2 - x1, y2 - y1
    if width < 8 or height < 8:
        return img

    controls, offsets = _controls(points, metrics, slim)
    sigma = max(face_width * _SIGMA_RATIO, 6.0)
    dx, dy = _displacement(controls, offsets, np.array([x1, y1], dtype=np.float32), width, height, sigma)
    eye_dx, eye_dy = _eye_bump(metrics, eyes, np.array([x1, y1], dtype=np.float32), width, height)
    dx, dy = dx + eye_dx, dy + eye_dy
    dx, dy = _border_falloff(dx, dy, 0)
    # 应变限制必须在 border_falloff 之后：衰减本身会抬高梯度（实测 0.22 → 0.53）。
    dx, dy = _limit_strain(dx, dy)

    roi = np.asarray(img.crop((x1, y1, x2, y2)), dtype=np.float32)
    warped = Image.fromarray(_remap(roi, dx, dy))

    # 椭圆羽化遮罩：矩形贴回会在 ROI 四角留直边，椭圆更贴合脸的形状
    mask = Image.new("L", (width, height), 0)
    inset = pad * 0.45
    ImageDraw.Draw(mask).ellipse(
        [inset, inset, width - inset, height - inset], fill=255
    )
    mask = mask.filter(ImageFilter.GaussianBlur(max(1.0, pad * 0.45)))

    out = img.copy()
    out.paste(warped, (x1, y1), mask)
    return out


__all__ = ["reshape"]
