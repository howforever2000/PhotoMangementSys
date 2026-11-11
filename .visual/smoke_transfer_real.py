import os, sys, json, sqlite3, base64
sys.path.insert(0, r"C:\Users\Admin\Documents\Projects\PhotoMangementSys\python")
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from PIL import Image

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
from ai_studio.workflows.base import edit_input_bytes, read_image, to_image

base = cfg["edit"]["base_url"]; model = cfg["edit"]["model"]
content_p = os.path.join(ROOT, "test_fixture_photos", "IMG_0001_camera.jpg")
ref_p = os.path.join(ROOT, "night_city.jpg")
content = read_image(content_p); ref = read_image(ref_p)
print("content", content.size, "ref", ref.size)

data = oc.images_edit(base, KEY, model,
                      [edit_input_bytes(content), edit_input_bytes(ref)],
                      "把第二张图的冷色调和夜景氛围迁移到第一张图上，严格保持第一张图的建筑、街道、车辆与构图不变。")
p = os.path.join(OUT, "smoke_transfer_real.png")
open(p, "wb").write(data[0])
print("saved", p, to_image(data[0]).size, len(data[0]))
