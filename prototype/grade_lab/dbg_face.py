import os
import sys
import types

sys.modules.setdefault("requests", types.ModuleType("requests"))
sys.path.insert(0, os.path.abspath("python"))

from PIL import Image  # noqa: E402

from ai_studio import facedetect, localmodels  # noqa: E402

img = Image.open("prototype/grade_lab/samples/face_adele.png").convert("RGB")
print("size", img.size)
sess = localmodels.get_session("python/models/det_10g.onnx")
t, scale, px, py = facedetect._letterbox_tensor(img)
outs = sess.run(None, {sess.get_inputs()[0].name: t})
print("n outputs", len(outs), "shapes", [o.shape for o in outs])
mx = max(float(o.max()) for o in outs if o.ndim == 2 and o.shape[1] == 1)
print("max score", mx)
faces = facedetect.detect(sess, img)
print("faces(tight)", len(faces), [round(f["score"], 3) for f in faces])

pad = Image.new("RGB", (int(img.width * 1.8), int(img.height * 1.8)), (90, 90, 90))
pad.paste(img, ((pad.width - img.width) // 2, (pad.height - img.height) // 2))
faces2 = facedetect.detect(sess, pad)
print("faces(padded)", len(faces2), [round(f["score"], 3) for f in faces2])
