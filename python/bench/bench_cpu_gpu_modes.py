"""实测脚本：CPU / GPU 两模式下「分类 + 语义」两条腿的分批耗时与模型段耗时。

用法（仓库根目录执行，用 VCR 自己的 venv）：
  python/.venv-vcr/Scripts/python.exe python/bench/bench_cpu_gpu_modes.py ^
      --round "cpu-p6:cpu:6" --round "gpu-p1:gpu:1"

每轮独立起一个 VCR 微服务实例（不同端口），跑 N 张真实照片（分类 + 语义各若干批），
收集「客户端侧批耗时」与「服务端 [vcr.stat] 模型段均摊」，最后打印汇总并落盘 JSON。

刻意设计：
  - 语义腿先生成 256px 缩略图再喂（对齐真实扫描：CLIP 吃的是缩略图，不是原图）；
  - 每轮独立进程：GPU 轮若段错误崩溃不影响其他轮（崩溃如实记录为 CRASHED）；
  - 客户端计时只测 HTTP 往返，服务端内部耗时由 [vcr.stat] 提供，两者可交叉验证；
  - **embed 之后再取一次 /health**（sessions_after）：CLIP 是懒加载的，只在 embed
    之后才出现在会话表里；不取这一次就永远问不出「CLIP 到底跑在哪个 provider 上」，
    而这决定了 embed 那段耗时能不能算到 GPU 头上。

已知结论（2026-09-28）：
  - GPU 模式 P≥2 会崩（DML 会话并发 run 段错误，BUG-2026-0923-001），本脚本如实
    记为 CRASHED_OR_FAILED，不要「改成 P=1 重跑」来掩盖。
"""
import argparse
import json
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

# 本文件位于 <repo>/python/bench/ ⇒ parents[2] 才是仓库根
ROOT = Path(__file__).resolve().parents[2]
PY = ROOT / "python" / ".venv-vcr" / "Scripts" / "python.exe"
CWD = ROOT / "python"
IMG_DIR = Path(r"D:/YUAN HAO/Pictures/2026/test")
N_IMAGES = 40        # 5 批：第 1 批当预热（模型懒加载 + 首次前向），统计只用后 4 批
BATCH = 8


def http(method, url, body=None, timeout=300):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode("utf-8"))


def wait_health(port, timeout=240, want_gpu=None):
    """等待服务就绪；want_gpu=True 时还要求会话确实绑到非 CPU provider。"""
    t0 = time.time()
    last = None
    while time.time() - t0 < timeout:
        try:
            h = http("GET", f"http://127.0.0.1:{port}/health", timeout=5)
            last = h
            if h.get("ok"):
                if want_gpu is None:
                    return h, None
                sess = (h.get("gpu") or {}).get("sessions") or {}
                provs = [p for v in sess.values() for p in (v or [])]
                if any(not str(p).startswith("CPU") for p in provs):
                    return h, None
            time.sleep(1.0)
        except Exception:
            time.sleep(1.0)
    return None, last


def make_thumbs(paths, outdir):
    """生成 256px 缩略图（对齐真实扫描：语义腿吃的是缩略图）。"""
    from PIL import Image
    outdir.mkdir(parents=True, exist_ok=True)
    outs = []
    for p in paths:
        dst = outdir / (Path(p).stem + ".jpg")
        if not dst.exists():
            with Image.open(p) as im:
                im = im.convert("RGB")
                im.thumbnail((256, 256), Image.BILINEAR)
                im.save(dst, "JPEG", quality=88)
        outs.append(str(dst))
    return outs


