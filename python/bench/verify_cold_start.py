# -*- coding: utf-8 -*-
"""验证：服务冷启动期间 /embed_text 是否可用（重启后首次语义搜索的关键路径）

模拟 Rust 侧新逻辑：ensure_service_ready(wait_models=false) 拉起服务后立即 POST /embed_text。
预期：/health ok=false（cls/det/scene 仍在加载）时 /embed_text 已能返回向量（CLIP 独立懒加载）。
"""
import json
import subprocess
import sys
import time
import urllib.request

sys.stdout.reconfigure(encoding="utf-8")
BASE = "http://127.0.0.1:8765"

proc = subprocess.Popen(
    [sys.executable, "server.py"],
    cwd="python",
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)
try:
    # 1. 等服务「可达」（不等模型就绪，等价 Rust poll_alive）
    t0 = time.time()
    h = None
    while time.time() - t0 < 25:
        try:
            h = json.load(urllib.request.urlopen(BASE + "/health", timeout=1))
            break
        except Exception:
            time.sleep(0.2)
    if h is None:
        print("[FAIL] 服务 25s 内不可达")
        sys.exit(1)
    print(f"服务可达：{time.time()-t0:.1f}s | ok={h['ok']} clip_ready={h['clip_ready']}（此刻主链路模型通常仍在加载）")

    # 2. 立即语义编码（CLIP 懒加载由本请求触发）
    body = json.dumps({"text": "海边日落"}).encode()
    t1 = time.time()
    req = urllib.request.Request(
        BASE + "/embed_text", data=body, headers={"Content-Type": "application/json"}
    )
    r = json.load(urllib.request.urlopen(req, timeout=120))
    dt = time.time() - t1
    print(f"/embed_text 成功：{dt:.1f}s | dim={len(r['embedding'])}")

    # 3. 之后状态
    h2 = json.load(urllib.request.urlopen(BASE + "/health", timeout=3))
    print(f"调用后：ok={h2['ok']} clip_ready={h2['clip_ready']}")

    ok = len(r["embedding"]) == 512
    print("[PASS] 冷启动期间语义编码可用（重启后首次搜索不再需要先做语义扫描）" if ok else "[FAIL] 维度异常")
    sys.exit(0 if ok else 1)
finally:
    proc.terminate()
    try:
        proc.wait(timeout=10)
    except Exception:
        proc.kill()
    print("测试服务已清理")
