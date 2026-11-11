import json
import requests

cands = [
    ("GET", "https://maas.qianwenaiapi.com/"),
    ("GET", "https://maas.qianwenaiapi.com/docs"),
    ("GET", "https://maas.qianwenaiapi.com/openapi.json"),
    ("GET", "https://maas.qianwenaiapi.com/compatible-mode/v1/openapi.json"),
    ("GET", "https://maas.qianwenaiapi.com/compatible-mode/v1/models"),
    ("POST", "https://maas.qianwenaiapi.com/compatible-mode/v1/chat/completions"),
]
for method, url in cands:
    try:
        if method == "GET":
            r = requests.get(url, timeout=15)
        else:
            r = requests.post(url, json={"model": "qwen-image-3.0", "messages": [{"role": "user", "content": "hi"}]}, timeout=15)
        ct = r.headers.get("content-type", "")
        print(f"{method} {url} -> HTTP {r.status_code} ct={ct} body={r.text[:200]!r}")
    except Exception as e:
        print(f"{method} {url} -> EXC {type(e).__name__}: {e}")
