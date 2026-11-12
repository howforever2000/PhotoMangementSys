import onnxruntime as ort

for p in [
    "python/models/det_10g.onnx",
    "prototype/grade_lab/models/real_esrgan_x4.onnx",
    "prototype/grade_lab/models/gfpgan_1.4.onnx",
]:
    s = ort.InferenceSession(p, providers=["CPUExecutionProvider"])
    print("==", p)
    for i in s.get_inputs():
        print("  IN ", i.name, i.shape, i.type)
    for o in s.get_outputs():
        print("  OUT", o.name, o.shape, o.type)
