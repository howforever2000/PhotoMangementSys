import json
import urllib.request

api = "https://api.github.com/repos/facefusion/facefusion-assets/releases?per_page=100"
req = urllib.request.Request(api, headers={"User-Agent": "x"})
rels = json.load(urllib.request.urlopen(req, timeout=60))
for r in rels:
    names = {a["name"] for a in r.get("assets", [])}
    if "real_esrgan_x4.onnx" in names or "gfpgan_1.4.onnx" in names:
        print(
            r["tag_name"],
            "ESRGAN" if "real_esrgan_x4.onnx" in names else "",
            "GFPGAN" if "gfpgan_1.4.onnx" in names else "",
        )
