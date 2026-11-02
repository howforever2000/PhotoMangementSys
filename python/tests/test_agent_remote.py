"""预算闸 + 规划层 + P1b 端到端（模型只填槽位、执行仍走本地确定路径）。

这里全部用 MockTransport 假扮"用户的模型端点"，因此可以断言**调用次数**：
- `maxCalls=0` 时一次都不许发（P0/P1a/P1c 的零成本保证）；
- P1b 整个 job 只发 2 次（1 次规划 + 每张 1 次出图）；
- 预算不够时在**发请求之前**中止，状态收敛为 budget。
"""
from __future__ import annotations

import asyncio
import base64
import json
import random
from pathlib import Path

import httpx
import pytest
from PIL import Image

from agent.budget import BudgetExceeded, BudgetGuard, PriceTable
from agent.jobs import Job
from agent.planner import Planner, PlannerError
from agent.prompt import CompileInput, compile_prompt
from agent.providers.base import Endpoint
from agent.providers.openai_compat import OpenAICompat
from agent.recipes.schema import load_registry
from agent.runtime import RunEnv, run_job

REG = load_registry()

PNG_1PX = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=="
)


def make_photo(path: Path, seed: int = 1) -> str:
    rnd = random.Random(seed)
    w, h = 240, 180
    img = Image.frombytes("RGB", (w, h), bytes(rnd.getrandbits(8) for _ in range(w * h * 3)))
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(str(path), quality=90)
    return str(path)


def ep(kind: str, model: str = "m", **kw) -> Endpoint:
    return Endpoint(
        base_url="https://api.example.com/v1",
        api_key="sk-test-abcdef123456",
        model=model,
        kind=kind,
        unit_price=0.15,
        **kw,
    )


# ------------------------------------------------------------------ 预算


def test_max_calls_zero_blocks_every_model_call():
    """这条是 P0/P1a/P1c 的机制保证：不是"我们没写 http"，而是代码层做不到。"""
    g = BudgetGuard(max_cost=10.0, max_calls=0)
    with pytest.raises(BudgetExceeded) as ei:
        g.before("image")
    assert "maxCalls=0" in str(ei.value)
    assert g.calls == 0 and g.spent == 0.0


def test_call_limit_blocks_the_third_call():
    g = BudgetGuard(max_cost=100.0, max_calls=2)
    g.before("image")
    g.before("image")
    with pytest.raises(BudgetExceeded) as ei:
        g.before("image")
    assert "次数上限" in str(ei.value)


def test_cost_limit_uses_price_table_estimate():
    g = BudgetGuard(max_cost=0.30, max_calls=10, table=PriceTable(image=0.15, planner=0.001))
    g.before("image")
    g.before("image")
    with pytest.raises(BudgetExceeded) as ei:
        g.before("image")  # 第三次会到 0.45
    assert "超过本次上限" in str(ei.value)
    assert abs(ei.value.spent - 0.30) < 1e-9


def test_settle_replaces_estimate_with_actual():
    g = BudgetGuard(max_cost=10.0, max_calls=5, table=PriceTable(image=0.2))
    reserved = g.before("image")
    g.settle(reserved, 0.05)
    assert g.spent == pytest.approx(0.05)
    assert g.calls == 1


def test_summary_is_serializable():
    g = BudgetGuard(max_cost=1.0, max_calls=3, table=PriceTable(image=0.2))
    g.before("image")
    s = g.summary()
    assert s["calls"] == 1 and s["remainingCalls"] == 2
    assert json.dumps(s)  # 不能含不可序列化对象


# ------------------------------------------------------------------ 规划层


def chat_reply(tool_name: str, args: dict, call_id: str = "c1") -> dict:
    return {
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": call_id,
                            "type": "function",
                            "function": {"name": tool_name, "arguments": json.dumps(args)},
                        }
                    ],
                }
            }
        ],
        "usage": {"total_tokens": 40},
    }


