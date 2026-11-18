# -*- mode: python ; coding: utf-8 -*-
# AI 创作微服务 PyInstaller 打包定义（单文件 exe，FEAT-152 / 本地增强打包修复）
#
# 用法（由 build_ai_exe.ps1 调用，需在 python/.venv-ai 最小化环境中执行）：
#   .venv-ai/Scripts/pyinstaller.exe --clean --noconfirm ai-studio.spec
#
# 说明：
#   - 入口 ai_studio_entry.py（顶层启动器，导入 ai_studio.server → 保留包上下文）
#   - 输出 dist/ai-server.exe（单文件，含 Python 运行时 + fastapi/uvicorn/requests/Pillow
#     + numpy + onnxruntime + 内置 LUT 资产）
#   - console=True：控制台子系统；宿主用 CREATE_NO_WINDOW 隐藏窗口（与 vcr-server 同）
#
# 为什么必须打包 numpy / onnxruntime（曾经的 excludes 是错的）：
#   `server.py` → `jobs.py` → `workflows/__init__.py` 在 import 阶段就无条件加载
#   film_sim / restore，而它们直接 `import numpy`（film_sim.py）与惰性 import
#   onnxruntime（localmodels.py）。把 numpy 放进 excludes 的结果不是「本地增强不可用」，
#   而是**整个 ai-server.exe 在 import 阶段就崩**，云端工具也一起不可用。
#   发布版要做到「下载即用」，这两个包必须随包分发。
#
# 为什么把 ai_studio/luts 作为 datas 打包：
#   `config.LUTS_DIR` 指向包目录下的 luts/；单文件模式下代码在 PYZ、资产解包到
#   sys._MEIPASS，datas 的落位目标与 config.py 的 _MEIPASS 兜底路径严格对齐。
#
# FEAT-168：hiddenimports 与两份 JSON 清单由 op_catalog.json 生成，不再手抄模块名。
# 新增算子 / 工具只改那一份 JSON，打包不会静默漏模块（既有坑见 local-enhance.md §7）。

import json
import os

# PyInstaller 执行 spec 时注入 SPECPATH（spec 文件所在目录）；用它定位清单，
# 不依赖 os.getcwd()（build_ai_exe.ps1 可能与 spec 不在同一工作目录下启动）。
_SPEC_DIR = globals().get('SPECPATH') or os.path.dirname(os.path.abspath(__file__))

def _catalog():
    path = os.path.join(_SPEC_DIR, 'ai_studio', 'op_catalog.json')
    with open(path, 'r', encoding='utf-8') as fh:
        return json.load(fh)

def _catalog_modules():
    data = _catalog()
    out = []
    for item in [*data['ops'], *data['tools']]:
        mod = item.get('module', '')
        if mod.startswith('ai_studio.') and mod not in out:
            out.append(mod)
    return out

_CATALOG_MODULES = _catalog_modules()

a = Analysis(
    ['ai_studio_entry.py'],
    pathex=['.'],
    binaries=[],
    datas=[
        # 内置胶片 LUT（含 NOTICE.txt：CC-BY-NC-SA 要求随分发保留归属）
        ('ai_studio/luts', 'ai_studio/luts'),
        # 唯一真相源清单：运行时由 catalog.py / recipecatalog.py 读取，
        # 必须随包分发（漏一份 → 启动或首次使用该工具时 FileNotFoundError）
        ('ai_studio/op_catalog.json', 'ai_studio'),
        ('ai_studio/recipes.json', 'ai_studio'),
        # OCR 的 CTC 字典（FEAT-171）：config._bundled_file 读取，漏了 → 文字提取报「字典缺失」
        ('ai_studio/ppocr_keys_v1.txt', 'ai_studio'),
    ],
    hiddenimports=[
        # uvicorn 的动态导入子模块（缺了会在运行时以字符串名 import 失败）
        'uvicorn', 'uvicorn.logging', 'uvicorn.loops.auto', 'uvicorn.loops.asyncio',
        'uvicorn.protocols.http.auto', 'uvicorn.protocols.http.h11_impl',
        'uvicorn.protocols.websockets.auto', 'uvicorn.lifespan.auto',
        # onnxruntime 的原生绑定是惰性/动态导入，静态分析抓不到（与 vcr-server.spec 同处理）
        'onnxruntime', 'onnxruntime.capi', 'onnxruntime.capi._pybind_state',
        # 表单解析依赖（import 名为 multipart，包名 python-multipart）
        'multipart',
    ] + _CATALOG_MODULES + [
        # 清单之外、由 import 链引入但静态分析会漏的纯算法 / 支撑模块
        'ai_studio', 'ai_studio.catalog', 'ai_studio.recipecatalog',
        'ai_studio.critic', 'ai_studio.steptrace', 'ai_studio.logutil',
        'ai_studio.schemas', 'ai_studio.compose', 'ai_studio.presets',
        'ai_studio.providers', 'ai_studio.providers.openai_compat',
        'ai_studio.lutcube', 'ai_studio.colortransfer', 'ai_studio.facedetect',
        'ai_studio.localmodels', 'ai_studio.autograde', 'ai_studio.faceparse',
        'ai_studio.skinretouch', 'ai_studio.landmark', 'ai_studio.facewarp',
        # FEAT-171：文字提取（纯 numpy det 后处理 + rec CTC 解码），供 ocr 工作流用
        'ai_studio.ocr_engine',
        'ai_studio.workflows.base',
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    # 只排除确实用不到的重型包；numpy / onnxruntime 必须保留（见文件头说明）
    excludes=['tkinter', 'matplotlib', 'pytest', 'cv2', 'onnx', 'tokenizers', 'scipy'],
    noarchive=False,
    optimize=0,
)

pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name='ai-server',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=True,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
