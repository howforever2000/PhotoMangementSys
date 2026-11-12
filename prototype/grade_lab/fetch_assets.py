"""下载测试用 .cube 胶片 LUT（abpy/FujifilmCameraProfiles, CC-BY-NC-SA）并拷贝样图。

仅测试用；生产接入时按许可替换素材。
"""
import os
import shutil
import time
import urllib.parse
import urllib.request

BASE_URL = "https://raw.githubusercontent.com/abpy/FujifilmCameraProfiles/master/cube%20lut/"

# key -> 仓库内文件名（sRGB 版，供 sRGB 图片直接套用）
LUTS = {
    "provia": "provia_sRGB.cube",
    "velvia": "velvia_sRGB.cube",
    "astia": "astia_sRGB.cube",
    "classic_chrome": "classic chrome_sRGB.cube",
    "classic_neg": "classic neg_sRGB.cube",
    "eterna": "eterna_sRGB.cube",
    "nostalgic_neg": "nostalgic neg_sRGB.cube",
}

HERE = os.path.dirname(os.path.abspath(__file__))
LUTS_DIR = os.path.join(HERE, "luts")
SAMPLES_DIR = os.path.join(HERE, "samples")
PROJ = os.path.abspath(os.path.join(HERE, "..", ".."))

# 源图 -> 目标文件名
SAMPLE_COPIES = {
    "night_city.jpg": "night_city.jpg",
    "covers/login-sunset.jpg": "login_sunset.jpg",
    "covers/cover.jpg": "cover_teal.jpg",
}


def download(url: str, dst: str) -> bool:
    if os.path.exists(dst) and os.path.getsize(dst) > 1000:
        return True
    for attempt in range(4):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "grade-lab"})
            with urllib.request.urlopen(req, timeout=60) as resp:
                data = resp.read()
            if len(data) > 1000:
                with open(dst, "wb") as fh:
                    fh.write(data)
                return True
            print(f"  short body ({len(data)}) attempt {attempt + 1}: {url}")
        except Exception as exc:  # 网络抖动重试
            print(f"  retry {attempt + 1} failed: {type(exc).__name__}")
            time.sleep(1.5)
    return False


def main() -> None:
    os.makedirs(LUTS_DIR, exist_ok=True)
    os.makedirs(SAMPLES_DIR, exist_ok=True)
    # 先拷样图（不依赖网络）
    for src, name in SAMPLE_COPIES.items():
        s = os.path.join(PROJ, src)
        d = os.path.join(SAMPLES_DIR, name)
        if os.path.exists(s):
            if not os.path.exists(d):
                shutil.copyfile(s, d)
            print("sample", name, os.path.getsize(d))
        else:
            print("MISSING sample", s)
    failed = []
    for key, name in LUTS.items():
        dst = os.path.join(LUTS_DIR, name)
        ok = download(BASE_URL + urllib.parse.quote(name), dst)
        if ok:
            print("lut", key, os.path.getsize(dst))
        else:
            failed.append(key)
    if failed:
        print("FAILED LUTS:", failed)


if __name__ == "__main__":
    main()
