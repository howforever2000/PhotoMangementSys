# -*- mode: python ; coding: utf-8 -*-
# VCR 视觉识别微服务 PyInstaller 打包定义（单文件 exe）
#
# 用法（由 build_vcr_exe.ps1 调用，需在 python/.venv-vcr 最小化环境中执行）：
#   .venv-vcr/Scripts/pyinstaller.exe --clean --noconfirm vcr-server.spec
#
# 说明：
#   - 入口 server.py，输出 dist/vcr-server.exe（单文件，含 Python 运行时 + 全部依赖）
#   - 模型(.onnx) / 数据(persons.db) 不打包进 exe，运行期由 config 通过
#     VCR_MODEL_DIR / VCR_DATA_DIR 环境变量指向宿主可写区（由宿主 Rust 拉起时注入）。
#   - console=True：exe 为控制台子系统；宿主用 CREATE_NO_WINDOW 隐藏窗口。

a = Analysis(
    ['server.py'],
    pathex=['.'],
    binaries=[],
    datas=[],
    hiddenimports=[
        'uvicorn', 'uvicorn.logging', 'uvicorn.loops.auto', 'uvicorn.loops.asyncio',
        'uvicorn.protocols.http.auto', 'uvicorn.protocols.http.h11_impl',
        'uvicorn.protocols.websockets.auto', 'uvicorn.lifespan.auto',
        'onnxruntime.capi._pybind_state', 'cv2',
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=['tkinter', 'matplotlib', 'pytest'],
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
    name='vcr-server',
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
