"""预算闸（软闸，执行侧）。硬闸在 Rust（submit 前就拒，连 Python 都不启）。

## 两层闸为什么要都在

- **硬闸（Rust）**：拿"预估 × 张数"和用户设的 `maxCost` 比。超了直接拒绝，
  用户得到的是"这次会花 ¥3.6，超过你的上限 ¥0.5"，而不是跑一半被掐。
- **软闸（Python）**：每次模型调用**之前**用真实单价再算一遍。
  Rust 的预估不可能精确（不知道模型会重试几次），所以执行期必须还有一道。

## maxCalls 是"零成本"的机制保证

P0/P1a/P1c 的 `maxCalls=0`：任何走模型的调用在 `before()` 里就抛。
于是"这三条路径不可能花钱"不是靠自觉，而是代码层面做不到（docs/v5.md §9）。
"""
from __future__ import annotations

from dataclasses import dataclass

ERR_BUDGET = "budget"


class BudgetExceeded(RuntimeError):
    """预算/调用数超限。job 会以 status=budget 收敛，并带上已花金额。"""

    def __init__(self, message: str, spent: float = 0.0, calls: int = 0):
        super().__init__(message)
        self.spent = spent
        self.calls = calls

    def to_dict(self) -> dict:
        return {"kind": ERR_BUDGET, "message": str(self), "spent": self.spent, "calls": self.calls}


@dataclass
class PriceTable:
    """用户自填单价（未知填 0 → 只作调用次数约束，金额记 estimated）。"""

    image: float = 0.0
    planner: float = 0.0

    @staticmethod
    def from_dict(d: dict | None) -> "PriceTable":
        d = d or {}
        return PriceTable(
            image=float(d.get("image") or 0.0),
            planner=float(d.get("planner") or 0.0),
        )

    def price(self, kind: str) -> float:
        return self.image if kind == "image" else self.planner


class BudgetGuard:
    """一次 job 内的账。所有方法都在同一条事件循环上调用（无需加锁）。"""

    def __init__(
        self,
        max_cost: float = 0.0,
        max_calls: int = 0,
        table: PriceTable | None = None,
        spent: float = 0.0,
        calls: int = 0,
    ):
        self.max_cost = float(max_cost or 0.0)
        self.max_calls = int(max_calls or 0)
        self.table = table or PriceTable()
        self.spent = float(spent or 0.0)
        self.calls = int(calls or 0)

    # ---- 查询 ----

    @property
    def remaining_calls(self) -> int:
        return max(0, self.max_calls - self.calls)

    @property
    def remaining_cost(self) -> float:
        return max(0.0, self.max_cost - self.spent)

    def summary(self) -> dict:
        return {
            "maxCost": round(self.max_cost, 4),
            "maxCalls": self.max_calls,
            "spent": round(self.spent, 6),
            "calls": self.calls,
            "remainingCalls": self.remaining_calls,
            "remainingCost": round(self.remaining_cost, 4),
        }

    # ---- 前置检查 ----

    def before(self, kind: str, estimated: float | None = None) -> float:
        """在**发起调用前**检查并预占。返回预占金额（调用后传给 settle）。"""
        if self.max_calls <= 0:
            raise BudgetExceeded(
                "本次任务被设定为不调用模型（maxCalls=0）：零成本路径不会出网。",
                self.spent,
                self.calls,
            )
        if self.calls + 1 > self.max_calls:
            raise BudgetExceeded(
                f"已达本次调用的次数上限（{self.max_calls} 次），仍有步骤需要模型。",
                self.spent,
                self.calls,
            )
        est = float(estimated) if estimated is not None else self.table.price(kind)
        est = max(0.0, est)
        if self.max_cost > 0 and self.spent + est > self.max_cost + 1e-9:
            raise BudgetExceeded(
                f"再调一次预计花 ¥{est:.4f}，会超过本次上限 ¥{self.max_cost:.2f}"
                f"（已花 ¥{self.spent:.4f}）。",
                self.spent,
                self.calls,
            )
        self.calls += 1
        self.spent += est
        return est

    def settle(self, reserved: float, actual: float | None) -> None:
        """用真实cost替换预占（拿不到真实值时保留预占，标记为估算由 Rust 侧处理）。"""
        if actual is None:
            return
        self.spent += float(actual) - float(reserved)
        if self.spent < 0:
            self.spent = 0.0