@pytest.mark.asyncio
async def test_fill_slots_parses_tool_call():
    seen: dict = {}

    def handler(request: httpx.Request) -> httpx.Response:
        seen.update(json.loads(request.content))
        return httpx.Response(
            200,
            json=chat_reply(
                "fill_slots",
                {"look": "anime_cel", "intensity": "strong", "note": "夕阳逆光"},
            ),
        )

    guard = BudgetGuard(max_cost=1.0, max_calls=3, table=PriceTable(planner=0.002, image=0.15))
    client = OpenAICompat(ep("planner"), transport=httpx.MockTransport(handler))
    planner = Planner(client, guard)

    recipe = REG.get("style.ai_reillustrate")
    slots, notes = await planner.fill_slots(recipe, recipe.prompt_template, "画成夕阳插画", [PNG_1PX])
    assert slots["look"] == "anime_cel"
    assert slots["intensity"] == "strong"
    assert notes and "模型" in notes[0]
    # 强制走 fill_slots 工具 + 传了图
    assert seen["tool_choice"]["function"]["name"] == "fill_slots"
    assert seen["tools"][0]["function"]["name"] == "fill_slots"
    assert seen["tools"][0]["function"]["parameters"]["properties"]["look"]["enum"] == [
        "anime_cel",
        "soft_paint",
        "ink_line",
    ]
    assert guard.calls == 1


@pytest.mark.asyncio
async def test_fill_slots_errors_when_model_ignores_tool():
    handler = lambda r: httpx.Response(  # noqa: E731
        200, json={"choices": [{"message": {"role": "assistant", "content": "我觉得挺好"}}]}
    )
    guard = BudgetGuard(max_cost=1.0, max_calls=2, table=PriceTable(planner=0.0))
    planner = Planner(OpenAICompat(ep("planner"), transport=httpx.MockTransport(handler)), guard)
    recipe = REG.get("style.ai_reillustrate")
    with pytest.raises(PlannerError) as ei:
        await planner.fill_slots(recipe, recipe.prompt_template, "x", [])
    assert "fill_slots" in str(ei.value)


