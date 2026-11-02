# -*- mode: python ; coding: utf-8 -*-
# Agent 侧车 PyInstaller 打包定义（单文件 exe）
#
# 用法（由 build_agent_exe.ps1 调用，需在 python/.venv-agent 最小化环境中执行）：
#   .venv-agent/Scripts/pyinstaller.exe --clean --noconfirm agent_server.spec
#
# 体积预算：≤40MB（docs/v5.md §25-3）。所以本 spec **刻意不含**
# onnxruntime / opencv / numpy —— 抠主体等重活走用户配置的远程端点，
# 本地只留 Pillow（压缩、排版合成、增强前后整理）。
#
# ⚠️ hiddenimports 里 `agent.*` 一条都不能少：入口用 uvicorn 的
#    `"agent.app:app"` **字符串**导入，PyInstaller 静态分析看不到字符串里的模块，
#    漏掉的表现是「/health 通但 /agent/submit 崩」（ImportError: agent.app）。
#    这正是 docs/v5.md §24 最后一条要实测的东西，这里按完整清单显式列出。
#    另一处同类坑：uvicorn 的协议/loop 实现按平台动态选，缺一条就 500（BUG 见
#    vcr-server.spec 同段注释）。

a = Analysis(
    ['agent_server.py'],
    pathex=['.'],
    binaries=[],
    datas=[],
    hiddenimports=[
        # 业务包（字符串导入，静态分析不可见）
        'agent', 'agent.app', 'agent.auth', 'agent.jobs', 'agent.logutil',
        # uvicorn 动态选择链
        'uvicorn', 'uvicorn.logging', 'uvicorn.loops.auto', 'uvicorn.loops.asyncio',
        'uvicorn.protocols.http.auto', 'uvicorn.protocols.http.h11_impl',
        'uvicorn.protocols.websockets.auto', 'uvicorn.lifespan.auto',
        # fastapi 依赖注入在运行期用 typing 反射，稳妥起见显式带上
        'fastapi', 'pydantic', 'httpx', 'PIL.Image',
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[
        'tkinter', 'matplotlib', 'pytest',
        # 体积红线：这几个一旦被误引入，exe 会从 40MB 涨到 100MB+
        'onnxruntime', 'onnx', 'cv2', 'numpy', 'torch', 'tokenizers',
    ],
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
    name='agent-server',
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
