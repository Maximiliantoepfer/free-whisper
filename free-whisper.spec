# -*- mode: python ; coding: utf-8 -*-
import glob as _glob
import os as _os
from PyInstaller.utils.hooks import collect_all

datas = [('assets', 'assets')]
binaries = []
hiddenimports = [
    'onnxruntime',
    'sounddevice', '_sounddevice_data',
    'keyboard',
    'pyperclip',
    'pyautogui',
    'pyscreeze',
    'pygetwindow',
    'win32api', 'win32con', 'win32gui', 'pywintypes',
    'numpy', 'numpy.core', 'numpy.core._multiarray_umath',
    'free_whisper',
    'free_whisper.core',
    'free_whisper.ui',
    'free_whisper.utils',
    'free_whisper.db',
]

# ctranslate2: DLLs must land in ctranslate2/ subdirectory so that
# ctranslate2/__init__.py can find and pre-load them via ctypes.CDLL().
# PyInstaller's `binaries` puts everything flat in _MEIPASS root — wrong.
# We add them as `datas` with explicit destination 'ctranslate2' instead.
_ct2_pkg = _os.path.join('.venv', 'Lib', 'site-packages', 'ctranslate2')
datas += [
    (dll, 'ctranslate2')
    for dll in _glob.glob(_os.path.join(_ct2_pkg, '*.dll'))
    if 'cudnn' not in _os.path.basename(dll).lower()
]

tmp_ret = collect_all('ctranslate2')
datas += tmp_ret[0]           # Python files → ctranslate2/
# tmp_ret[1] (binaries) intentionally skipped — DLLs handled above
hiddenimports += tmp_ret[2]

tmp_ret = collect_all('faster_whisper')
datas += tmp_ret[0]; binaries += tmp_ret[1]; hiddenimports += tmp_ret[2]

tmp_ret = collect_all('sounddevice')
datas += tmp_ret[0]; binaries += tmp_ret[1]; hiddenimports += tmp_ret[2]


a = Analysis(
    ['app_entry.py'],
    pathex=['src'],
    binaries=binaries,
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=['.'],
    hooksconfig={},
    runtime_hooks=['rthook_ctranslate2.py'],
    excludes=['torch', 'torchaudio'],
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)

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
    upx_exclude=[],
    console=False,  # True for terminal logs and False for production
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    icon=['assets\\icons\\app_icon.ico'],
)

coll = COLLECT(
    exe,
    a.binaries,
    a.datas,
    strip=False,
    upx=True,
    upx_exclude=[],
    name='free-whisper',
)