def run_round(tag, mode, parallel, port, log_dir):
    print(f"\n===== 轮次 {tag} (mode={mode} P={parallel} port={port}) =====", flush=True)
    err_path = log_dir / f"{tag}.stderr.log"
    env = dict(os.environ)
    env["VCR_PORT"] = str(port)
    env["VCR_PHOTO_PARALLEL"] = str(parallel)
    proc = None
    result = {"tag": tag, "mode": mode, "parallel": parallel, "status": "?"}
    try:
        with open(err_path, "wb") as ferr:
            proc = subprocess.Popen(
                [str(PY), "server.py"], cwd=str(CWD), env=env,
                stdout=ferr, stderr=subprocess.STDOUT,
            )
        # 1. 等服务就绪
        h, last = wait_health(port, timeout=240)
        if h is None:
            result["status"] = "BOOT_TIMEOUT"
            print("服务启动超时", last, flush=True)
            return result

        # 2. GPU 模式：切换开关并等会话真绑上 Dml
        if mode == "gpu":
            try:
                info = http("POST", f"http://127.0.0.1:{port}/gpu", {"enabled": True}, timeout=60)
                print("[gpu] 切换返回 provider =", info.get("provider"), flush=True)
            except Exception as e:
                print("[gpu] 切换失败:", e, flush=True)
            h, last = wait_health(port, timeout=180, want_gpu=True)
            if h is None:
                result["status"] = "GPU_SWITCH_TIMEOUT"
                print("[gpu] 会话未绑上 GPU provider，最后一个 health:",
                      json.dumps(last, ensure_ascii=False)[:400], flush=True)
        result["provider"] = ((h.get("gpu") or {}).get("provider") if h else None)
        result["sessions"] = ((h.get("gpu") or {}).get("sessions") if h else None)
        result["threads"] = h.get("threads") if h else None
        print("[health] provider =", result["provider"], "| sessions =", result["sessions"], flush=True)

        imgs = sorted([str(p) for p in IMG_DIR.iterdir()
                       if p.suffix.lower() in (".jpg", ".jpeg")])[:N_IMAGES]
        if not imgs:
            result["status"] = "NO_IMAGES"
            return result
        thumbs = make_thumbs(imgs, Path(os.environ.get("TEMP", ".")) / f"pm_bench_{tag}")

        # 3. 分类腿：分批计时（HTTP 往返口径）
        cls_batches = []
        for i in range(0, len(imgs), BATCH):
            chunk = imgs[i:i + BATCH]
            t0 = time.time()
            try:
                r = http("POST", f"http://127.0.0.1:{port}/classify_batch", {"paths": chunk})
                ok = len(r.get("results", []))
            except Exception as e:
                print("[classify] 批失败:", repr(e)[:200], flush=True)
                result["status"] = "CRASHED_OR_FAILED"
                return result
            ms = (time.time() - t0) * 1000.0
            cls_batches.append({"n": len(chunk), "ms": round(ms, 1), "results": ok})
            print(f"  classify 批 {i // BATCH + 1}: {len(chunk)}张 {ms:.0f}ms "
                  f"({ms / len(chunk):.1f}ms/张)", flush=True)

        # 4. 语义腿：缩略图 → /embed_batch（首次会触发 CLIP 懒加载）
        emb_batches = []
        for i in range(0, len(thumbs), BATCH):
            chunk = thumbs[i:i + BATCH]
            t0 = time.time()
            try:
                r = http("POST", f"http://127.0.0.1:{port}/embed_batch", {"paths": chunk})
                ok = len(r.get("results", []))
            except Exception as e:
                print("[embed] 批失败:", repr(e)[:200], flush=True)
                result["status"] = "EMBED_FAILED"
                return result
            ms = (time.time() - t0) * 1000.0
            emb_batches.append({"n": len(chunk), "ms": round(ms, 1), "results": ok})
            print(f"  embed    批 {i // BATCH + 1}: {len(chunk)}张 {ms:.0f}ms "
                  f"({ms / len(chunk):.1f}ms/张)", flush=True)

        # 4.1 embed 之后再取一次 health：CLIP 懒加载，此时才会出现在会话表里
        try:
            h2, _ = wait_health(port, timeout=15)
            result["sessions_after"] = ((h2.get("gpu") or {}).get("sessions") if h2 else None)
            result["health_after"] = h2
            print("[health-after] sessions =", result["sessions_after"], flush=True)
        except Exception as e:
            print("[health-after] 读取失败:", e, flush=True)

        result["status"] = "OK"
        result["classify"] = cls_batches
        result["embed"] = emb_batches
        # 5. 取服务端 [vcr.stat] 行
        time.sleep(0.5)
        try:
            txt = err_path.read_text(encoding="utf-8", errors="replace")
            result["server_stats"] = [ln.strip() for ln in txt.splitlines()
                                      if "[vcr.stat]" in ln or "[VCR]" in ln]
        except Exception as e:
            result["server_stats"] = [f"读取日志失败: {e}"]
        return result
    finally:
        if proc and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=15)
            except Exception:
                proc.kill()
        if result.get("status") == "?":
            result["status"] = "CRASHED"


def summarize(r):
    if r.get("status") != "OK":
        return f"{r['tag']}: {r['status']}"

    def agg(b):
        n = sum(x["n"] for x in b)
        ms = sum(x["ms"] for x in b)
        return n, ms, (ms / n if n else 0)

    # 稳态 = 去掉第 1 批（预热批含模型懒加载与首次前向，不能计入口径）
    cn1, cms1, _ = agg(r["classify"][:1])
    cn, cms, cper = agg(r["classify"][1:])
    en1, ems1, _ = agg(r["embed"][:1])
    en, ems, eper = agg(r["embed"][1:])
    r["steady"] = {
        "classify_per_item": round(cper, 1),
        "embed_per_item": round(eper, 1),
        "classify_warmup_batch_ms": cms1,
        "embed_warmup_batch_ms": ems1,
    }
    return (f"{r['tag']}: provider={r.get('provider')} | "
            f"classify 稳态 {cn}张 {cms:.0f}ms ({cper:.1f}ms/张) [预热批 {cms1:.0f}ms] | "
            f"embed 稳态 {en}张 {ems:.0f}ms ({eper:.1f}ms/张) [预热批 {ems1:.0f}ms]")


def main():
    global N_IMAGES, BATCH
    ap = argparse.ArgumentParser()
    ap.add_argument("--round", action="append", required=True,
                    help="格式 tag:mode:parallel，如 cpu-p6:cpu:6")
    ap.add_argument("--out", default="_tmp_bench_result.json")
    ap.add_argument("--images", type=int, default=N_IMAGES,
                    help=f"取样张数（默认 {N_IMAGES}，第 1 批当预热不计入稳态）")
    ap.add_argument("--batch", type=int, default=BATCH)
    args = ap.parse_args()
    N_IMAGES, BATCH = args.images, args.batch

    log_dir = ROOT / "python" / "bench" / "out" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    results = []
    for spec in args.round:
        tag, mode, par = spec.split(":")
        port = 8791 + len(results)
        try:
            r = run_round(tag, mode, int(par), port, log_dir)
        except Exception as e:
            r = {"tag": tag, "mode": mode, "status": f"HARNESS_ERROR: {e!r}"}
        results.append(r)
        print(">>>", summarize(r), flush=True)
        for ln in r.get("server_stats", [])[:12]:
            print("    |", ln[:400], flush=True)

    (ROOT / args.out).write_text(
        json.dumps(results, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n结果已写入 {args.out}", flush=True)


if __name__ == "__main__":
    sys.exit(main())
