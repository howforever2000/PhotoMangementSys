"""本地 LUT 风格滤镜（P1a / P0 的零成本路径）。

实现方式刻意简单：**逐通道 LUT + HSV 饱和**，不引 numpy。
好处是快、确定、无依赖；代价是做不到"局部色调分离"这类高级调色 ——
那类需求属于 remote 引擎（AI 重绘）的范围。

参数来自 `style_presets.py`（唯一真相），所以高级区显示的数字就是这里执行用的数字。
"""
from __future__ import annotations

from dataclasses import dataclass

from PIL import Image

from .imageio import open_rgb, save_jpeg
from .style_presets import preset_params


@dataclass
class StyleResult:
    src: str
    out: str
    preset: str
    level: str
    params: dict
    width: int
    height: int

    def to_dict(self) -> dict:
        return {
            "src": self.src,
            "out": self.out,
            "preset": self.preset,
            "level": self.level,
            "params": self.params,
            "w": self.width,
            "h": self.height,
        }


def _clamp255(v: float) -> int:
    return 0 if v < 0 else (255 if v > 255 else int(round(v)))


def build_channel_lut(gain: float, highlight: float, shadow: float) -> list[int]:
    """单通道 LUT：增益(色温) → 阴影压/提 → 高光压/提。

    三段各自作用在不同亮度区间，所以叠加顺序对结果影响很小（可复现）。
    h = highlight/100（正=提亮高光），s = shadow/100（负=压暗阴影）。
    """
    h = highlight / 100.0
    s = shadow / 100.0
    out: list[int] = []
    for i in range(256):
        x = i / 255.0
        x = x * gain                                   # 色温：整体增益
        x = x + s * (1.0 - x) * (1.0 - x) # 阴影段（近黑处影响最大）
        x = x + h * (x ** 3)              # 高光段（近白处影响最大）
        out.append(_clamp255(x * 255.0))
    return out


def build_saturation_lut(saturation: int) -> list[int]:
    k = 1.0 + saturation / 100.0
    return [_clamp255(i * k) for i in range(256)]


def apply_style(src: str, dst: str, preset: str, level: str, quality: int = 90) -> StyleResult:
    """对单张图应用本地风格。同输入同参数 → 同输出。"""
    p = preset_params(preset, level)
    img = open_rgb(src)

    gain_r = 1.0 + p["temp"] / 100.0
    gain_b = 1.0 - p["temp"] / 100.0
    lut_r = build_channel_lut(gain_r, p["highlight"], p["shadow"])
    lut_g = build_channel_lut(1.0, p["highlight"], p["shadow"])
    lut_b = build_channel_lut(gain_b, p["highlight"], p["shadow"])

    graded = img.point(lut_r + lut_g + lut_b)

    if p["saturation"]:
        hsv = graded.convert("HSV")
        h, s, v = hsv.split()
        s = s.point(build_saturation_lut(p["saturation"]))
        graded = Image.merge("HSV", (h, s, v)).convert("RGB")

    save_jpeg(graded, dst, quality=quality)
    return StyleResult(src, dst, preset, level, p, graded.width, graded.height)


__all__ = ["apply_style", "StyleResult", "build_channel_lut", "build_saturation_lut"]
