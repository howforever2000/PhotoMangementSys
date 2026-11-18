"""结构化步骤事件收集（FEAT-168）

`pipeline.execute_plan(on_step=...)` 产生逐步事件，本模块把它收成一份可读的
`steps[]`（running 会被随后的 done/failed 覆盖），供工作流通过
`progress(..., steps=...)` 上报给 jobs.py，最终落在 `AiJobStatus.steps`。

只做收集，不含图像处理、不含作业状态、不含 I/O。
"""
from __future__ import annotations

from typing import Any, Dict, List


class StepTrace:
    def __init__(self) -> None:
        self._events: List[Dict[str, Any]] = []

    def on_step(self, event: Dict[str, Any]) -> None:
        """同 index 的 running 会被后续终态覆盖（前端只显示每步一行）。"""
        idx = event["index"]
        for i, old in enumerate(self._events):
            if old["index"] == idx:
                self._events[i] = dict(event)
                return
        self._events.append(dict(event))

    def skipped(self, index: int, total: int, op: str, label: str, reason: str) -> None:
        self.on_step({"index": index, "total": total, "op": op, "label": label,
                      "status": "skipped", "ms": 0, "issues": [reason]})

    def snapshot(self) -> List[Dict[str, Any]]:
        return [dict(e) for e in sorted(self._events, key=lambda x: x["index"])]

    def reset(self) -> None:
        self._events.clear()


__all__ = ["StepTrace"]
