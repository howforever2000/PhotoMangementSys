"""服务层：分类通道（YOLOv8-cls）

关键修复：ONNX 导出输出已是概率分布（值域 [0,1]，实测 top1 可达 0.94），
**不再套 softmax**——旧实现二次 softmax 把置信度压到 ~0.005，属 P0 bug。
"""
from dataclasses import dataclass, field

import numpy as np
from PIL import Image

from .. import config, preprocess
from ..mapping import get_mapping


@dataclass
class ClsOutcome:
    top_idx: int
    probs: np.ndarray                 # 完整 1000 维概率
    category: str
    sub_category: str = ""
    label: str = ""
    confidence: float = 0.0
    cat_scores: list[tuple[str, float, str]] = field(default_factory=list)  # (cat, score, label)
    ready: bool = False
    error: str = ""


def run(img: Image.Image, registry) -> ClsOutcome:
    sess = registry.cls
    if sess is None:
        return ClsOutcome(0, np.zeros(1000), "other", ready=False, error="分类模型缺失")

    tensor = preprocess.cls_tensor(img)
    out = registry.run("cls", tensor)[0][0]      # (1000,)，已是概率
    probs = np.clip(out, 0.0, 1.0)
    if probs.sum() > 0:
        probs = probs / probs.sum()

    mapping = get_mapping()
    topk = np.argsort(probs)[::-1][: config.TOP_K]

    # 大类加权投票：同一大类下 top-k 细类概率求和
    cat_score: dict[str, float] = {}
    cat_label: dict[str, str] = {}
    for i in topk:
        cat = mapping.category_of(int(i))
        cat_score[cat] = cat_score.get(cat, 0.0) + float(probs[i])
        if cat not in cat_label:
            cat_label[cat] = mapping.classes[int(i)]
    ranking = sorted(cat_score.items(), key=lambda kv: kv[1], reverse=True)

    top_idx = int(topk[0])
    category = ranking[0][0] if ranking else "other"
    return ClsOutcome(
        top_idx=top_idx,
        probs=probs,
        category=category,
        sub_category=mapping.sub_of(top_idx),
        label=cat_label.get(category, ""),
        confidence=float(probs[top_idx]),
        cat_scores=[(c, s, cat_label.get(c, "")) for c, s in ranking],
        ready=True,
    )
