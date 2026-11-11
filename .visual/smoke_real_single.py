import os, sys, json, sqlite3, base64
sys.path.insert(0, r"C:\Users\Admin\Documents\Projects\PhotoMangementSys\python")
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

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
content = read_image(os.path.join(ROOT, "night_city.jpg"))
b = edit_input_bytes(content)
print("single input bytes", len(b), to_image(b).size)

data = oc.images_edit(base, KEY, model, [b],
                      "把这张城市夜景照片变成柔和的印象派水彩画风格，严格保持所有建筑、街道、车辆的构图与位置不变。")
p = os.path.join(OUT, "smoke_real_single.png")
open(p, "wb").write(data[0])
print("saved", p, to_image(data[0]).size, len(data[0]))
