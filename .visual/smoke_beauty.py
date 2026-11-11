import os, sys, json, sqlite3, base64, io
sys.path.insert(0, r"C:\Users\Admin\Documents\Projects\PhotoMangementSys\python")
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from PIL import Image, ImageDraw

ROOT = r"C:\Users\Admin\Documents\Projects\PhotoMangementSys"
OUT = os.path.join(ROOT, ".visual")

appdata = os.path.join(os.environ["APPDATA"], "com.photomanagementsys.app")
key = open(os.path.join(appdata, "app.key"), "rb").read()
con = sqlite3.connect("file:" + os.path.join(appdata, "photos.db") + "?mode=ro", uri=True)
cfg = json.loads(con.execute("select value from app_settings where key='ai_studio:1'").fetchone()[0])
con.close()
raw = base64.b64decode(cfg["edit"]["api_key_enc"][3:])
KEY = AESGCM(key).decrypt(raw[:12], raw[12:], None).decode()

# build a 1200x1600 portrait test photo (to verify downscale to 1024 long edge)
im = Image.new("RGB", (1200, 1600), (250, 245, 240))
d = ImageDraw.Draw(im)
for y in range(0, 1600, 128):
    for x in range(0, 1200, 128):
        if (x // 128 + y // 128) % 2 == 0:
            d.rectangle([x, y, x + 127, y + 127], fill=(40, 70, 170))
d.ellipse([300, 500, 900, 1150], fill=(235, 200, 180), outline=(0, 150, 70), width=20)
d.text((520, 780), "PHOTO", fill=(200, 30, 30))
inp = os.path.join(OUT, "smoke_input.jpg")
im.save(inp, "JPEG", quality=95)
print("input saved", im.size)

from ai_studio.workflows.base import edit_input_bytes
b = edit_input_bytes(im)
print("edit_input_bytes ->", len(b), "bytes, dims", Image.open(io.BytesIO(b)).size)

from ai_studio.schemas import JobRequest, Providers, ProviderConfig
from ai_studio.workflows import beauty

outdir = os.path.join(OUT, "smoke_out")
os.makedirs(outdir, exist_ok=True)
req = JobRequest(
    tool="beauty", paths=[inp], prompt="", outputDir=outdir,
    options={"style": "natural", "level": "medium"},
    providers=Providers(edit=ProviderConfig(baseUrl=cfg["edit"]["base_url"], apiKey=KEY, model=cfg["edit"]["model"])),
)
res = beauty.run(req, lambda *a: None, None)
print("beauty.run result:", json.dumps(res, ensure_ascii=False))
for p in res["outputs"]:
    print(" output:", p, Image.open(p).size, os.path.getsize(p), "bytes")
