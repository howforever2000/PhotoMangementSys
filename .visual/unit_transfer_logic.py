import os, sys, json
sys.path.insert(0, r"C:\Users\Admin\Documents\Projects\PhotoMangementSys\python")
from PIL import Image
import io

ROOT = r"C:\Users\Admin\Documents\Projects\PhotoMangementSys"
from ai_studio.schemas import JobRequest, Providers, ProviderConfig
from ai_studio.workflows import style_transfer
from ai_studio import presets

captured = {}
buf = io.BytesIO()
Image.new("RGB", (64, 48), (120, 90, 60)).save(buf, "JPEG")
fake_bytes = buf.getvalue()

def fake_edit(base, key, model, images, prompt, timeout=0, size=None):
    captured["n"] = len(images)
    captured["prompt"] = prompt
    return [fake_bytes]

style_transfer.oc.images_edit = fake_edit
style_transfer._describe_reference = lambda vis, img: "warm cinematic golden tones"

req = JobRequest(
    tool="style_transfer",
    paths=[os.path.join(ROOT, "covers", "login-sunset.jpg")],
    prompt="",
    outputDir=os.path.join(ROOT, ".visual", "unit_transfer_out"),
    options={"mode": "transfer", "referencePath": os.path.join(ROOT, "covers", "login-sunset.jpg")},
    providers=Providers(
        edit=ProviderConfig(baseUrl="http://dummy/v1", apiKey="sk-dummy", model="qwen-image-3.0"),
        vision=ProviderConfig(baseUrl="http://dummy/v1", apiKey="sk-dummy", model="qwen-vl"),
    ),
)
res = style_transfer.run(req, lambda *a: None, None)
print("n_images=", captured["n"])
print("prompt=", captured["prompt"])
assert captured["n"] == 1, "transfer 必须只送单图"
assert "Reference style: warm cinematic golden tones." in captured["prompt"], "缺少风格描述"
assert presets.STYLE_TRANSFER_PROMPT in captured["prompt"]
assert res["outputs"] and os.path.exists(res["outputs"][0])
print("LOGIC TEST PASS")
