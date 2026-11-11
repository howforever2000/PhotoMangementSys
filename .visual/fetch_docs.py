import requests, re, sys

urls = [
    "https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions",
    "https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-image-api",
    "https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-image-generation",
    "https://docs.bailian.console.aliyun.com/zh/model-studio/qwen-image",
]
for u in urls:
    try:
        r = requests.get(u, timeout=30, headers={"User-Agent": "Mozilla/5.0"})
        h = r.text
        print("=" * 80)
        print(u, "->", r.status_code, "len", len(h))
        for kw in ["modalities", "image_url", "qwen-image", "图像编辑", "\"image\"", "ratio"]:
            idx = h.find(kw)
            print(f"  kw {kw!r}: {'FOUND@'+str(idx) if idx>=0 else '-'}")
    except Exception as e:
        print(u, "EXC", e)
