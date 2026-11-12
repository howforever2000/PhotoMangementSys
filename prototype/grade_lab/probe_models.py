import urllib.request

URLS = [
    "https://github.com/facefusion/facefusion-assets/releases/download/models-3.6.0/real_esrgan_x4.onnx",
    "https://github.com/facefusion/facefusion-assets/releases/download/models-3.6.0/gfpgan_1.4.onnx",
    "https://github.com/facefusion/facefusion-assets/releases/download/models/real_esrgan_x4.onnx",
    "https://github.com/facefusion/facefusion-assets/releases/download/models/gfpgan_1.4.onnx",
    "https://media.githubusercontent.com/media/onnx/models/main/validated/vision/super_resolution/sub_pixel_cnn_2016/model/super-resolution-10.onnx",
]

for u in URLS:
    try:
        req = urllib.request.Request(u, headers={"Range": "bytes=0-0", "User-Agent": "probe"})
        with urllib.request.urlopen(req, timeout=25) as r:
            cr = r.headers.get("Content-Range")
            cl = r.headers.get("Content-Length")
            print("OK", r.status, "len", cl, "range", cr, u)
    except Exception as e:
        print("ERR", type(e).__name__, str(e)[:120], u)
