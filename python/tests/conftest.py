"""pytest 引导：把 `python/` 加进 sys.path，使 `import agent.*` 可用。

跑法（项目根目录）：
    python/.venv-agent/Scripts/python.exe -m pytest python/tests -q
"""
from __future__ import annotations

import sys
from pathlib import Path

_PY_DIR = Path(__file__).resolve().parents[1]
if str(_PY_DIR) not in sys.path:
    sys.path.insert(0, str(_PY_DIR))
