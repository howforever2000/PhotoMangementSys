"""发长判定（性别侧重的零新增模型启发式，FEAT-209）。

**只做一件事**：给 BiSeNet 的人脸解析类别图 + 检测框几何，判断「头发是否垂到脖颈
以下」。本模块不判性别 —— 「长发 → 女性侧重」是产品口径，在 `ops_local` 的编排层落地。

分层纪律与 `skintone` / `skinretouch` 一致：纯函数、不读盘、不调模型，输入是布尔掩码
（头发类别的像素），因此可离线单测。

## 规则（2026-11-22 在 40 张真实人像上定标）

下颌线**以下**还有成片头发，且那一片确实贴在脸侧 → 长发。两个条件必须同时成立：

  - 只看「底边有没有头发」会误判：帽子 / 兜帽会被解析成 hair 类
    （实测婴儿兜帽 edge=0.21，但侧向占比只有 0.01）；
  - 只看「侧向有没有头发」会把鬓角当长发（男性鬓角在脸侧、但在下颌线以上）。

## 局限（写在这里，避免后人重复验证）

盘发 / 扎高马尾 / 贴头皮短发 → 判成「非长发」，回落到男性侧重档。这是**刻意的
宽松失败模式**：男性档只做「还原原肤色 + 轻提亮」，判错不毁片（女性档的白 + 嫩红
更强）。实测盘发样本的女性召回约 2/8，散发/齐肩发样本正常命中。
"""
from __future__ import annotations

from dataclasses import dataclass

import numpy as np

# 头发像素太少（脸太小 / 无头发 / 光头）时不判，宁可不判也不猜
MIN_HAIR_PX = 200
# 下颌线容差（crop 相对高度）：检测框底边不是精确的下颌
CHIN_TOL = 0.02
# 「底边」带：crop 最下面 4%
EDGE_BAND = 0.96
# 「脸侧」列：左右各 30%
SIDE_MARGIN = 0.30
# 判决阈值（两组条件取「或」，每组都必须同时满足「面积 + 侧向」）
MIN_EDGE_HAIR = 0.04
MIN_SIDE_HAIR = 0.05
MIN_BELOW_HAIR = 0.12
MIN_SIDE_BELOW_HAIR = 0.10


@dataclass(frozen=True)
class HairStats:
    """一张脸头发在下颌线以下的分布指标（crop 相对坐标）。"""

    chin_y: float        # 下颌线在 crop 内的高度比例（外扩 30% 时约 0.81）
    below_ratio: float   # 下颌线以下的头发占全部头发的比例
    edge_ratio: float    # 底边带里头发占该带面积的比例
    side_ratio: float    # 下颌线以下、脸侧列里的头发占全部头发的比例

    @property
    def is_long(self) -> bool:
        """是否「长发垂到脖颈」。两组证据任一成立即可，但每组都要面积 + 侧向。"""
        by_edge = self.edge_ratio >= MIN_EDGE_HAIR and self.side_ratio >= MIN_SIDE_HAIR
        by_below = (
            self.below_ratio >= MIN_BELOW_HAIR
            and self.side_ratio >= MIN_SIDE_BELOW_HAIR
        )
        return bool(by_edge or by_below)


def hair_stats(
    hair_mask: np.ndarray,
    bbox: tuple[int, int, int, int],
    box: tuple[int, int, int, int],
) -> HairStats | None:
    """统计一张脸的头发分布。`bbox` 是检测框、`box` 是该 crop 在原图上的位置。

    `hair_mask` 与 crop 同尺寸；`hair_mask` 为空/头发太少返回 None（调用方按「非长发」处理）。
    """
    mask = np.asarray(hair_mask, dtype=bool)
    h, w = mask.shape
    if h < 8 or w < 8:
        return None
    total_hair = int(mask.sum())
    if total_hair < MIN_HAIR_PX:
        return None

    x1, y1, x2, y2 = box
    chin_y = (bbox[3] - y1) / max(y2 - y1, 1)
    chin_y = float(min(max(chin_y, 0.0), 1.0))

    yy, xx = np.mgrid[0:h, 0:w]
    fy = yy / max(h - 1, 1)
    fx = xx / max(w - 1, 1)
    below = fy > max(chin_y - CHIN_TOL, 0.0)
    edge = fy > EDGE_BAND
    side = (fx < SIDE_MARGIN) | (fx > 1.0 - SIDE_MARGIN)
    edge_area = int(edge.sum())

    return HairStats(
        chin_y=chin_y,
        below_ratio=float((mask & below).sum()) / total_hair,
        edge_ratio=float((mask & edge).sum()) / edge_area if edge_area else 0.0,
        side_ratio=float((mask & below & side).sum()) / total_hair,
    )


def looks_long_hair(
    hair_mask: np.ndarray,
    bbox: tuple[int, int, int, int],
    box: tuple[int, int, int, int],
) -> bool:
    """便捷入口：统计不足（None）时按「非长发」处理。"""
    stats = hair_stats(hair_mask, bbox, box)
    return bool(stats and stats.is_long)


__all__ = [
    "HairStats",
    "hair_stats",
    "looks_long_hair",
    "MIN_HAIR_PX",
]
