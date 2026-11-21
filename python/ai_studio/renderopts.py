"""渲染参数载体（FEAT-197）：文生图模型的采样步数随作业下传到算子层。

## 为什么单独一个模块

步数是**作业级**参数——一次出片里所有云端编辑调用共用同一套步数，用户只在一个
地方选。但它只在算子层被消费（`ops` → `providers.images_edit`），于是有两个都不
能用的传递通道：

  - 塞进 `step["params"]`：那是 `op_catalog.json` 声明的**算子参数**，
    `catalog.norm_params()` 只按 schema 返回键，未声明的键会被静默丢弃
    （`catalog.py` 的 `_norm_one` 只遍历 schemas）。表现出来就是「选了步数，
    出图完全没变」——最难查的那种死控件。
  - 挂到 `providers`：那是「服务商 + 凭证 + 模型名」的配置，混进渲染参数会让
    「provider 配置」的语义失真，也会让 provider 的连通性测试莫名带上步数。

所以用一个显式上下文对象，沿 `execute_plan → 算子` 传下去；本机算子收下但不使用。

## 为什么缺省不往请求里放 `steps`

`images_edit` 的 `steps` 只在调用方显式给出时才进 body（旧实现就是这个口径）。
默认 1 是**界面语义**（用户看到的默认档），不是「向网关声明 1 步」：各家网关对
`steps` 的默认值不同，把 1 硬塞过去等于替用户改了出图质量，而用户什么都没选。
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from PIL import Image

# 界面默认档（「不可设置时默认为 1」）
DEFAULT_RENDER_STEPS = 1
# 上限：再高的步数在 1K 出图上收益递减，却让单张耗时线性上涨
MAX_RENDER_STEPS = 50


def clamp_render_steps(raw: Any) -> int:
    """任意来源（前端 / 作业 options）的步数 → [1, MAX] 整数，非法回落默认 1。"""
    try:
        v = int(raw)
    except (TypeError, ValueError):
        return DEFAULT_RENDER_STEPS
    return max(1, min(MAX_RENDER_STEPS, v))


@dataclass(frozen=True)
class RunContext:
    """一次作业的渲染上下文。本机算子接收但不使用（保持算子签名统一）。

    FEAT-208 起，「照片级」字段随本对象一并下传，由 `pipeline.execute_plan`
    在**每张照片**入口处用 `dataclasses.replace` 派生独立副本（作业级字段
    render_steps / style_prompt 原样继承，照片级字段每图新建）—— 批量照片级
    并行下互不可见，不存在跨照片串味。
    """

    render_steps: int = DEFAULT_RENDER_STEPS
    """文生图采样步数（FEAT-197）。"""

    style_prompt: str = ""
    """影调风格段落（FEAT-198，`modelstyle.compose()` 的结果）。

    为什么走这里而不是 `params.extra`：`catalog.norm_params` 对 string 参数截到
    500 字，而一段风格描述（中文原文 + 英文要点）常到 600~900 字 —— 塞进去会被
    **静默截断**，用户看到「选了风格但只有前半段生效」。作业级上下文不经过参数
    夹取，是唯一能完整送达的位置。
    """

    source: Image.Image | None = field(default=None, compare=False)
    """本张照片的原图引用（FEAT-208，照片级）。

    「肤色保真」必须拿滤镜链**之前**的像素当锚点，而算子链只传上一步输出 ——
    execute_plan 的入参 img 就是原图，经此字段下传给需要它的算子。不进参数
    夹取（schema 只收标量），也不可比较（PIL Image 无稳定相等语义）。
    """

    scratch: dict = field(default_factory=dict, compare=False)
    """同一条链内、同一次 execute_plan 里的跨步共享槽（FEAT-208，照片级）。

    现在只有一个键：`face_analysis` —— 分区磨皮已经跑过的 SCRFD + BiSeNet
    结果（[(bbox, box, skin_mask)]），肤色保真直接复用，不再重复推理。
    纪律：
      - 只在同一次 execute_plan 内有效（每张照片 replace 出新 dict）；
      - 改几何的算子（形变/超分）必须清掉它，防止掩码与像素错位；
      - 消费方用 pop 取走，杜绝二次误用。
    """

    def gateway_steps(self) -> int | None:
        """要写进请求体的 `steps`；None = 不写（让网关用自己的默认值）。"""
        return self.render_steps if self.render_steps > DEFAULT_RENDER_STEPS else None

    def apply_style(self, prompt: str) -> str:
        """把风格段落追加到提示词末尾（空风格原样返回）。"""
        block = (self.style_prompt or "").strip()
        if not block:
            return prompt
        return f"{prompt.strip()} {block}".strip()


EMPTY = RunContext()


__all__ = [
    "DEFAULT_RENDER_STEPS",
    "MAX_RENDER_STEPS",
    "RunContext",
    "EMPTY",
    "clamp_render_steps",
]