@pytest.mark.asyncio
async def test_run_loop_executes_tools_then_finish():
    scripted = [
        chat_reply("compress", {"targetKb": 120}, "c1"),
        chat_reply("finish", {"message": "已压缩 1 张"}, "c2"),
    ]
    state = {"i": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        if body.get("tools"):
            idx = min(state["i"], len(scripted) - 1)
            state["i"] += 1
            return httpx.Response(200, json=scripted[idx])
        return httpx.Response(404)

    guard = BudgetGuard(max_cost=1.0, max_calls=5, table=PriceTable(planner=0.0))
    planner = Planner(OpenAICompat(ep("planner"), transport=httpx.MockTransport(handler)), guard)

    executed: list[tuple[str, dict]] = []

    async def dispatch(name: str, args: dict) -> dict:
        executed.append((name, args))
        return {"ok": True, "items": []}

    out = await planner.run_loop("压到 120KB", [], dispatch)
    assert out["status"] == "done"
    assert out["text"] == "已压缩 1 张"
    assert [e[0] for e in executed] == ["compress"]
    assert guard.calls == 2, "两次规划调用各过一次预算闸"


@pytest.mark.asyncio
async def test_run_loop_stops_on_budget():
    handler = lambda r: httpx.Response(200, json=chat_reply("compress", {"targetKb": 100}))  # noqa: E731
    guard = BudgetGuard(max_cost=10.0, max_calls=1, table=PriceTable(planner=0.0))
    planner = Planner(OpenAICompat(ep("planner"), transport=httpx.MockTransport(handler)), guard)

    async def dispatch(name: str, args: dict) -> dict:
        return {"ok": True}

    with pytest.raises(BudgetExceeded):
        await planner.run_loop("x", [], dispatch)


# ------------------------------------------------------------------ P1b 端到端


def fake_endpoint(fail_image: bool = False, chat_slots: dict | None = None):
    """一个假端点：/chat/completions 填槽位，/images/edits 返回 1px PNG。"""
    calls = {"chat": 0, "image": 0}
    slots = chat_slots or {"look": "anime_cel", "intensity": "moderate", "note": "夕阳"}

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("/chat/completions"):
            calls["chat"] += 1
            return httpx.Response(200, json=chat_reply("fill_slots", slots))
        if request.url.path.endswith("/images/edits"):
            calls["image"] += 1
            if fail_image:
                return httpx.Response(400, json={"error": "content policy"})
            return httpx.Response(
                200,
                json={"data": [{"b64_json": base64.b64encode(PNG_1PX).decode()}], "cost": 0.12},
            )
        return httpx.Response(404)

    return httpx.MockTransport(handler), calls


def run_p1b(tmp_path: Path, *, transport, calls, budget: dict, price: dict):
    recipe = REG.get("style.ai_reillustrate")
    paths = [make_photo(tmp_path / "a.jpg", 1), make_photo(tmp_path / "b.jpg", 2)]
    guard = BudgetGuard(
        max_cost=budget.get("maxCost", 5.0),
        max_calls=budget.get("maxCalls", 8),
        table=PriceTable.from_dict(price),
    )
    env = RunEnv(
        provider=OpenAICompat(ep("image"), transport=transport),
        planner=Planner(OpenAICompat(ep("planner"), transport=transport), guard),
        budget=guard,
        size="1024x1024",
        tier="draft",
    )
    compiled = compile_prompt(
        CompileInput(
            recipe=recipe,
            user_text="画成夕阳插画",
            tray_paths=paths,
            provider_ready=True,
        )
    )
    assert compiled.p1_path == "P1b" and compiled.plan_pending
    job = Job(3, recipe.id)
    asyncio.run(
        run_job(job, recipe, compiled, {"paths": paths, "outputDir": str(tmp_path / "out")}, env)
    )
    return job


def test_p1b_end_to_end_fills_slots_then_renders(tmp_path: Path):
    transport, calls = fake_endpoint()
    job = run_p1b(
        tmp_path,
        transport=transport,
        calls=calls,
        budget={"maxCost": 5.0, "maxCalls": 8},
        price={"image": 0.15, "planner": 0.002},
    )
    snap = job.snapshot()
    assert snap["status"] == "done", snap.get("error")
    assert snap["p1Path"] == "P1b"
    # 轨迹：选图 → 填参 → 每张重绘 → 体检 → 完成
    tools = [s["tool"] for s in snap["steps"]]
    assert tools[0] == "select" and "fill_slots" in tools
    assert tools.count("style_transfer_remote") == 2
    # 提示词是模板渲染出来的，锚点在最前
    assert snap["prompt"].startswith("Preserve the subject, pose, framing")
    assert "anime_cel" in snap["prompt"]
    # 产物 + 成本
    assert len(snap["outputs"]) == 2
    assert all(Path(o).is_file() for o in snap["outputs"])
    assert snap["costTotal"] > 0
    # 调用次数：1 次规划 + 2 次出图
    assert calls == {"chat": 1, "image": 2}
    assert snap["callsUsed"] >= 1


def test_p1b_stops_before_spending_when_budget_is_tight(tmp_path: Path):
    transport, calls = fake_endpoint()
    # 只够规划一次 + 图 0 张（image 单价 0.15，maxCost 0.16 → 第二张就超）
    job = run_p1b(
        tmp_path,
        transport=transport,
        calls=calls,
        budget={"maxCost": 0.16, "maxCalls": 8},
        price={"image": 0.15, "planner": 0.0},
    )
    snap = job.snapshot()
    assert snap["status"] == "budget", snap
    assert "上限" in (snap["error"] or "")
    assert calls["image"] == 1, "超预算时不得继续发请求"


def test_p1b_image_failure_is_reported_not_crashed(tmp_path: Path):
    transport, calls = fake_endpoint(fail_image=True)
    job = run_p1b(
        tmp_path,
        transport=transport,
        calls=calls,
        budget={"maxCost": 5.0, "maxCalls": 8},
        price={"image": 0.15, "planner": 0.002},
    )
    snap = job.snapshot()
    assert snap["status"] == "error"
    assert snap["outputs"] == []


def test_p1b_rejects_when_no_provider_configured(tmp_path: Path):
    recipe = REG.get("style.ai_reillustrate")
    with pytest.raises(Exception) as ei:
        compile_prompt(
            CompileInput(recipe=recipe, user_text="画成插画", tray_paths=["a.jpg"], provider_ready=False)
        )
    assert "模型地址" in str(ei.value)
