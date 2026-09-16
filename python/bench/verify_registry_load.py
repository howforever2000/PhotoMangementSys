"""模型注册表加载回归测试（BUG-2026-0921-003 防护）

背景（为什么需要这个测试）：
  `ModelRegistry._load` 曾把未定义的 `opts` 传给 `_session_facts`，NameError 被
  宽泛的 except 吞掉 → `_ready['det'] = False` 且被**永久缓存** → `/health.ok`
  恒为 false → 宿主的 ensure(wait_models=true) 每次等满 90s 后失败 → 「识别性能
  设置」面板三个请求相继卡住，UI 表现为「一直转圈、不能选模型和线程」，
  而日志里一行都没有（性能端点在 Rust 侧此前无埋点）。

本测试锁死以下不变量：
  1. `_load` 成功路径不抛异常、通道就绪、错误表为空；
  2. 会话实测事实里的 threads 与实际 SessionOptions 一致（`opts` 复用正确）；
  3. 文件缺失 / 加载失败时错误可见（load_errors 非空）而不是静默；
  4. 失败后的冷却重试：冷却期内不重复尝试，冷却期满允许重试；
  5. `_reload_all`（GPU 开关 / 线程数切换用）会清掉失败登记。

用法：
  python python/bench/verify_registry_load.py
退出码 0 = 全部通过。
"""
import os
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from vcr import config  # noqa: E402
from vcr import model_registry as mr  # noqa: E402


class Case:
    def __init__(self) -> None:
        self.passed = 0
        self.failed = 0

    def check(self, name: str, ok: bool, detail: str = "") -> None:
        mark = "PASS" if ok else "FAIL"
        print(f"[{mark}] {name}" + (f" | {detail}" if detail else ""))
        if ok:
            self.passed += 1
        else:
            self.failed += 1


def test_real_det_load(c: Case) -> None:
    """真实模型加载：必须就绪，且 threads 事实与实际会话选项一致。"""
    reg = mr.ModelRegistry()
    sess = reg.det
    c.check("det 模型加载非 None", sess is not None,
            f"load_errors={reg.load_errors()}")
    c.check("is_ready('det') 为 True", reg.is_ready("det"))
    c.check("load_errors 为空", reg.load_errors() == {}, str(reg.load_errors()))
    facts = reg._session_info.get("det") or {}
    c.check(
        "会话实测 threads == config.threads()",
        facts.get("threads") == config.threads(),
        f"facts.threads={facts.get('threads')} config.threads()={config.threads()}",
    )
    c.check("会话实测 providers 非空", bool(facts.get("providers")), str(facts.get("providers")))
    # /health 口径：ok 取决于 det 就绪
    c.check("health.ok 语义成立（det 就绪）", reg.is_ready("det"))


def test_missing_model_is_visible(c: Case) -> None:
    """模型缺失：必须记入 load_errors（而不是静默 False）。"""
    reg = mr.ModelRegistry()
    reg._load("ghost", [os.path.join(config.MODEL_DIR, "__not_exist__.onnx")])
    c.check("缺失模型 is_ready=False", reg.is_ready("ghost") is False)
    c.check("缺失模型 load_errors 非空", bool(reg.load_errors().get("ghost")),
            str(reg.load_errors()))


def test_failure_cooldown_retry(c: Case) -> None:
    """加载失败 → 冷却期内不重试；冷却期满自动重试（避免一次抖动永久锁死通道）。"""
    reg = mr.ModelRegistry()
    bad = os.path.join(config.MODEL_DIR, "__not_exist__.onnx")
    calls = {"n": 0}
    real_isfile = os.path.isfile

    def counting_isfile(p):  # noqa: ANN001
        if p == bad:
            calls["n"] += 1
        return real_isfile(p)

    os.path.isfile = counting_isfile  # type: ignore[assignment]
    try:
        reg._load("ghost", [bad])
        first = calls["n"]
        reg._load("ghost", [bad])  # 冷却期内 → 不应再探测文件
        c.check("冷却期内不重复尝试加载", calls["n"] == first,
                f"first={first} now={calls['n']}")
        # 把失败时刻往前拨，模拟冷却期满
        reg._failed_at["ghost"] = time.monotonic() - mr.LOAD_RETRY_COOLDOWN - 1
        reg._load("ghost", [bad])
        c.check("冷却期满允许重试", calls["n"] > first,
                f"first={first} now={calls['n']}")
    finally:
        os.path.isfile = real_isfile  # type: ignore[assignment]


def test_reload_clears_failures(c: Case) -> None:
    """_reload_all（GPU/线程切换路径）必须清掉失败登记，让通道有机会恢复。"""
    reg = mr.ModelRegistry()
    reg._load("ghost", [os.path.join(config.MODEL_DIR, "__not_exist__.onnx")])
    c.check("切换前存在失败登记", bool(reg.load_errors()))
    reg._reload_all()
    c.check("_reload_all 后失败登记清空", reg.load_errors() == {})
    c.check("_reload_all 后 ready 清空", reg.is_ready("ghost") is False and not reg._ready)


def main() -> int:
    c = Case()
    print(f"MODEL_DIR = {config.MODEL_DIR}")
    print(f"config.threads() = {config.threads()}")
    test_real_det_load(c)
    test_missing_model_is_visible(c)
    test_failure_cooldown_retry(c)
    test_reload_clears_failures(c)
    print(f"\n汇总：通过 {c.passed} / 失败 {c.failed}")
    return 0 if c.failed == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
