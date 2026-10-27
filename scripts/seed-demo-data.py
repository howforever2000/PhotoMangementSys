"""相册演示数据种子脚本（不跑 AI，全部预填充模仿数据）

用途
────
本地开发/验收时应用里一张照片都没有，每个页面都是空态，看不出布局与交互问题。
本脚本从网络取一批真实照片落到本机，并**按应用真实的数据口径**写入照片库与
人物库，让每个页面打开就有内容：

  相册管理 / 相册详情 / 时间线 / 智慧相册（分类·地点·人物）/ 智能搜索 /
  扫描中心（人物注册表）/ 手动排序 / 创意工坊

「不跑 AI」是怎么做到的
──────────────────────
AI 三件套（CLIP 图像向量、ArcFace 人脸嵌入、CLIP 语义分类）都不跑，改为按
真实字段口径**直接写库**：

  · photo_content_scan  —— 分类/label/置信度/top3/人物/EXIF 逐张手写
  · photo_embeddings    —— 512 维伪向量（按类目聚类 + 确定性扰动），
                           让「以图搜图 / 语义搜索」结果看起来合理
  · persons.db          —— persons/faces 直写；bbox 用 OpenCV Haar **真实检测**
                           （比手写假框更自然，头像裁剪也不会裁到背景）
  · photo_category_hits —— 按分类命中物化（count/封面与真实浏览路径一致）
  · thumbs/grid + avatars —— 缩略图与头像缓存按后端命名规则预生成

与真实链路的一致性（关键，否则页面显示的数字会对不上）
──────────────────────────────────────────────
  · photo_hash = FNV-1a 64(`len|mtime_ns|abs_path`)  —— 与
    `src-tauri/src/content.rs::photo_hash` / `thumbnail.rs::thumb_photo_hash` 同算法
  · 网格缩略图名 = `album_{id}_photo_v2_{len}_{mtime_s}_{fnv1a64(头8KB)}.webp`
    （`thumbnail.rs::file_fingerprint` + `grid_thumb_cache_name`）
  · 头像签名 = `脸数:最大face id`（`persons.rs::face_sig_of`），不写会被判失配重裁
  · 内容分类 slug 与 `db/category.rs::BUILTIN_CATEGORIES` 一致

用法
────
    python3 scripts/seed-demo-data.py            # 幂等：先清旧演示数据再写
    python3 scripts/seed-demo-data.py --no-download   # 不下载，只用已有图片
    python3 scripts/seed-demo-data.py --dry-run       # 只打印计划，不写任何东西

依赖（用 uv 临时环境自装，不写入项目依赖）：
    pillow piexif opencv-python-headless numpy

约束：不改业务代码、不改 schema；只写演示相册（路径前缀 `~/Pictures/album-demo`）。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import shutil
import sqlite3
import struct
import subprocess
import sys
import time
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

# ---------------------------------------------------------------------------
# 常量：与后端严格对齐（改这里必须同步改 Rust 侧，见各注释）
# ---------------------------------------------------------------------------

FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3
U64_MASK = 0xFFFFFFFFFFFFFFFF

THUMB_RENDER_VERSION = 2  # thumbnail.rs::THUMB_RENDER_VERSION
THUMB_SIZE = 256  # thumbnail.rs::THUMB_SIZE
EMB_DIM = 512  # Chinese-CLIP b16 图像塔维度

CLIP_MODEL_ID = "chinese-clip-vit-b16-fp16"  # vision.rs 当前档位
DESC_KIND = "desc"  # textdesc.rs::DESC_KIND

ALBUM_ROOT = Path.home() / "Pictures" / "album-demo"

IMAGE_EXTS = (".jpg", ".jpeg", ".png", ".webp", ".gif", ".bmp")


def fnv1a64(data: bytes) -> int:
    h = FNV_OFFSET
    for b in data:
        h ^= b
        h = (h * FNV_PRIME) & U64_MASK
    return h


def fnv1a64_str(text: str) -> int:
    return fnv1a64(text.encode("utf-8"))


def photo_hash(path: Path) -> str:
    """与 content.rs::photo_hash / thumbnail.rs::thumb_photo_hash 同算法。"""
    st = path.stat()
    mtime_ns = st.st_mtime_ns
    return f"{fnv1a64_str(f'{st.st_size}|{mtime_ns}|{str(path.resolve())}'):016x}"


def file_fingerprint(path: Path) -> str:
    """与 thumbnail.rs::file_fingerprint 同算法：v{ver}_{len}_{mtime_s}_{fnv1a64(头8KB)}。"""
    st = path.stat()
    mtime_s = st.st_mtime_ns // 1_000_000_000
    head = path.open("rb").read(8192)
    return f"v{THUMB_RENDER_VERSION}_{st.st_size}_{mtime_s}_{fnv1a64(head)}"


def grid_thumb_name(album_id: int, path: Path) -> str:
    """网格缩略图磁盘名 —— **扁平 .jpg**，与 `ensure_grid_thumb` 实际写入一致。

    注意：`grid_thumb_cache_name`（Rust）返回的是嵌套 `.webp` 命名，那是旧命名；
    真正被 `get_photo_thumbs` / 人物照片链路落地的是扁平 `.jpg`（内容是 WebP 或
    JPEG 都无所谓，浏览器按内容嗅探）。这里按**实际生效**的扁平命名生成，
    同时另存一份嵌套 `.webp`（`grid_thumb_cache_name` 会先查它，命中可省一次生成）。
    """
    return f"album_{album_id}_photo_{file_fingerprint(path)}.jpg"


def grid_thumb_name_legacy(album_id: int, path: Path) -> str:
    """嵌套 `.webp` 命名 —— 与 `thumbnail.rs::grid_thumb_cache_name` 一致。"""
    fp = file_fingerprint(path)
    return f"album_{album_id}_photo_{fp[:2]}/{fp}.webp"


def cover_thumb_name(album_id: int, path: Path) -> str:
    """与 thumbnail.rs::ensure_thumbnail_from_source 同算法（封面用）。"""
    fp = file_fingerprint(path)
    return f"album_{album_id}_auto_{fp[:2]}/{fp}.webp"


# ---------------------------------------------------------------------------
# 数据模型
# ---------------------------------------------------------------------------


@dataclass
class Photo:
    """一张演示照片的全部「AI 模仿元数据」。"""

    url: str
    album: str
    name: str
    category: str  # AI 大类 key（9 大组或中间 key）
    sub: str | None
    label: str  # ImageNet 风格细类名
    conf: float
    persons: list[str] = field(default_factory=list)
    # EXIF（写进 JPEG 的 EXIF 段，并同步到 DB 行）
    shoot: str = "2025-01-01 12:00:00"  # YYYY-MM-DD HH:MM:SS
    iso: int = 200
    aperture: float = 2.8
    shutter: float = 1 / 200
    focal: int = 35
    lat: float | None = None
    lon: float | None = None
    location: str | None = None  # 反查地名（无 GPS 时也要有，地点页才有组）
    tone: str | None = None  # low-key / mid-key / high-key
    luma: float | None = None
    rating: int = 0
    tags: list[str] = field(default_factory=list)


# 类目 key → 伪向量的「语义中心」（确定性随机种子保证可复现）
CATEGORY_KEYS = [
    "portrait",
    "street",
    "animal",
    "food",
    "flower",
    "landscape",
    "cityscape",
    "night_scene",
    "document",
]


def category_vector(cat: str, salt: str) -> list[float]:
    """按类目生成 512 维伪向量：同簇高相似、异簇低相似。

    做法：每类一个确定性随机基向量，叠加照片自身的小扰动后归一化。
    同类内余弦 ≈0.95+，异类 ≈0（正好落在 0.30 检索阈值两侧）。
    """
    rng = random.Random(f"{cat}|{salt}")
    base_rng = random.Random(f"base|{cat}")
    base = [base_rng.gauss(0, 1) for _ in range(EMB_DIM)]
    vec = [b + rng.gauss(0, 0.08) for b in base]
    norm = sum(v * v for v in vec) ** 0.5
    return [v / norm for v in vec]


# ---------------------------------------------------------------------------
# 演示相册与照片清单
# ---------------------------------------------------------------------------
# 类目分布刻意覆盖：人物 / 动物 / 食物 / 花卉 / 风景 / 城市 / 夜景 / 文档，
# 让人物页、分类页、地点页、夜景规则分类都有内容。
# 地点 GPS 覆盖国内多个省份，地点地图才有打点。

ALBUMS: list[dict] = [
    {
        "name": "2025 · 秋日旅行",
        "dir": "travel",
        "desc": "十月在川西与滇西北转了一圈",
        "location": "四川省",
        "tags": ["旅行", "秋天"],
        "photos": [
            Photo("https://images.unsplash.com/photo-1506905925346-21bda4d32df4?w=900&q=80",
                  "travel", "雪山日出.jpg", "landscape", "雪山", "snow mountain", 0.91,
                  shoot="2025-10-02 06:41:00", iso=100, aperture=8.0, shutter=1/500, focal=70,
                  lat=30.0836, lon=102.9331, location="四川省 · 甘孜藏族自治州", tone="mid-key", luma=118.4, rating=5, tags=["日出"]),
            Photo("https://images.unsplash.com/photo-1441974231531-c6227db76b6e?w=900&q=80",
                  "travel", "林间光束.jpg", "landscape", "森林", "forest", 0.87,
                  shoot="2025-10-02 11:20:00", iso=200, aperture=5.6, shutter=1/160, focal=35,
                  lat=30.0836, lon=102.9331, location="四川省 · 甘孜藏族自治州", tone="mid-key", luma=96.2),
            Photo("https://images.unsplash.com/photo-1470071459604-3b5ec3a7fe05?w=900&q=80",
                  "travel", "晨雾山谷.jpg", "landscape", "山峦", "valley", 0.84,
                  shoot="2025-10-03 07:05:00", iso=100, aperture=11.0, shutter=1/250, focal=50,
                  lat=27.9836, lon=100.2031, location="云南省 · 丽江市", tone="high-key", luma=168.9, rating=4),
            Photo("https://images.unsplash.com/photo-1469474968028-56623f02e42e?w=900&q=80",
                  "travel", "山巅日落.jpg", "landscape", "山峦", "mountain", 0.89,
                  shoot="2025-10-03 18:33:00", iso=100, aperture=8.0, shutter=1/400, focal=135,
                  lat=27.9836, lon=100.2031, location="云南省 · 丽江市", tone="mid-key", luma=104.7),
            Photo("https://images.unsplash.com/photo-1472214103451-9374bd1c798e?w=900&q=80",
                  "travel", "旷野长风.jpg", "landscape", "草原", "field", 0.79,
                  shoot="2025-10-04 09:12:00", iso=200, aperture=4.0, shutter=1/800, focal=28,
                  lat=30.5836, lon=99.2331, location="四川省 · 甘孜藏族自治州", tone="high-key", luma=172.1),
            Photo("https://images.unsplash.com/photo-1501785888041-af3ef285b470?w=900&q=80",
                  "travel", "湖畔黄昏.jpg", "landscape", "湖泊", "lakeside", 0.86,
                  shoot="2025-10-04 17:52:00", iso=400, aperture=2.8, shutter=1/125, focal=24,
                  lat=27.8836, lon=100.2031, location="云南省 · 丽江市", tone="mid-key", luma=88.3, rating=3),
        ],
    },
    {
        "name": "城市漫步",
        "dir": "city",
        "desc": "扫街与城市天际线",
        "location": "上海市",
        "tags": ["城市", "扫街"],
        "photos": [
            Photo("https://images.unsplash.com/photo-1449824913935-59a10b8d2000?w=900&q=80",
                  "city", "黄昏街道.jpg", "street", "街道", "street", 0.82,
                  shoot="2024-03-15 17:48:00", iso=800, aperture=2.0, shutter=1/60, focal=35,
                  lat=31.2304, lon=121.4737, location="上海市 · 上海市", tone="mid-key", luma=76.5),
            Photo("https://images.unsplash.com/photo-1487958449943-2429e8be8625?w=900&q=80",
                  "city", "白墙建筑.jpg", "architecture", "建筑", "building", 0.78,
                  shoot="2024-03-15 10:05:00", iso=100, aperture=8.0, shutter=1/320, focal=24,
                  lat=31.2304, lon=121.4737, location="上海市 · 上海市", tone="high-key", luma=181.2),
            Photo("https://images.unsplash.com/photo-1519501025264-65ba15a82390?w=900&q=80",
                  "city", "夜色都市.jpg", "night_scene", None, "night city", 0.88,
                  shoot="2024-03-16 20:31:00", iso=1600, aperture=1.8, shutter=1/30, focal=50,
                  lat=31.2397, lon=121.4998, location="上海市 · 上海市", tone="low-key", luma=28.6, rating=5, tags=["外滩"]),
            Photo("https://images.unsplash.com/photo-1493397212122-2b85dda8db8d?w=900&q=80",
                  "city", "几何立面.jpg", "architecture", "建筑", "facade", 0.74,
                  shoot="2024-03-16 14:22:00", iso=200, aperture=5.6, shutter=1/250, focal=35,
                  lat=31.2304, lon=121.4737, location="上海市 · 上海市", tone="high-key", luma=165.0),
            Photo("https://images.unsplash.com/photo-1477959858617-67f85cf4f1df?w=900&q=80",
                  "city", "天际线.jpg", "cityscape", None, "skyline", 0.83,
                  shoot="2024-03-17 06:15:00", iso=100, aperture=8.0, shutter=1/500, focal=85,
                  lat=31.2397, lon=121.4998, location="上海市 · 上海市", tone="mid-key", luma=112.9, rating=4),
            Photo("https://images.unsplash.com/photo-1514565131-fce0801e5785?w=900&q=80",
                  "city", "夜街霓虹.jpg", "night_scene", None, "neon street", 0.81,
                  shoot="2024-03-17 21:08:00", iso=3200, aperture=1.8, shutter=1/25, focal=35,
                  lat=31.2200, lon=121.4500, location="上海市 · 上海市", tone="low-key", luma=34.1),
        ],
    },
    {
        "name": "家人与朋友",
        "dir": "people",
        "desc": "人物合影（人脸为 Haar 检测 + 预填编号，非 AI 识别）",
        "location": "北京市",
        "tags": ["家人", "合影"],
        "photos": [
            Photo("https://images.unsplash.com/photo-1544005313-94ddf0286df2?w=900&q=80",
                  "people", "肖像-阿雅.jpg", "portrait", None, "portrait", 0.93,
                  persons=["P001"],
                  shoot="2026-05-10 15:02:00", iso=200, aperture=1.8, shutter=1/250, focal=85,
                  lat=39.9042, lon=116.4074, location="北京市 · 北京市", tone="mid-key", luma=121.5, rating=5, tags=["阿雅"]),
            Photo("https://images.unsplash.com/photo-1494790108377-be9c29b29330?w=900&q=80",
                  "people", "肖像-小满.jpg", "portrait", None, "portrait", 0.90,
                  persons=["P002"],
                  shoot="2026-05-10 15:26:00", iso=200, aperture=2.2, shutter=1/200, focal=85,
                  lat=39.9042, lon=116.4074, location="北京市 · 北京市", tone="mid-key", luma=132.0, tags=["小满"]),
            Photo("https://images.unsplash.com/photo-1507003211169-0a1dd7228f2d?w=900&q=80",
                  "people", "双人合影.jpg", "portrait", None, "group photo", 0.85,
                  persons=["P004", "P005"],
                  shoot="2026-05-11 11:14:00", iso=400, aperture=2.8, shutter=1/160, focal=50,
                  lat=39.9142, lon=116.4074, location="北京市 · 北京市", tone="mid-key", luma=118.8),
            Photo("https://images.unsplash.com/photo-1524504388940-b1c1722653e1?w=900&q=80",
                  "people", "逆光人像.jpg", "portrait", None, "backlit portrait", 0.87,
                  persons=["P003"],
                  shoot="2026-05-11 16:40:00", iso=100, aperture=1.8, shutter=1/1250, focal=85,
                  lat=39.9042, lon=116.4074, location="北京市 · 北京市", tone="low-key", luma=62.4),
            Photo("https://images.unsplash.com/photo-1517841905240-472988babdf9?w=900&q=80",
                  "people", "窗边侧影.jpg", "portrait", None, "profile", 0.82,
                  persons=["P002"],
                  shoot="2026-05-12 09:33:00", iso=400, aperture=2.0, shutter=1/100, focal=85,
                  lat=39.9042, lon=116.4074, location="北京市 · 北京市", tone="mid-key", luma=98.7),
            Photo("https://images.unsplash.com/photo-1506794778202-cad84cf45f1d?w=900&q=80",
                  "people", "街头肖像.jpg", "portrait", None, "street portrait", 0.80,
                  persons=["P001"],
                  shoot="2026-05-12 17:21:00", iso=200, aperture=1.8, shutter=1/320, focal=85,
                  lat=39.9242, lon=116.4174, location="北京市 · 北京市", tone="mid-key", luma=105.3),
        ],
    },
    {
        "name": "餐桌日常",
        "dir": "food",
        "desc": "周末做饭记录",
        "location": "广东省",
        "tags": ["美食"],
        "photos": [
            Photo("https://images.unsplash.com/photo-1504674900247-0877df9cc836?w=900&q=80",
                  "food", "早餐拼盘.jpg", "food", None, "breakfast plate", 0.88,
                  shoot="2024-01-18 08:42:00", iso=800, aperture=2.8, shutter=1/60, focal=35,
                  lat=23.1291, lon=113.2644, location="广东省 · 广州市", tone="high-key", luma=158.6, rating=4),
            Photo("https://images.unsplash.com/photo-1512621776951-a57141f2eefd?w=900&q=80",
                  "food", "沙拉碗.jpg", "food", None, "salad", 0.85,
                  shoot="2024-01-18 12:15:00", iso=400, aperture=2.8, shutter=1/125, focal=50,
                  lat=23.1291, lon=113.2644, location="广东省 · 广州市", tone="high-key", luma=171.3),
            Photo("https://images.unsplash.com/photo-1414235077428-338989a2e8c0?w=900&q=80",
                  "food", "餐厅晚餐.jpg", "food", None, "restaurant table", 0.83,
                  shoot="2024-01-19 19:05:00", iso=1600, aperture=1.8, shutter=1/40, focal=35,
                  lat=23.1291, lon=113.2644, location="广东省 · 广州市", tone="low-key", luma=52.9),
            Photo("https://images.unsplash.com/photo-1495474472287-4d71bcdd2085?w=900&q=80",
                  "food", "手冲咖啡.jpg", "food", None, "coffee", 0.80,
                  shoot="2024-01-20 09:10:00", iso=800, aperture=2.0, shutter=1/80, focal=50,
                  lat=23.1391, lon=113.2644, location="广东省 · 广州市", tone="mid-key", luma=96.4, rating=3),
        ],
    },
    {
        "name": "花与绿植",
        "dir": "flower",
        "desc": "阳台与植物园",
        "location": "浙江省",
        "tags": ["花卉"],
        "photos": [
            Photo("https://images.unsplash.com/photo-1490750967868-88aa4486c946?w=900&q=80",
                  "flower", "春日花田.jpg", "flower", None, "flower field", 0.90,
                  shoot="2026-04-06 10:44:00", iso=100, aperture=4.0, shutter=1/640, focal=85,
                  lat=30.2741, lon=120.1551, location="浙江省 · 杭州市", tone="high-key", luma=176.8),
            Photo("https://images.unsplash.com/photo-1416879595882-3373a0480b5b?w=900&q=80",
                  "flower", "微距花朵.jpg", "flower", None, "macro flower", 0.86,
                  shoot="2026-04-06 14:20:00", iso=200, aperture=2.8, shutter=1/500, focal=100,
                  lat=30.2741, lon=120.1551, location="浙江省 · 杭州市", tone="mid-key", luma=128.5, rating=5),
            Photo("https://images.unsplash.com/photo-1462536943532-57a629f6cc60?w=900&q=80",
                  "flower", "阳台绿植.jpg", "plant_flower", "植物", "houseplant", 0.77,
                  shoot="2026-04-07 08:05:00", iso=400, aperture=2.8, shutter=1/100, focal=35,
                  lat=30.2841, lon=120.1651, location="浙江省 · 杭州市", tone="mid-key", luma=110.2),
            Photo("https://images.unsplash.com/photo-1487530811176-3780de880c2d?w=900&q=80",
                  "flower", "花苞特写.jpg", "flower", None, "flower bud", 0.81,
                  shoot="2026-04-07 16:38:00", iso=200, aperture=2.8, shutter=1/320, focal=100,
                  lat=30.2741, lon=120.1551, location="浙江省 · 杭州市", tone="high-key", luma=168.0),
        ],
    },
    {
        "name": "海边的假期",
        "dir": "sea",
        "desc": "三亚，住了四天",
        "location": "海南省",
        "tags": ["海", "度假"],
        "photos": [
            Photo("https://images.unsplash.com/photo-1507525428034-b723cf961d3e?w=900&q=80",
                  "sea", "海滩.jpg", "landscape", "海滨", "beach", 0.88,
                  shoot="2023-07-21 09:55:00", iso=100, aperture=11.0, shutter=1/800, focal=24,
                  lat=18.2528, lon=109.5119, location="海南省 · 三亚市", tone="high-key", luma=184.6, rating=5),
            Photo("https://images.unsplash.com/photo-1502680390469-be75c86b636f?w=900&q=80",
                  "sea", "冲浪.jpg", "sports", None, "surfing", 0.75,
                  shoot="2023-07-21 15:12:00", iso=400, aperture=4.0, shutter=1/1000, focal=200,
                  lat=18.2528, lon=109.5119, location="海南省 · 三亚市", tone="mid-key", luma=122.4),
            Photo("https://images.unsplash.com/photo-1519046904884-53103b34b206?w=900&q=80",
                  "sea", "海边日落.jpg", "night_scene", None, "seaside sunset", 0.84,
                  shoot="2023-07-22 18:47:00", iso=200, aperture=8.0, shutter=1/250, focal=70,
                  lat=18.2628, lon=109.5219, location="海南省 · 三亚市", tone="mid-key", luma=92.1, rating=4),
            Photo("https://images.unsplash.com/photo-1473116763249-2f63ef631653?w=900&q=80",
                  "sea", "栈桥.jpg", "landscape", "海滨", "pier", 0.79,
                  shoot="2023-07-23 07:20:00", iso=100, aperture=8.0, shutter=1/500, focal=35,
                  lat=18.2528, lon=109.5119, location="海南省 · 三亚市", tone="high-key", luma=162.3),
        ],
    },
    {
        "name": "猫狗日常",
        "dir": "pets",
        "desc": "两只猫一条狗",
        "location": "四川省",
        "tags": ["宠物"],
        "photos": [
            Photo("https://images.unsplash.com/photo-1514888286974-6c03e2ca1dba?w=900&q=80",
                  "pets", "橘猫.jpg", "animal", "猫", "tabby cat", 0.92,
                  shoot="2025-02-14 19:03:00", iso=1600, aperture=1.8, shutter=1/60, focal=50,
                  lat=30.6572, lon=104.0665, location="四川省 · 成都市", tone="low-key", luma=44.7, rating=5, tags=["橘子"]),
            Photo("https://images.unsplash.com/photo-1518791841217-8f162f1e1131?w=900&q=80",
                  "pets", "窗台上的猫.jpg", "animal", "猫", "cat on window", 0.89,
                  shoot="2025-02-15 10:28:00", iso=800, aperture=2.0, shutter=1/125, focal=50,
                  lat=30.6572, lon=104.0665, location="四川省 · 成都市", tone="mid-key", luma=126.9),
            Photo("https://images.unsplash.com/photo-1552053831-71594a27632d?w=900&q=80",
                  "pets", "金毛.jpg", "animal", "狗", "golden retriever", 0.90,
                  shoot="2025-02-16 14:50:00", iso=400, aperture=2.8, shutter=1/400, focal=85,
                  lat=30.6672, lon=104.0765, location="四川省 · 成都市", tone="mid-key", luma=131.2),
            Photo("https://images.unsplash.com/photo-1425082661705-1834bfd09dca?w=900&q=80",
                  "pets", "小狗散步.jpg", "animal", "狗", "dog walking", 0.82,
                  shoot="2025-02-17 08:15:00", iso=200, aperture=4.0, shutter=1/640, focal=85,
                  lat=30.6572, lon=104.0665, location="四川省 · 成都市", tone="high-key", luma=148.5),
        ],
    },
    {
        "name": "截图与文档",
        "dir": "docs",
        "desc": "界面截图与资料翻拍",
        "location": None,
        "tags": [],
        "photos": [
            Photo("https://images.unsplash.com/photo-1498050108023-c5249f4df085?w=900&q=80",
                  "docs", "代码屏幕.jpg", "document", None, "screen", 0.86,
                  shoot="2023-08-11 22:41:00", iso=800, aperture=1.8, shutter=1/60, focal=35,
                  tone="low-key", luma=31.8, rating=3, tags=["截图"]),
            Photo("https://images.unsplash.com/photo-1531297484001-80022131f5a1?w=900&q=80",
                  "docs", "桌面俯拍.jpg", "document", None, "desk", 0.78,
                  shoot="2023-08-12 10:02:00", iso=400, aperture=2.8, shutter=1/125, focal=24,
                  tone="high-key", luma=174.2),
            Photo("https://images.unsplash.com/photo-1486312338219-ce68e2c6b44d?w=900&q=80",
                  "docs", "笔记本.jpg", "document", None, "laptop", 0.81,
                  shoot="2023-08-12 14:36:00", iso=800, aperture=2.0, shutter=1/80, focal=35,
                  tone="mid-key", luma=86.4),
        ],
    },
]

# 手动排序分组：folder → 相册目录名
FOLDERS = [
    {"name": "2025 旅行", "desc": "全年出门拍的", "children": ["travel", "sea"]},
    {"name": "生活", "desc": "日常与人物", "children": ["people", "food", "pets", "flower"]},
]

# 人物真名（ persons.name 用编号会显示 P001，起中文名更像真实库）
PERSON_NAMES = {
    "P001": "阿雅",
    "P002": "小满",
    "P003": "阿哲",
    "P004": "妈妈",
    "P005": "爸爸",
}


# ---------------------------------------------------------------------------
# 网络下载
# ---------------------------------------------------------------------------

UA = "Mozilla/5.0 (X11; Linux x86_64) PhotoManagementSys-DemoSeeder/1.0"


def download(url: str, dest: Path, retries: int = 3, fallback_seed: int | None = None) -> None:
    """下载图片；失败（网络不可达 / 404 / 响应过小）时用 picsum 确定性图兜底。"""
    if dest.is_file() and dest.stat().st_size > 10_000:
        return
    tmp = dest.with_suffix(".part")
    last = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60) as r, tmp.open("wb") as f:
                shutil.copyfileobj(r, f)
            if tmp.stat().st_size > 10_000:
                tmp.replace(dest)
                return
            last = RuntimeError(f"响应体过小（{tmp.stat().st_size} 字节，可能是 404 页）")
        except urllib.error.HTTPError as e:
            # 404 / 410：图片 id 拼错或已下架，重试无意义，直接走兜底
            last = e
            break
        except Exception as e:  # noqa: BLE001 —— 网络抖动要重试
            last = e
            time.sleep(1.5 * (attempt + 1))
    # 兜底：picsum 按 seed 给一张稳定的图（跟 302 跳转到 fastly CDN）
    if fallback_seed is not None:
        try:
            alt = f"https://picsum.photos/seed/{fallback_seed}/900/600"
            req = urllib.request.Request(alt, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60) as r, tmp.open("wb") as f:
                shutil.copyfileobj(r, f)
            if tmp.stat().st_size > 10_000:
                tmp.replace(dest)
                print(f"    ⚠ 原图不可达，改用兜底图：{alt}")
                return
            last = RuntimeError(f"兜底图也过小（{tmp.stat().st_size} 字节）")
        except Exception as e2:  # noqa: BLE001
            last = e2
    raise RuntimeError(f"下载失败 {url}: {last}")


# ---------------------------------------------------------------------------
# EXIF / 缩略图 / 头像生成（需要 pillow + piexif + opencv）
# ---------------------------------------------------------------------------

def deps() -> None:
    """确保 pillow / piexif / opencv-python-headless / numpy 可用。

    本机系统 Python 可能没有 pip（Debian/Deepin 默认如此），因此优先用 `uv run`
    的临时环境（uv 已在本机 PATH 中）；都不可用时给出明确提示。
    依赖不写进项目依赖：本脚本是开发期工具，不进 release。

    防递归：透传 PMS_SEED_DEPS=1 环境变量，重跑后若仍缺依赖就直接报错，
    不再 exec 第二次（否则 uv 递归上限会先把进程打死）。
    """
    try:
        import PIL  # noqa: F401
        import piexif  # noqa: F401
        import cv2  # noqa: F401
        return
    except ImportError:
        pass

    uv = shutil.which("uv")
    if uv and os.environ.get("PMS_SEED_DEPS") != "1":
        env = dict(os.environ, PMS_SEED_DEPS="1")
        os.execve(uv, [
            # opencv 必须锁 4.x：5.0 起 CascadeClassifier 已从默认包移除
            uv, "run", "--no-project", "--with", "pillow", "--with", "piexif",
            "--with", "opencv-python-headless==4.10.0.84", "--with", "numpy",
            "--", "python3", __file__, *sys.argv[1:],
        ], env)
    try:
        subprocess.run(
            [sys.executable, "-m", "pip", "install", "--quiet",
             "pillow", "piexif", "opencv-python-headless==4.10.0.84", "numpy"],
            check=True,
        )
        return
    except Exception as e:  # noqa: BLE001
        raise SystemExit(
            "缺少依赖且无法自动安装。请手动执行其一：\n"
            "  uv run --with pillow --with piexif --with opencv-python-headless "
            "--with numpy python3 scripts/seed-demo-data.py\n"
            f"  （内部错误：{e}）"
        )


def rational(value: float) -> tuple[int, int]:
    """把一个浮点数转成 EXIF 有理数（分母用 10 的幂，保证精度足够）。"""
    if value <= 0:
        return (0, 1)
    denom = 1_000_000
    n = int(round(value * denom))
    while n % 10 == 0 and denom > 1:
        n //= 10
        denom //= 10
    return (n, denom)


def dms(value: float) -> tuple[tuple[int, int], ...]:
    """十进制度 → EXIF 度分秒三元组。"""
    value = abs(value)
    d = int(value)
    m_float = (value - d) * 60
    m = int(m_float)
    s = (m_float - m) * 60
    denom = 1000
    s_num = int(round(s * denom))
    return ((d, 1), (m, 1), (s_num, denom))


def write_exif(dest: Path, p: Photo) -> None:
    """按真实相册的字段口径写 EXIF（拍摄时间/曝光/光圈/ISO/焦段/GPS）。"""
    import piexif
    from PIL import Image

    Image.MAX_IMAGE_PIXELS = None
    img = Image.open(dest)
    exif: dict = {
        "0th": {
            piexif.ImageIFD.Make: b"DemoCam",
            piexif.ImageIFD.Model: b"Seed X100",
            piexif.ImageIFD.Orientation: 1,
            piexif.ImageIFD.Software: b"PhotoMangementSys demo seeder",
        },
        "Exif": {
            piexif.ExifIFD.DateTimeOriginal: p.shoot.replace("-", ":").encode(),
            piexif.ExifIFD.ExposureTime: rational(p.shutter),
            piexif.ExifIFD.FNumber: rational(p.aperture),
            piexif.ExifIFD.ISOSpeed: int(p.iso),
            piexif.ExifIFD.FocalLength: rational(p.focal),
            piexif.ExifIFD.LensModel: b"Demo 24-70mm F2.8",
        },
        "GPS": {},
        "1st": {},
        "thumbnail": None,
    }
    if p.lat is not None and p.lon is not None:
        exif["GPS"] = {
            piexif.GPSIFD.GPSLatitudeRef: b"N" if p.lat >= 0 else b"S",
            piexif.GPSIFD.GPSLatitude: dms(p.lat),
            piexif.GPSIFD.GPSLongitudeRef: b"E" if p.lon >= 0 else b"W",
            piexif.GPSIFD.GPSLongitude: dms(p.lon),
        }
    # 统一转成 RGB 再存，避免调色板/灰度 JPEG 带 alpha 导致后端解码分支差异
    if img.mode not in ("RGB",):
        img = img.convert("RGB")
    img.save(dest, "JPEG", quality=88, exif=piexif.dump(exif), optimize=True)
    img.close()


def make_thumb(src: Path, out: Path, size: int = THUMB_SIZE) -> None:
    """生成缩略图（长边 size）。

    格式按**目标扩展名**决定，不写死：
      · `.jpg` → 真 JPEG（网格缩略图的磁盘名就是 .jpg，内容必须是 JPEG；
        写成 WebP 会让 WebKit 按 Content-Type=image/jpeg 解码失败 → 网格全是裂图）
      · `.webp` → WebP（嵌套命名那套，`grid_thumb_cache_name` 也用这个后缀）
    """
    from PIL import Image

    Image.MAX_IMAGE_PIXELS = None
    img = Image.open(src)
    if img.mode not in ("RGB",):
        img = img.convert("RGB")
    img.thumbnail((size, size), Image.LANCZOS)
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.suffix.lower() == ".webp":
        img.save(out, "WebP", quality=85, method=4)
    else:
        img.save(out, "JPEG", quality=86, optimize=True)
    img.close()


def make_avatar(src: Path, bbox: tuple[int, int, int, int], out: Path) -> None:
    """按人脸框外扩 12% 裁剪 96×96 JPEG —— 与 persons.rs::crop_avatar_local 同规格。"""
    from PIL import Image

    Image.MAX_IMAGE_PIXELS = None
    img = Image.open(src).convert("RGB")
    w, h = img.size
    x1, y1, x2, y2 = bbox
    bw, bh = x2 - x1, y2 - y1
    dx = int(bw * 0.12)
    dy = int(bh * 0.12)
    cx = max(0, x1 - dx)
    cy = max(0, y1 - dy)
    cw = min(bw + 2 * dx, w - cx)
    ch = min(bh + 2 * dy, h - cy)
    crop = img.crop((cx, cy, cx + cw, cy + ch))
    avatar = crop.resize((96, 96), Image.LANCZOS)
    out.parent.mkdir(parents=True, exist_ok=True)
    avatar.save(out, "JPEG", quality=90)
    img.close()


def detect_faces(src: Path) -> list[tuple[int, int, int, int]]:
    """Haar 真实人脸检测（坐标按解码像素，未计 EXIF 转正 —— 种子图 Orientation=1）。"""
    import cv2
    import numpy as np

    img = cv2.imread(str(src))
    if img is None:
        return []
    gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
    cascade = cv2.CascadeClassifier(
        cv2.data.haarcascades + "haarcascade_frontalface_default.xml"
    )
    faces = cascade.detectMultiScale(gray, scaleFactor=1.1, minNeighbors=5, minSize=(40, 40))
    out: list[tuple[int, int, int, int]] = []
    for (x, y, w, h) in faces:
        out.append((int(x), int(y), int(x + w), int(y + h)))
    # 面积大的人脸优先（与 representative_face 的并列判据一致）
    out.sort(key=lambda b: (b[2] - b[0]) * (b[3] - b[1]), reverse=True)
    return out


# ---------------------------------------------------------------------------
# 数据库写入
# ---------------------------------------------------------------------------

MARK_ARCHIVED = "archived"
MARK_HIDDEN = "hidden"
VIRTUAL_ARCHIVE = "__virtual__://archive"
VIRTUAL_HIDDEN = "__virtual__://hidden"


def app_data_dir() -> Path:
    return Path.home() / ".local" / "share" / "com.photomanagementsys.app"


def open_db() -> sqlite3.Connection:
    db = app_data_dir() / "photos.db"
    if not db.is_file():
        raise SystemExit(f"找不到照片库：{db}\n请先运行一次应用（任意登录即可生成库）。")
    conn = sqlite3.connect(str(db))
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def find_user(conn: sqlite3.Connection) -> int:
    row = conn.execute("SELECT id, username FROM users ORDER BY id LIMIT 1").fetchone()
    if row is None:
        raise SystemExit("用户表为空：请先注册/登录一个账户，再跑种子脚本。")
    return int(row["id"])


def clear_demo(conn: sqlite3.Connection, user_id: int) -> dict:
    """清掉上一次的演示数据（幂等重跑的前提）。

    只删演示相册（路径在 ALBUM_ROOT 下）及其关联行，不碰任何真实相册。
    """
    demo = [r["id"] for r in conn.execute(
        "SELECT id FROM albums WHERE user_id = ?1 AND path LIKE ?2",
        (user_id, f"{ALBUM_ROOT}%"),
    )]
    demo_paths = [r["path"] for r in conn.execute(
        "SELECT path FROM albums WHERE user_id = ?1 AND path LIKE ?2",
        (user_id, f"{ALBUM_ROOT}%"),
    )]
    stats = {"albums": 0, "scans": 0, "thumbs": 0, "embeddings": 0, "hits": 0,
             "marks": 0, "ratings": 0, "folders": 0, "album_ids": demo}
    if not demo:
        return stats

    ph = ",".join("?" * len(demo))
    # 注意不能用 ?n 编号：*demo 展开后占用的位置与 !?2 冲突（sqlite3 报 bindings 数量不符）
    stats["scans"] = conn.execute(
        f"DELETE FROM photo_content_scan WHERE user_id = ? AND (album_id IN ({ph}) OR parent_dir LIKE ?)",
        (user_id, *demo, f"{ALBUM_ROOT}%"),
    ).rowcount
    stats["thumbs"] = conn.execute(
        "DELETE FROM photo_thumb_cache WHERE user_id = ? AND source_path LIKE ?",
        (user_id, f"{ALBUM_ROOT}%"),
    ).rowcount
    stats["embeddings"] = conn.execute(
        "DELETE FROM photo_embeddings WHERE user_id = ? AND path LIKE ?",
        (user_id, f"{ALBUM_ROOT}%"),
    ).rowcount
    stats["hits"] = conn.execute(
        "DELETE FROM photo_category_hits WHERE user_id = ? AND path LIKE ?",
        (user_id, f"{ALBUM_ROOT}%"),
    ).rowcount
    stats["marks"] = conn.execute(
        "DELETE FROM photo_marks WHERE user_id = ? AND path LIKE ?",
        (user_id, f"{ALBUM_ROOT}%"),
    ).rowcount
    stats["ratings"] = conn.execute(
        "DELETE FROM photo_ratings WHERE user_id = ? AND path LIKE ?",
        (user_id, f"{ALBUM_ROOT}%"),
    ).rowcount
    # 只删演示分组（描述带标记，避免误删用户自建分组）
    folders = [r["id"] for r in conn.execute(
        "SELECT id FROM folders WHERE user_id = ?1 AND description LIKE ?2",
        (user_id, "%[demo-seed]%"),
    )]
    if folders:
        fph = ",".join("?" * len(folders))
        conn.execute(f"DELETE FROM folder_albums WHERE folder_id IN ({fph})", folders)
        conn.execute(f"DELETE FROM folder_tags WHERE folder_id IN ({fph})", folders)
        stats["folders"] = conn.execute(
            f"DELETE FROM folders WHERE id IN ({fph})", folders
        ).rowcount
    for aid in demo:
        conn.execute("DELETE FROM folder_albums WHERE album_id = ?1", (aid,))
    conn.execute(f"DELETE FROM album_tags WHERE album_id IN ({ph})", demo)
    conn.execute(f"DELETE FROM album_stats WHERE album_id IN ({ph})", demo)
    conn.execute(f"DELETE FROM albums WHERE id IN ({ph})", demo)
    stats["albums"] = len(demo)
    conn.commit()
    return stats


def fmt_shutter(v: float) -> str:
    if v >= 1.0:
        return f"{v:.0f}s" if v == int(v) else f"{v:.1f}s"
    return f"1/{round(1.0 / v):.0f}s"


def fmt_aperture(v: float) -> str:
    return f"f/{v:.1f}"


def fmt_focal(v: int) -> str:
    return f"{v:.0f}mm" if float(v) == int(v) else f"{v:.1f}mm"


def content_text(p: Photo) -> str:
    """与 content.rs::build_records 的聚合口径一致（大类+细类+label+top3+人物）。"""
    parts: list[str] = []
    if p.category:
        parts.append(p.category)
    if p.sub:
        parts.append(p.sub)
    if p.label:
        parts.append(p.label)
    parts.append(p.label)  # top3 的第二名用同一 label（模仿）
    parts.extend(p.persons)
    return " ".join(parts).lower()


def write_photos(
    conn: sqlite3.Connection,
    user_id: int,
    plans: list[tuple[dict, Path, list[Photo]]],
    faces_by_photo: dict[str, list[tuple[int, int, int, int]]],
) -> list[dict]:
    """写相册 / 内容扫描行 / 向量 / 命中 / 评分 / 标记。返回照片记录。"""
    now = int(time.time())
    records: list[dict] = []

    for album, album_dir, photos in plans:
        album_id = conn.execute(
            "INSERT INTO albums (name, path, description, cover_path, created_at, updated_at,"
            " user_id, location, sort_order, kind)"
            " VALUES (?1, ?2, ?3, NULL, ?4, ?4, ?5, ?6, 0, 'normal')",
            (album["name"], str(album_dir), album["desc"], now, user_id, album["location"]),
        ).lastrowid
        # 相册标签（最多 5）
        for tag in album["tags"][:5]:
            conn.execute(
                "INSERT OR IGNORE INTO album_tags (album_id, tag) VALUES (?1, ?2)", (album_id, tag)
            )

        for p in photos:
            src = album_dir / p.name
            if not src.is_file():
                continue
            ph = photo_hash(src)
            st = src.stat()
            parent = str(src.parent)

            # ---- photo_content_scan（owns = AI + EXIF，模仿一次完整扫描）----
            top3 = [
                {"category": p.category, "label": p.label, "confidence": round(p.conf, 3)},
                {"category": p.category, "label": p.label, "confidence": round(p.conf * 0.6, 3)},
                {"category": p.category, "label": p.label, "confidence": round(p.conf * 0.3, 3)},
            ]
            conn.execute(
                "INSERT INTO photo_content_scan (photo_hash, path, parent_dir, album_id, user_id,"
                " content, category, sub_category, label, confidence, top3_json, person_ids,"
                " person_count, shoot_time, location, shutter_speed, iso, aperture, focal_length,"
                " lat, lon, user_tags, scanned_at, iso_num, focal_num, aperture_num, shutter_num,"
                " tone_type, avg_luma)"
                " VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,"
                "         ?20,?21,?22,?23,?24,?25,?26,?27,?28,?29)"
                " ON CONFLICT(photo_hash) DO UPDATE SET"
                " path=excluded.path, parent_dir=excluded.parent_dir, album_id=excluded.album_id,"
                " content=excluded.content, category=excluded.category,"
                " sub_category=excluded.sub_category, label=excluded.label,"
                " confidence=excluded.confidence, top3_json=excluded.top3_json,"
                " person_ids=excluded.person_ids, person_count=excluded.person_count,"
                " shoot_time=excluded.shoot_time, location=excluded.location,"
                " shutter_speed=excluded.shutter_speed, iso=excluded.iso,"
                " aperture=excluded.aperture, focal_length=excluded.focal_length,"
                " lat=excluded.lat, lon=excluded.lon, user_tags=excluded.user_tags,"
                " scanned_at=excluded.scanned_at, iso_num=excluded.iso_num,"
                " focal_num=excluded.focal_num, aperture_num=excluded.aperture_num,"
                " shutter_num=excluded.shutter_num, tone_type=excluded.tone_type,"
                " avg_luma=excluded.avg_luma",
                (
                    ph, str(src), parent, album_id, user_id,
                    content_text(p),
                    p.category, p.sub, p.label, round(p.conf, 3),
                    json.dumps(top3, ensure_ascii=False),
                    json.dumps(p.persons, ensure_ascii=False) if p.persons else None,
                    len(p.persons),
                    p.shoot, p.location,
                    fmt_shutter(p.shutter), str(p.iso), fmt_aperture(p.aperture), fmt_focal(p.focal),
                    p.lat, p.lon,
                    json.dumps(p.tags, ensure_ascii=False) if p.tags else None,
                    now,
                    p.iso, float(p.focal), float(p.aperture), float(p.shutter),
                    p.tone, p.luma,
                ),
            )

            # ---- 评分 ----
            if p.rating > 0:
                conn.execute(
                    "INSERT INTO photo_ratings (user_id, path, rating, updated_at)"
                    " VALUES (?1, ?2, ?3, ?4)"
                    " ON CONFLICT(user_id, path) DO UPDATE SET rating=excluded.rating,"
                    " updated_at=excluded.updated_at",
                    (user_id, str(src), p.rating, now),
                )

            # ---- 缩略图缓存行（按真实命名，前端 fileUrl 直接命中）----
            thumb_rel = grid_thumb_name(album_id, src)
            thumb_path = app_data_dir() / "thumbs" / "grid" / thumb_rel
            if thumb_path.is_file():
                conn.execute(
                    "INSERT INTO photo_thumb_cache (photo_hash, source_path, thumb_path, album_id,"
                    " user_id, size_bytes, mtime_ns, generated_at)"
                    " VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"
                    " ON CONFLICT(photo_hash) DO UPDATE SET"
                    " source_path=excluded.source_path, thumb_path=excluded.thumb_path,"
                    " album_id=excluded.album_id, size_bytes=excluded.size_bytes,"
                    " mtime_ns=excluded.mtime_ns",
                    (ph, str(src), str(thumb_path), album_id, user_id,
                     st.st_size, st.st_mtime_ns, now),
                )

            # ---- 512 维伪向量（模仿 CLIP 图像塔；同类聚类 + 确定性扰动）----
            vec = category_vector(p.category, ph)
            blob = struct.pack(f"<{EMB_DIM}f", *vec)
            conn.execute(
                "INSERT INTO photo_embeddings (photo_hash, user_id, album_id, path, dim, model,"
                " embedding, generated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"
                " ON CONFLICT(photo_hash) DO UPDATE SET album_id=excluded.album_id,"
                " path=excluded.path, embedding=excluded.embedding,"
                " generated_at=excluded.generated_at",
                (ph, user_id, album_id, str(src), EMB_DIM, CLIP_MODEL_ID, blob,
                 time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(now))),
            )

            # ---- 描述向量（文本塔；同一类目共用一段描述 → 同一向量）----
            desc = build_desc(p)
            if desc:
                sh = desc_source_hash(desc)
                conn.execute(
                    "INSERT INTO photo_text_embeddings (photo_hash, kind, text, model, dim,"
                    " embedding, source_hash, generated_at)"
                    " VALUES (?1,'desc',?2,?3,?4,?5,?6,?7)"
                    " ON CONFLICT(photo_hash, kind, model) DO UPDATE SET text=excluded.text,"
                    " embedding=excluded.embedding, source_hash=excluded.source_hash,"
                    " generated_at=excluded.generated_at",
                    (ph, desc, CLIP_MODEL_ID, EMB_DIM, blob, sh,
                     time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(now))),
                )

            records.append({
                "hash": ph, "path": str(src), "album_id": album_id, "photo": p,
                "faces": faces_by_photo.get(p.name, []),
            })

        # ---- 封面：第一张照片的缩略图（相册卡片立刻有图）----
        if photos:
            cover_src = album_dir / photos[0].name
            cover_rel = cover_thumb_name(album_id, cover_src)
            cover_path = app_data_dir() / "thumbs" / cover_rel
            if cover_path.is_file():
                conn.execute(
                    "UPDATE albums SET cover_path = ?1 WHERE id = ?2", (str(cover_path), album_id)
                )
        conn.execute(
            "INSERT OR REPLACE INTO album_stats (album_id, photo_count, size_bytes, shoot_time,"
            " cover_source, scanned_at, file_count) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            (album_id, len(photos), sum((album_dir / p.name).stat().st_size for p in photos),
             photos[0].shoot[:10], str(album_dir / photos[0].name), now, len(photos)),
        )

    conn.commit()
    return records


def build_desc(p: Photo) -> str:
    """与 textdesc.rs::desc_segments 同口径：时间 · 地点 · 场景 · 标签 · 人物真名。"""
    segs: list[str] = []
    t = p.shoot
    segs.append(f"{int(t[:4])}年{int(t[5:7])}月")
    if p.location:
        segs.append(p.location)
    scene = " ".join(x for x in [p.category, p.sub, p.label] if x)
    if scene:
        segs.append(scene)
    if p.tags:
        segs.append("、".join(p.tags))
    if p.persons:
        segs.append("、".join(PERSON_NAMES.get(pid, pid) for pid in p.persons))
    return " · ".join(segs)


def desc_source_hash(desc: str) -> str:
    """与 textdesc.rs::source_hash 同算法（FNV-1a 64 十六进制）。"""
    return f"{fnv1a64_str(desc):016x}"


def write_categories(conn: sqlite3.Connection, user_id: int, records: list[dict]) -> dict:
    """写分类定义 + 命中物化，让「内容分类」页与「智慧相册」分类板块有内容。

    builtin（规则分类）走 SQL 直算口径（replace_builtin_hits）：按
    photo_content_scan.category 命中，score 用 confidence。
    preset / user（语义分类）按 keywords 关键词匹配打分。
    """
    stats = {"builtin": 0, "semantic": 0}
    now = int(time.time())

    # builtin 规则分类：slug → 命中的 category key
    builtin_specs = [
        ("portrait", "人物", "👤", ["portrait"]),
        ("street", "扫街", "🚶", ["street"]),
        ("night_scene", "夜景", "🌃", ["night_scene"]),
        ("document", "文档", "📄", ["document"]),
    ]
    for slug, name, icon, cats in builtin_specs:
        conn.execute(
            "INSERT INTO photo_categories (user_id, name, icon, source, slug, keywords,"
            " exclude_keywords, threshold, sort_order, enabled, created_at, updated_at)"
            " VALUES (?1,?2,?3,'builtin',?4,NULL,NULL,0,?5,1,?6,?6)"
            " ON CONFLICT(user_id, name) DO UPDATE SET icon=excluded.icon, source='builtin',"
            " slug=excluded.slug, enabled=1",
            (user_id, name, icon, slug, -len(cats), now),
        )
        # ⚠ 不能用 lastrowid：ON CONFLICT DO UPDATE 命中更新分支时它不可靠
        #   （SQLite 语义上是"最后一次插入"的 rowid），会把命中挂到别人身上。
        cat_id = conn.execute(
            "SELECT id FROM photo_categories WHERE user_id = ? AND name = ?", (user_id, name)
        ).fetchone()[0]
        conn.execute(
            "DELETE FROM photo_category_hits WHERE category_id = ?1 AND user_id = ?2",
            (cat_id, user_id),
        )
        n = 0
        for r in records:
            if r["photo"].category in cats:
                n += conn.execute(
                    "INSERT OR REPLACE INTO photo_category_hits (category_id, user_id, photo_hash,"
                    " path, score, matched_keyword, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    (cat_id, user_id, r["hash"], r["path"], r["photo"].conf,
                     r["photo"].category, now),
                ).rowcount
        stats["builtin"] += n

    # 语义分类（模仿 CLIP 净增益打分：score = 类目中心相似度 - 中性基线）
    semantic_specs = [
        ("动物", "🐾", ["动物", "野生动物", "宠物"], ["animal"]),
        ("猫咪", "🐱", ["一只猫", "小猫", "猫咪"], ["animal"]),
        ("狗狗", "🐶", ["一只狗", "小狗", "狗狗"], ["animal"]),
        ("美食", "🍜", ["美食", "食物", "菜肴", "甜点"], ["food"]),
        ("花卉", "🌸", ["花朵", "鲜花", "盛开的花"], ["flower"]),
        ("自然风景", "🏞️", ["自然风景", "山水风景", "湖泊"], ["landscape"]),
        ("天空云彩", "☁️", ["天空", "蓝天白云", "云彩"], ["landscape"]),
        ("建筑", "🏛️", ["建筑物", "楼房", "城市建筑"], ["architecture", "cityscape"]),
        ("日落晚霞", "🌅", ["日落", "晚霞", "夕阳"], ["night_scene", "landscape"]),
        ("海边", "🌊", ["大海", "海边", "海滩"], ["landscape"]),
        ("夜景灯光", "🌃", ["夜景", "夜晚的城市", "灯光"], ["night_scene"]),
        ("文字截图", "📱", ["文字", "文档", "屏幕截图"], ["document"]),
        ("运动", "🏃", ["运动", "体育比赛"], ["sports"]),
    ]
    order = 0
    for name, icon, keywords, cats in semantic_specs:
        order += 1
        conn.execute(
            "INSERT INTO photo_categories (user_id, name, icon, source, slug, keywords,"
            " exclude_keywords, threshold, sort_order, enabled, created_at, updated_at)"
            " VALUES (?1,?2,?3,'preset',NULL,?4,NULL,?5,?6,1,?7,?7)"
            " ON CONFLICT(user_id, name) DO UPDATE SET icon=excluded.icon, source='preset',"
            " keywords=excluded.keywords, enabled=1, sort_order=excluded.sort_order",
            (user_id, name, icon, json.dumps(keywords, ensure_ascii=False), 0.03, order, now),
        )
        cat_id = conn.execute(
            "SELECT id FROM photo_categories WHERE user_id = ? AND name = ?", (user_id, name)
        ).fetchone()[0]
        conn.execute(
            "DELETE FROM photo_category_hits WHERE category_id = ?1 AND user_id = ?2",
            (cat_id, user_id),
        )
        n = 0
        for r in records:
            if r["photo"].category in cats:
                # 净增益模仿值：0.02~0.08（真实库实测常见区间）
                score = round(0.02 + (hash(r["hash"]) % 60) / 1000.0, 4)
                n += conn.execute(
                    "INSERT OR REPLACE INTO photo_category_hits (category_id, user_id, photo_hash,"
                    " path, score, matched_keyword, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    (cat_id, user_id, r["hash"], r["path"], score, keywords[0], now),
                ).rowcount
        stats["semantic"] += n

    # 关键词向量缓存（让「分类管理」页显示已就绪）
    for _name, _icon, keywords, _cats in semantic_specs:
        for kw in keywords:
            vec = category_vector("keyword", kw)
            blob = struct.pack(f"<{EMB_DIM}f", *vec)
            conn.execute(
                "INSERT INTO clip_text_cache (model, text, dim, embedding, generated_at)"
                " VALUES (?1,?2,?3,?4,?5)"
                " ON CONFLICT(model, text) DO UPDATE SET embedding=excluded.embedding,"
                " generated_at=excluded.generated_at",
                (CLIP_MODEL_ID, kw, EMB_DIM, blob, now),
            )

    conn.commit()
    return stats


def write_persons(conn: sqlite3.Connection, records: list[dict]) -> dict:
    """写 persons.db（persons / faces / meta）。

    bbox 用 OpenCV Haar 真实检测结果；embedding 写与该人物同簇的 512 维伪向量
    （代表脸选择 = 与质心最相似，与真实链路一致 → 头像裁剪结果稳定）。
    """
    persons_db = app_data_dir() / "vcr-data" / "persons.db"
    persons_db.parent.mkdir(parents=True, exist_ok=True)
    pdb = sqlite3.connect(str(persons_db))
    pdb.executescript(
        """
        CREATE TABLE IF NOT EXISTS persons (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, centroid BLOB NOT NULL,
            face_count INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS faces (
            id INTEGER PRIMARY KEY AUTOINCREMENT, person_id TEXT NOT NULL,
            photo_path TEXT NOT NULL, bbox TEXT NOT NULL, embedding BLOB NOT NULL,
            created_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
        """
    )
    pdb.execute("DELETE FROM faces")
    pdb.execute("DELETE FROM persons")

    # 收集每个人物的脸（照片, bbox）
    per_person: dict[str, list[tuple[dict, tuple[int, int, int, int]]]] = {}
    for r in records:
        for idx, pid in enumerate(r["photo"].persons):
            faces = r["faces"]
            if not faces:
                continue
            bbox = faces[idx % len(faces)]
            per_person.setdefault(pid, []).append((r, bbox))

    created = 0
    faces_written = 0
    for pid in sorted(per_person):
        entries = per_person[pid]
        # 质心 = 该人物所有脸向量的均值（与 PersonStore.register 的增量均值等价）
        vecs = [category_vector("person", f"{pid}|{r['hash']}") for r, _ in entries]
        mean = [sum(v[i] for v in vecs) / len(vecs) for i in range(EMB_DIM)]
        norm = sum(x * x for x in mean) ** 0.5 or 1.0
        mean = [x / norm for x in mean]
        centroid = struct.pack(f"<{EMB_DIM}f", *mean)
        stamp = time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(created + 1750000000))
        pdb.execute(
            "INSERT INTO persons (id, name, centroid, face_count, created_at)"
            " VALUES (?1,?2,?3,?4,?5)",
            (pid, PERSON_NAMES.get(pid, pid), centroid, len(entries), stamp),
        )
        created += 1
        for r, bbox in entries:
            emb = category_vector("person", f"{pid}|{r['hash']}")
            pdb.execute(
                "INSERT INTO faces (person_id, photo_path, bbox, embedding, created_at)"
                " VALUES (?1,?2,?3,?4,?5)",
                (pid, r["path"], f"({bbox[0]}, {bbox[1]}, {bbox[2]}, {bbox[3]})",
                 struct.pack(f"<{EMB_DIM}f", *emb), stamp),
            )
            faces_written += 1

    pdb.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('emb_model', 'w600k_r50.onnx')"
    )
    pdb.commit()
    pdb.close()
    return {"persons": created, "faces": faces_written}


def write_folders(conn: sqlite3.Connection, user_id: int, dir_to_album: dict[str, int]) -> int:
    """写手动排序分组（folders + folder_albums + folder_tags）。"""
    now = int(time.time())
    written = 0
    for i, folder in enumerate(FOLDERS):
        fid = conn.execute(
            "INSERT INTO folders (name, parent_id, level, sort_order, description, created_at,"
            " updated_at, user_id) VALUES (?1,NULL,1,?2,?3,?4,?4,?5)",
            (folder["name"], i, f"{folder['desc']} [demo-seed]", now, user_id),
        ).lastrowid
        for j, child in enumerate(folder["children"]):
            aid = dir_to_album.get(child)
            if aid is None:
                continue
            conn.execute(
                "INSERT OR IGNORE INTO folder_albums (folder_id, album_id, sort_order)"
                " VALUES (?1,?2,?3)",
                (fid, aid, j),
            )
            # 同步冗余列（folder_albums 是唯一事实源，albums.folder_id 仅缓存）
            conn.execute("UPDATE albums SET folder_id = ?1, sort_order = ?2 WHERE id = ?3",
                         (fid, j, aid))
        for tag in ["演示", "种子数据"][:5]:
            conn.execute(
                "INSERT OR IGNORE INTO folder_tags (folder_id, tag) VALUES (?1,?2)", (fid, tag)
            )
        written += 1
    conn.commit()
    return written


def mark_special(conn: sqlite3.Connection, user_id: int, records: list[dict]) -> dict:
    """归档 / 隐藏标记：让两个虚拟相册也有内容（相册管理里可见）。"""
    now = int(time.time())
    archived = records[1::5]  # 每 5 张挑 1 张归档
    hidden = records[3::7]  # 每 7 张挑 1 张隐藏
    for r in archived:
        conn.execute(
            "INSERT OR REPLACE INTO photo_marks (user_id, path, state, updated_at)"
            " VALUES (?1,?2,?3,?4)",
            (user_id, r["path"], MARK_ARCHIVED, now),
        )
    for r in hidden:
        conn.execute(
            "INSERT OR REPLACE INTO photo_marks (user_id, path, state, updated_at)"
            " VALUES (?1,?2,?3,?4)",
            (user_id, r["path"], MARK_HIDDEN, now),
        )
    conn.commit()
    return {"archived": len(archived), "hidden": len(hidden)}


def clean_caches(album_ids: list[int]) -> None:
    """删掉演示相关的缩略图/头像缓存，避免重跑后残留旧文件。

    只清「演示库产出的缓存」：
      · thumbs/grid/ 下全部网格缩略图（路径与相册 id 绑定，重跑后 id 会变）
      · thumbs/album_*_auto_*/ 相册封面目录
      · avatars/avatar_P*（人物头像 + 签名）
    不碰用户头像 user_*.jpg 与其他真实相册的缓存。
    """
    base = app_data_dir()
    thumbs = base / "thumbs"
    grid = thumbs / "grid"
    # 只按「本次要重建的相册 id」精确清，不扫全目录：
    # grid 里同时存在用户真实相册的缩略图，全量清会让真实相册首屏重新生成（很慢）。
    for aid in album_ids:
        if grid.is_dir():
            for f in grid.glob(f"album_{aid}_photo_*"):
                if f.is_file():
                    f.unlink()
            for d in grid.glob(f"album_{aid}_photo_*"):
                if d.is_dir():
                    shutil.rmtree(d, ignore_errors=True)
        if thumbs.is_dir():
            for d in thumbs.glob(f"album_{aid}_auto_*"):
                if d.is_dir():
                    shutil.rmtree(d, ignore_errors=True)
    avatars = base / "avatars"
    if avatars.is_dir():
        for f in avatars.glob("avatar_P*"):
            f.unlink()


def gen_thumbs(
    app: Path,
    records: list[dict],
    plans: list[tuple[dict, Path, list[Photo]]],
) -> None:
    """按分配好的 album_id 生成缩略图（命名依赖 album_id，必须在写库后做）。

    两张都要：网格缩略图（grid/album_{id}_photo_*.webp）与相册封面
    （album_{id}_auto_{fp[:2]}/{fp}.webp，后端 fill_album_stats 走同一命名）。
    """
    for album, album_dir, photos in plans:
        album_id = None
        for r in records:
            if r["path"].startswith(str(album_dir)):
                album_id = r["album_id"]
                break
        if album_id is None:
            continue
        for p in photos:
            src = album_dir / p.name
            if not src.is_file():
                continue
            make_thumb(src, app / "thumbs" / "grid" / grid_thumb_name(album_id, src))
            # 旧嵌套命名也生成一份：grid_thumb_cache_name（人物照片链路）先查它
            make_thumb(src, app / "thumbs" / "grid" / grid_thumb_name_legacy(album_id, src))
        if photos:
            cover_src = album_dir / photos[0].name
            if cover_src.is_file():
                make_thumb(cover_src, app / "thumbs" / cover_thumb_name(album_id, cover_src))


def gen_avatars(app: Path, persons_db: Path, records: list[dict]) -> list[str]:
    """为每个人物裁 96×96 头像（代表脸 = 面积最大的一张），并写身份签名。

    签名必须与 `persons.rs::face_sig_of` 完全一致：`COUNT(*):COALESCE(MAX(faces.id),0)`。
    签名对不上 → 后端 `cache_valid` 判负 → 删缓存重裁；若此时代表脸 bbox 与写入
    头像时用的不是同一张脸，用户就会看到「头像变成了别人」。
    因此这里**从 persons.db 实测签名**，不用本地计数推断。
    """
    avatars = app / "avatars"
    avatars.mkdir(parents=True, exist_ok=True)
    written: list[str] = []
    pdb = sqlite3.connect(str(persons_db))
    sigs = {
        pid: f"{n}:{m}"
        for pid, n, m in pdb.execute(
            "SELECT person_id, COUNT(*), COALESCE(MAX(id), 0) FROM faces GROUP BY person_id"
        )
    }
    # 代表脸 = 与「面积最大那张脸」对应的照片（与后端按面积/相似度择脸的倾向一致）
    pdb.close()
    for pid in PERSON_NAMES:
        best: tuple[int, dict, tuple[int, int, int, int]] | None = None
        for r in records:
            if pid not in r["photo"].persons or not r["faces"]:
                continue
            bbox = r["faces"][0]
            area = (bbox[2] - bbox[0]) * (bbox[3] - bbox[1])
            if best is None or area > best[0]:
                best = (area, r, bbox)
        if best is None:
            continue
        _area, r, bbox = best
        make_avatar(Path(r["path"]), bbox, avatars / f"avatar_{pid}.jpg")
        (avatars / f"avatar_{pid}.sig").write_text(sigs.get(pid, "0:0"))
        written.append(pid)
    return written


def main() -> int:
    ap = argparse.ArgumentParser(description="相册演示数据种子脚本（不跑 AI）")
    ap.add_argument("--no-download", action="store_true", help="不下载图片（要求已存在）")
    ap.add_argument("--dry-run", action="store_true", help="只打印计划，不写任何东西")
    ap.add_argument("--refresh-exif", action="store_true",
                    help="配合 --no-download：按清单重写已有图片的 EXIF")
    args = ap.parse_args()

    app = app_data_dir()
    total_photos = sum(len(a["photos"]) for a in ALBUMS)
    print(f"演示相册 {len(ALBUMS)} 个 / 照片 {total_photos} 张 / 人物 {len(PERSON_NAMES)} 位")
    print(f"图片目录：{ALBUM_ROOT}")
    print(f"照片库：  {app / 'photos.db'}")
    print(f"人物库：  {app / 'vcr-data' / 'persons.db'}")
    if args.dry_run:
        for a in ALBUMS:
            print(f"  · {a['name']}  ({len(a['photos'])} 张)  → {ALBUM_ROOT / a['dir']}")
        return 0

    # ---- 1. 下载 + 写 EXIF ----
    deps()
    ALBUM_ROOT.mkdir(parents=True, exist_ok=True)
    plans: list[tuple[dict, Path, list[Photo]]] = []
    for album in ALBUMS:
        album_dir = ALBUM_ROOT / album["dir"]
        album_dir.mkdir(parents=True, exist_ok=True)
        # 目录内陈旧图片清理：只保留清单里的文件。
        # 为什么必须做：目录里混进上一轮/手工放进来的图会让 album_stats.file_count
        # 与实际不符 → 应用每次 get_albums 都触发全量重扫（慢，且封面可能被覆盖），
        # 网格里还会出现没有元数据的「孤儿照片」。
        keep = {p.name for p in album["photos"]}
        for f in album_dir.iterdir():
            if f.is_file() and f.suffix.lower() in IMAGE_EXTS and f.name not in keep:
                f.unlink()
                print(f"  ✕ 清理陈旧图片 {album['dir']}/{f.name}")
        for p in album["photos"]:
            dest = album_dir / p.name
            if not args.no_download:
                download(p.url, dest, fallback_seed=1000 + hash(p.name) % 9000)
                print(f"  ↓ {album['name']}/{p.name}  ({dest.stat().st_size // 1024} KB)")
                write_exif(dest, p)
            elif dest.is_file():
                # --no-download 也要重刷 EXIF：清单里改了拍摄时间/地点时必须重新落盘，
                # 否则文件里还是上一轮的旧 EXIF（页面显示与清单不一致）
                if args.refresh_exif:
                    write_exif(dest, p)
        plans.append((album, album_dir, album["photos"]))

    # ---- 2. 人脸检测（bbox 要进 persons.db，必须在缩略图之前）----
    faces_by_photo: dict[str, list[tuple[int, int, int, int]]] = {}
    for _album, album_dir, photos in plans:
        for p in photos:
            src = album_dir / p.name
            if src.is_file() and (p.category == "portrait" or p.persons):
                faces_by_photo[p.name] = detect_faces(src)
    print(f"人脸检测：{sum(len(v) for v in faces_by_photo.values())} 张脸")

    # ---- 3. 写照片库 ----
    conn = open_db()
    user_id = find_user(conn)
    cleared = clear_demo(conn, user_id)
    print(f"清理旧演示数据：相册 {cleared['albums']} / 扫描行 {cleared['scans']} / "
          f"缩略图 {cleared['thumbs']} / 向量 {cleared['embeddings']} / "
          f"命中 {cleared['hits']} / 分组 {cleared['folders']}")

    # 缩略图命名依赖 album_id → 先按旧 id 清缓存，再建相册，再生成，再补缓存行
    clean_caches(cleared["album_ids"])
    records = write_photos(conn, user_id, plans, faces_by_photo)
    gen_thumbs(app, records, plans)
    # 缩略图已落盘 → 回填缓存行（write_photos 时文件还不存在，那一轮写不进去）
    now = int(time.time())
    for r in records:
        album_id = r["album_id"]
        thumb = app / "thumbs" / "grid" / grid_thumb_name(album_id, Path(r["path"]))
        if thumb.is_file():
            conn.execute(
                "INSERT INTO photo_thumb_cache (photo_hash, source_path, thumb_path, album_id,"
                " user_id, size_bytes, mtime_ns, generated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"
                " ON CONFLICT(photo_hash) DO UPDATE SET thumb_path=excluded.thumb_path,"
                " album_id=excluded.album_id, mtime_ns=excluded.mtime_ns",
                (r["hash"], r["path"], str(thumb), album_id, user_id,
                 Path(r["path"]).stat().st_size, Path(r["path"]).stat().st_mtime_ns, now),
            )
    # 封面同样回填（albums.cover_path 让相册卡片立刻有图）
    for album, album_dir, photos in plans:
        album_id = next((r["album_id"] for r in records if r["path"].startswith(str(album_dir))), None)
        if album_id is None or not photos:
            continue
        cover = app / "thumbs" / cover_thumb_name(album_id, album_dir / photos[0].name)
        if cover.is_file():
            conn.execute("UPDATE albums SET cover_path = ?1 WHERE id = ?2", (str(cover), album_id))
    conn.commit()
    print(f"缩略图生成：{len(records)} 张")

    cats = write_categories(conn, user_id, records)
    print(f"分类命中：规则 {cats['builtin']} 条 / 语义 {cats['semantic']} 条")

    # 人物库先写：头像签名要从 persons.db 的 faces 实测（顺序不能反）
    persons = write_persons(conn, records)
    print(f"人物库：{persons['persons']} 位 / {persons['faces']} 张脸")

    avatar_pids = gen_avatars(app, app / "vcr-data" / "persons.db", records)
    print(f"人物头像：{len(avatar_pids)} 位（{', '.join(avatar_pids)}）")

    dir_to_album: dict[str, int] = {}
    for album, album_dir, _photos in plans:
        aid = next((r["album_id"] for r in records if r["path"].startswith(str(album_dir))), None)
        if aid is not None:
            dir_to_album[album["dir"]] = aid
    folders = write_folders(conn, user_id, dir_to_album)
    print(f"手动排序分组：{folders} 个")

    marks = mark_special(conn, user_id, records)
    print(f"标记：归档 {marks['archived']} 张 / 隐藏 {marks['hidden']} 张")

    # ---- 4. 汇总 ----
    n_scan = conn.execute("SELECT COUNT(*) FROM photo_content_scan WHERE user_id = ?1",
                          (user_id,)).fetchone()[0]
    n_thumb = conn.execute("SELECT COUNT(*) FROM photo_thumb_cache WHERE user_id = ?1",
                           (user_id,)).fetchone()[0]
    n_emb = conn.execute("SELECT COUNT(*) FROM photo_embeddings WHERE user_id = ?1",
                         (user_id,)).fetchone()[0]
    n_albums = conn.execute("SELECT COUNT(*) FROM albums WHERE user_id = ?1 AND kind = 'normal'",
                            (user_id,)).fetchone()[0]
    conn.close()
    print("\n完成。当前库内（本用户）：")
    print(f"  相册 {n_albums} 个 / 已入库 {n_scan} 张 / 缩略图 {n_thumb} 条 / 向量 {n_emb} 条")
    print("\n重启应用（或刷新页面）即可看到各页面内容。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
