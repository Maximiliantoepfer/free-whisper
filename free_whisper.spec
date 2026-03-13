# -*- mode: python ; coding: utf-8 -*-
"""
PyInstaller spec for free-whisper.

Build commands:
  Windows .exe:
    pyinstaller free_whisper.spec --clean

  macOS .app:
    pyinstaller free_whisper.spec --clean --target-arch universal2

Notes:
  - Whisper models are NOT bundled. They download to %APPDATA%\free-whisper\models
    on first use via huggingface_hub (faster-whisper handles this automatically).
  - CTranslate2 uses ctypes.CDLL for runtime CPU dispatch DLL loading,
    so --collect-all ctranslate2 is required.
  - onnxruntime is required by silero-vad (if used).
"""
import sys
from pathlib import Path

block_cipher = None

a = Analysis(
    ['src/free_whisper/main.py'],
    pathex=[],
    binaries=[],
    datas=[
        ('assets', 'assets'),
    ],
    hiddenimports=[
        'faster_whisper',
        'ctranslate2',
        'sounddevice',
        'numpy',
        'PyQt6.QtCore',
        'PyQt6.QtGui',
        'PyQt6.QtWidgets',
        'PyQt6.QtSvg',
        'keyboard',
        'pyperclip',
        'pyautogui',
        'huggingface_hub',
        'tokenizers',
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[
        'torch',
        'tensorflow',
        'matplotlib',
        'tkinter',
        'PIL',
    ],
    win_no_prefer_redirects=False,
    win_private_assemblies=False,
    cipher=block_cipher,
    noarchive=False,
    # Collect all files from ctranslate2 (runtime DLL loading)
    collect_all=['ctranslate2'],
)

pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name='free-whisper',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    console=False,        # No console window
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    icon='assets/icons/app_icon.png',  # Use .ico on Windows for best results
)

coll = COLLECT(
    exe,
    a.binaries,
    a.zipfiles,
    a.datas,
    strip=False,
    upx=True,
    upx_exclude=[],
    name='free-whisper',
)

# macOS bundle
if sys.platform == 'darwin':
    app = BUNDLE(
        coll,
        name='free-whisper.app',
        icon='assets/icons/app_icon.png',
        bundle_identifier='com.free-whisper.app',
        info_plist={
            'NSMicrophoneUsageDescription': 'free-whisper needs microphone access to transcribe speech.',
            'LSUIElement': True,  # background app (no dock icon)
        },
    )
