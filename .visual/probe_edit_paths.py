import io, requests
from PIL import Image

b = io.BytesIO(); Image.new("RGB", (16,16), (10,20,30)).save(b, "PNG"); tiny = b.getvalue()

paths = [
    "https://maas.qianwenaiapi.com/images/edits",
    "https://maas.qianwenaiapi.com/v1/images/edits",
    "https://maas.qianwenaiapi.com/compatible-mode/v1/images/edits",
    "https://maas.qianwenaiapi.com/compatible-mode/v1/images/edit",
    "https://maas.qianwenaiapi.com/compatible-mode/v1/images/variations",
]
for p in paths:
    try:
        r = requests.post(p, files=[("image", ("a.png", tiny, "image/png"))],
                          data={"model": "qwen-image-3.0", "prompt": "x", "n": "1"}, timeout=15)
        print(f"{p} -> HTTP {r.status_code} {r.text[:80]!r}")
    except Exception as e:
        print(f"{p} -> EXC {type(e).__name__}: {e}")
