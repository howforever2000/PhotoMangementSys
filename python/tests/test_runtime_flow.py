"""确定性执行器：端到端跑一遍，断言产物真的存在且成本恒为 0。

这一层是"点一下就出活"的兑现处，所以测试也按用户视角写：
给几张图 + 一句描述（或没有描述）→ 磁盘上真的多出图 / 一份 PDF，
轨迹里能看到每一步，成本一栏始终是 0。
"""
from __future__ import annotations

import asyncio
import random
from pathlib import Path

from PIL import Image

from agent.jobs import Job
from agent.prompt import CompileInput, compile_prompt
from agent.recipes.schema import load_registry
from agent.runtime import run_job

REG = load_registry()


def make_photo(path: Path, size=(900, 600), seed: int = 1) -> str:
    rnd = random.Random(seed)
    w, h = size
    img = Image.frombytes("RGB", (w, h), bytes(rnd.getrandbits(8) for _ in range(w * h * 3)))
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(str(path), quality=92)
    return str(path)


def prepare(tmp_path: Path, recipe_id: str, paths: list[str], text: str = "", override: dict | None = None):
    recipe = REG.get(recipe_id)
    compiled = compile_prompt(
        CompileInput(
            recipe=recipe, user_text=text, tray_paths=paths, run_spec_override=override
        )
    )
    job = Job(1, recipe_id)
    body = {"paths": paths, "outputDir": str(tmp_path / "out")}
    asyncio.run(run_job(job, recipe, compiled, body))
    return job, compiled


def test_compress_job_produces_files_and_zero_cost(tmp_path: Path):
    paths = [make_photo(tmp_path / f"src{i}.jpg", seed=i) for i in range(3)]

    # ① 不写描述 = P0（纯预设）；② 写描述但预设不消费 → P1c（描述只进轨迹）
    job_p0, _ = prepare(tmp_path, "compress.target_kb", paths, override={"export": {"targetKb": 150}})
    assert job_p0.snapshot()["p1Path"] == "P0"

    job, compiled = prepare(
        tmp_path, "compress.target_kb", paths, text="压小一点", override={"export": {"targetKb": 150}}
    )

    snap = job.snapshot()
    assert snap["status"] == "done", snap.get("error")
    assert snap["costTotal"] == 0.0, "零成本路径的成本必须恒为 0"
    assert snap["callsUsed"] == 0, "没有模型调用"
    assert snap["p1Path"] == "P1c", "带描述的不消费预设走 P1c"

    outs = snap["outputs"]
    assert len(outs) == 3
    for o in outs:
        p = Path(o)
        assert p.is_file() and p.stat().st_size > 0
        assert p.stat().st_size <= 150 * 1024, f"{p.name} 超过目标体积"
    # 轨迹：选图 → 每张一条 → 体检 → 汇总
    tools = [s["tool"] for s in snap["steps"]]
    assert tools[0] == "select"
    assert tools.count("compress") == 3
    assert "probe" in tools and tools[-1] == "done"
    assert all(s["cost"] == 0 for s in snap["steps"])


def test_style_job_maps_intensity_and_names_output(tmp_path: Path):
    paths = [make_photo(tmp_path / "a.jpg", seed=3)]
    job, compiled = prepare(tmp_path, "style.warm_dusk", paths, text="再暖一点")
    snap = job.snapshot()
    assert snap["status"] == "done", snap.get("error")
    assert snap["p1Path"] == "P1a"
    assert snap["promptSource"] == "rule_mapped"
    assert snap["prompt"] is None
    assert any("intensity=strong" in a for a in snap["assumptions"])
    assert len(snap["outputs"]) == 1
    assert "warm_dusk" in Path(snap["outputs"][0]).name
    assert snap["costTotal"] == 0.0


def test_card_job_exports_pdf(tmp_path: Path):
    paths = [make_photo(tmp_path / f"c{i}.jpg", size=(800, 600), seed=10 + i) for i in range(5)]
    job, compiled = prepare(tmp_path, "card.magazine_3up", paths)
    snap = job.snapshot()
    assert snap["status"] == "done", snap.get("error")
    pdfs = [o for o in snap["outputs"] if o.lower().endswith(".pdf")]
    assert len(pdfs) == 1, snap["outputs"]
    assert Path(pdfs[0]).read_bytes()[:4] == b"%PDF"
    card_step = next(s for s in snap["steps"] if s["tool"] == "card_render")
    assert "2 页" in (card_step["note"] or ""), "5 张 / 每页 3 张 → 2 页"


def test_batch_job_chains_style_then_compress(tmp_path: Path):
    paths = [make_photo(tmp_path / f"b{i}.jpg", size=(1600, 1200), seed=20 + i) for i in range(2)]
    job, _ = prepare(tmp_path, "batch.party_material", paths)
    snap = job.snapshot()
    assert snap["status"] == "done", snap.get("error")
    tools = [s["tool"] for s in snap["steps"]]
    assert tools.count("style_transfer") == 2
    assert tools.count("compress") == 2
    assert len(snap["outputs"]) == 2
    for o in snap["outputs"]:
        assert Path(o).stat().st_size <= 400 * 1024


def test_empty_tray_is_an_actionable_error(tmp_path: Path):
    job, _ = prepare(tmp_path, "style.warm_dusk", [])
    snap = job.snapshot()
    assert snap["status"] == "error"
    assert "素材托盘" in (snap["error"] or "")


def test_cancel_stops_after_current_image(tmp_path: Path):
    paths = [make_photo(tmp_path / f"x{i}.jpg", seed=30 + i) for i in range(6)]
    recipe = REG.get("compress.target_kb")
    compiled = compile_prompt(
        CompileInput(recipe=recipe, tray_paths=paths, run_spec_override={"export": {"targetKb": 200}})
    )
    job = Job(2, recipe.id)
    job.request_cancel()  # 开跑前就取消
    asyncio.run(run_job(job, recipe, compiled, {"paths": paths, "outputDir": str(tmp_path / "o2")}))
    snap = job.snapshot()
    assert snap["status"] == "cancelled"
    assert snap["outputs"] == [], "取消后不应继续产出"


def test_overwrite_false_never_clobbers(tmp_path: Path):
    paths = [make_photo(tmp_path / "dup.jpg", seed=44)]
    j1, _ = prepare(tmp_path, "style.warm_dusk", paths)
    j2, _ = prepare(tmp_path, "style.warm_dusk", paths)
    o1 = j1.snapshot()["outputs"][0]
    o2 = j2.snapshot()["outputs"][0]
    assert o1 != o2, "同名产物必须另存，不覆盖"
    assert Path(o1).is_file() and Path(o2).is_file()
