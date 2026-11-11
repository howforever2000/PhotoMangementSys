import os, requests

BASE = "https://maas.qianwenaiapi.com/compatible-mode/v1"

def big(nbytes):
    return os.urandom(nbytes)

for size_mb in (2, 8):
    blob = big(size_mb * 1024 * 1024)
    try:
        r = requests.post(
            BASE + "/images/edits",
            headers={"Authorization": "Bearer sk-dummy-not-real"},
            files=[("image", ("big.png", blob, "image/png"))],
            data={"model": "qwen-image-3.0", "prompt": "test", "n": "1"},
            timeout=60,
        )
        print(f"[edits {size_mb}MB + dummy auth] HTTP {r.status_code} body={r.text[:120]!r}")
    except Exception as e:
        print(f"[edits {size_mb}MB + dummy auth] EXC {type(e).__name__}: {e}")
