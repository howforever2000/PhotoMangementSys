"""批量照片流程的公共骨架（照片级并行版）

## 为什么要有这一层

工坊的 8 个本地 workflow 原本各自写一份：

    outputs, warnings = [], []
    for i, p in enumerate(paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "xx 处理中", f"{i+1}/{n}")
        img = read_image(p)
        ...  # 逐张处理
        outputs.append(save_image(...))

照片级并行（见 `../parallel.py`）之后，这段骨架变成「提交 → 并行 → 按原序收」，
而**进度语义、取消语义、warning 与原 outputs 顺序**必须一字不变，否则前端的
`outputs[i]` 回填会错位、进度条会倒退。与其在 8 个文件里各写一遍正确的并发收尾，
不如收成这里一个函数。

## 语义约定（改动前务必读）

  - `process(index, path)` 返回 `Outcome`：本张的产物路径（可空）+ 本张的 warning
    + 可选 meta 合并（如统计的 faces 数）。**不要在 process 里直接 `warnings.append`**：
    多线程 append 的先后顺序不确定，会让 warning 列表顺序随机。
  - 结果按 `paths` 下标收：warning 顺序 = 照片顺序，`outputs` 顺序 = 照片顺序。
  - 单张抛异常 → 记一条 `"<文件名>: 处理失败（…）"` warning 并**继续**下一张
    （沿用既有降级纪律：单张失败绝不拖垮整批）。
  - 取消 → 未开工的剩余张不再提交；已完成的照常落盘，`on_cancel` 由调用方决定收尾。
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional

from ..parallel import is_cancelled, is_failed, map_photos, workers_for
from .base import check_cancel, stem_of


@dataclass
class Outcome:
    """单张的处理结果。`outputs` 为空表示这张不产出文件（如 OCR 无文字）。"""

    outputs: List[str] = field(default_factory=list)
    warnings: List[str] = field(default_factory=list)
    meta: Dict[str, Any] = field(default_factory=dict)


@dataclass
class BatchResult:
    outputs: List[str]
    warnings: List[str]
    meta: Dict[str, Any]


def run_batch(
    paths: List[str],
    process: Callable[[int, str], Outcome],
    progress: Callable[..., None],
    cancel,
    stage: str,
    onnx_heavy: bool = False,
    workers: Optional[int] = None,
    initial_warnings: Optional[List[str]] = None,
    stats: Optional[Dict[str, int]] = None,
    on_result: Optional[Callable[[int, Outcome], None]] = None,
    on_done: Optional[Callable[[int, int], None]] = None,
) -> BatchResult:
    """并行跑一批照片，按原序返回产物/warning。

    `stage` 是进度文案（如「自适应调色中」），进度区间沿用既有约定：
    3% 起步、95% 收尾，中间按**完成张数**线性推进。

    `stats` 若给出，会把每张 `Outcome.meta` 里的同名整型字段累加进去
    （各 workflow 的 `faces` / `images` 之类统计原本是手工累加的）。

    `on_result(index, outcome)` 在**收尾阶段按原序**逐张回调，供调用方取「最后一张
    的参数」这类需要确定下标顺序的数据（并行完成顺序是乱序的，不能直接用完成顺序）。

    `on_done(finished, total)` 覆盖默认的进度推进：需要把「结构化步骤」一起上报的
    流程（`recipe`）用它，否则默认进度只推「已完成 X/Y 张」，步骤卡收不到更新。
    """
    total = len(paths)
    n_workers = workers if workers is not None else workers_for(onnx_heavy)
    span = 92.0 / total if total else 0.0

    def _default_done(finished: int, _total: int) -> None:
        # 进度按完成张数推进：并行下完成顺序乱序，按计数才不会倒退
        progress(
            3 + int(span * finished),
            stage,
            f"{finished}/{total}",
        )

    notify = on_done if on_done is not None else _default_done

    def _task(index: int, path: str) -> Outcome:
        check_cancel(cancel)
        return process(index, path)

    slots = map_photos(paths, _task, cancel=cancel, workers=n_workers, on_done=notify)

    outputs: List[str] = []
    warnings: List[str] = list(initial_warnings or [])
    cancelled = 0
    for index, slot in enumerate(slots):
        name = stem_of(paths[index])
        if is_cancelled(slot):
            cancelled += 1
            continue
        if is_failed(slot):
            warnings.append(f"{name}: 处理失败（{slot.reason()}）")
            continue
        assert isinstance(slot, Outcome)
        outputs.extend(slot.outputs)
        warnings.extend(slot.warnings)
        if on_result is not None:
            on_result(index, slot)
        if stats is not None:
            for k, v in slot.meta.items():
                if isinstance(v, int):
                    stats[k] = stats.get(k, 0) + v

    if cancelled:
        # 取消不是错误：已完成的产物保留，未完成的不落盘、不告警
        warnings.append(f"已取消，剩余 {cancelled} 张未处理")
    return BatchResult(outputs=outputs, warnings=warnings, meta={})


__all__ = ["BatchResult", "Outcome", "run_batch"]
