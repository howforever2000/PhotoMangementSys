# -*- mode: python ; coding: utf-8 -*-
# AI 创作微服务 PyInstaller 打包定义（单文件 exe，FEAT-152）
#
# 用法（由 build_ai_exe.ps1 调用，需在 python/.venv-ai 最小化环境中执行）：
#   .venv-ai/Scripts/pyinstaller.exe --clean --noconfirm ai-studio.spec
#
# 说明：
#   - 入口 ai_studio_entry.py（顶层启动器，导入 ai_studio.server → 保留包上下文）
#   - 输出 dist/ai-server.exe（单文件，含 Python 运行时 + fastapi/uvicorn/requests/Pillow）
#   - 无外部数据文件：预设/模板在 presets.py 内，字体取 C:\Windows\Fonts（系统自带），
#     作业在内存、模型配置由 Rust 每次请求附带 → datas 为空
#   - console=True：控制台子系统；宿主用 CREATE_NO_WINDOW 隐藏窗口（与 vcr-server 同）

a = Analysis(
    ['ai_studio_entry.py'],
    pathex=['.'],
    binaries=[],
    datas=[],
    hiddenimports=[
        # uvicorn 的动态导入子模块（缺了会在运行时以字符串名 import 失败）
        'uvicorn', 'uvicorn.logging', 'uvicorn.loops.auto', 'uvicorn.loops.asyncio',
        'uvicorn.protocols.http.auto', 'uvicorn.protocols.http.h11_impl',
        'uvicorn.protocols.websockets.auto', 'uvicorn.lifespan.auto',
        # 业务包内所有模块（相对导入链，静态分析偶有遗漏时兜底）
        'ai_studio', 'ai_studio.config', 'ai_studio.logutil', 'ai_studio.schemas',
        'ai_studio.jobs', 'ai_studio.presets', 'ai_studio.compose', 'ai_studio.pipeline',
        'ai_studio.ops',
        'ai_studio.providers', 'ai_studio.providers.openai_compat',
        'ai_studio.workflows', 'ai_studio.workflows.base', 'ai_studio.workflows.journal',
        'ai_studio.workflows.beauty', 'ai_studio.workflows.style_transfer',
        'ai_studio.workflows.upscale', 'ai_studio.workflows.smart_edit',
        # 表单解析依赖（import 名为 multipart，包名 python-multipart）
        'multipart',
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=['tkinter', 'matplotlib', 'pytest', 'onnxruntime', 'cv2', 'onnx', 'tokenizers', 'numpy'],
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
