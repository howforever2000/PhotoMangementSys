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

from ai_studio.providers import openai_compat as oc
from ai_studio.workflows.base import edit_input_bytes, to_image

base = cfg["edit"]["base_url"]; model = cfg["edit"]["model"]
print("model", model)

content = Image.new("RGB", (640, 480), (255, 255, 255))
d = ImageDraw.Draw(content)
d.rectangle([40, 40, 600, 440], outline=(10, 10, 10), width=8)
d.text((200, 220), "CONTENT", fill=(10, 10, 10))
ref = Image.new("RGB", (640, 480), (30, 30, 30))
dr = ImageDraw.Draw(ref)
for i in range(480):
    dr.line([(0, i), (640, i)], fill=(30 + i // 3, 60 + i // 3, 160))
dr.text((250, 220), "WARM REF", fill=(255, 230, 180))

try:
    data = oc.images_edit(base, KEY, model,
                          [edit_input_bytes(content), edit_input_bytes(ref)],
                          "把第二张图的暖色调迁移到第一张图上，保持第一张的构图不变")
    p = os.path.join(OUT, "smoke_multi.png")
    open(p, "wb").write(data[0])
    print("multi-image OK ->", p, to_image(data[0]).size, len(data[0]), "bytes")
except Exception as e:
    print("multi-image FAILED:", type(e).__name__, str(e)[:300])
