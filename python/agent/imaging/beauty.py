"""人像润色（本地，零成本）。

## 为什么按「肤色掩膜」而不是先做人脸检测

docs/v5.md 的目标形态是"检测人脸 → 只对人脸区域处理"。但人脸检测器在 VCR 通道里，
而 VCR 要预热（READY 最长 90s），把它绑进"点一下就出活"的零成本路径上是自相矛盾。
所以本版本用 **YCbCr 肤色掩膜**：不依赖任何模型、毫秒级、结果确定。

代价要讲清楚：掩膜会同时覆盖手部/颈部等肤色区域，且在偏黄/偏红的灯光下容易漏。
这属于"够用的一版"，等 Rust 侧把 VCR 的人脸框随 submit 一起传进来
（`faces: [{x,y,w,h}]`）后，换成"只在框内用肤色掩膜"即可，接口不变。
"""
from __future__ import annotations

from dataclasses import dataclass

from PIL import Image, ImageFilter

from .imageio import open_rgb, save_jpeg

# 强度档 → (混合比例, 模糊半径)。半径按图片长边比例算，保证大小图观感一致。
STRENGTH: dict[str, tuple[float, float]] = {
    "mild": (0.35, 0.0022),
    "natural": (0.55, 0.0030),
    "strong": (0.85, 0.0042),
}

# YCbCr 肤色区间（经典经验值，覆盖东亚/高加索肤色）
CB_RANGE = (77, 127)
CR_RANGE = (133, 173)
Y_MIN = 60


@dataclass
class BeautyResult:
    src: str
    out: str
    strength: str
    blend: float
    width: int
    height: int

    def to_dict(self) -> dict:
        return {
            "src": self.src,
            "out": self.out,
            "strength": self.strength,
            "blend": round(self.blend, 3),
            "w": self.width,
            "h": self.height,
        }


def _range_lut(lo: int, hi: int) -> list[int]:
    """区间内的值 → 255，区间外线性衰减到 0（避免硬边）。"""
    return [0 if i < lo or i > hi else 255 for i in range(256)]


def skin_mask(img: Image.Image) -> Image.Image:
    """肤色掩膜（L 模式，255=肤色）。"""
    ycbcr = img.convert("YCbCr")
    y, cb, cr = ycbcr.split()
    m_cb = cb.point(_range_lut(*CB_RANGE))
    m_cr = cr.point(_range_lut(*CR_RANGE))
    m_y = y.point(_range_lut(Y_MIN, 255))
    from PIL import ImageChops

    mask = ImageChops.multiply(ImageChops.multiply(m_cb, m_cr), m_y)
    # 羽化：硬掩膜直接混合会留下明显的"面具边"
    return mask.filter(ImageFilter.GaussianBlur(radius=max(2, min(img.size) // 200)))


def smooth(src: str, dst: str, strength: str = "natural", quality: int = 92) -> BeautyResult:
    """磨皮 + 轻微提亮（只在掩膜内生效）。"""
    blend, radius_ratio = STRENGTH.get(strength, STRENGTH["natural"])
    img = open_rgb(src)
    radius = max(1.5, max(img.size) * radius_ratio)

    blurred = img.filter(ImageFilter.GaussianBlur(radius=radius))
    # 轻微提亮/压饱和，避免磨皮后发灰
    bright = blurred.point(lambda v: min(255, int(v * 1.03 + 2)))

    mask = skin_mask(img).point(lambda v: int(v * blend))
    out = Image.composite(bright, img, mask)

    save_jpeg(out, dst, quality=quality)
    return BeautyResult(src, dst, strength, blend, out.width, out.height)


__all__ = ["smooth", "skin_mask", "BeautyResult", "STRENGTH"]
