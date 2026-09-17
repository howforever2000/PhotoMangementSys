"""P0c：抽样导出——给定关键词与其阈值，打印 top-N 命中缩略图路径（供人工/视觉模型抽查精度）

用法：python python/bench/calib_top.py "一只猫" 0.38 8
"""
import json
import os
import sys

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)
from vcr.services.embed_service import get_embed_service  # noqa: E402

X = np.load(os.path.join(HERE, "out", "embeddings.npy")).astype(np.float32)
X = X / (np.linalg.norm(X, axis=1, keepdims=True) + 1e-9)
PATHS = json.load(open(os.path.join(HERE, "out", "paths.json"), encoding="utf-8"))

kw = sys.argv[1]
thr = float(sys.argv[2]) if len(sys.argv) > 2 else 0.38
topn = int(sys.argv[3]) if len(sys.argv) > 3 else 8
skip = int(sys.argv[4]) if len(sys.argv) > 4 else 0
svc = get_embed_service()
t = svc.embed_text(kw).astype(np.float32)
s = X @ t
order = np.argsort(-s)
hits = [i for i in order if s[i] >= thr]
print(f"# {kw} @ {thr}: 命中 {len(hits)}/{len(s)}；以下 第{skip + 1}~{skip + topn} 名")
for i in order[skip:skip + topn]:
    print(f"{float(s[i]):.3f}\t{PATHS[i]}")
