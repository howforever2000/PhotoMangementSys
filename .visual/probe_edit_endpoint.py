import io, os, sys
import requests
from PIL import Image

BASE = "https://maas.qianwenaiapi.com/compatible-mode/v1"

def tiny_png():
    b = io.BytesIO()
    Image.new("RGB", (16, 16), (200, 120, 80)).save(b, "PNG")
    return b.getvalue()

def call(desc, fn):
    try:
        r = fn()
        print(f"[{desc}] HTTP {r.status_code}  body={r.text[:200]!r}")
    except Exception as e:
        print(f"[{desc}] EXC {type(e).__name__}: {e}")

print("== 1) GET /models (no auth) ==")
call("models", lambda: requests.get(BASE + "/models", timeout=15))

print("== 2) POST /images/edits tiny (no auth) ==")
call("edits-tiny", lambda: requests.post(
    BASE + "/images/edits",
    files=[("image", ("a.png", tiny_png(), "image/png"))],
    data={"model": "qwen-image-3.0", "prompt": "test", "n": "1"},
    timeout=30))

print("== 3) POST /images/generations tiny json (no auth) ==")
call("generations", lambda: requests.post(
    BASE + "/images/generations",
    json={"model": "qwen-image-3.0", "prompt": "test", "n": 1},
    timeout=30))
