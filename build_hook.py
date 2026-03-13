# build_hook.py
from PyInstaller.utils.hooks import get_module_file_attribute, collect_submodules
import os

hiddenimports = [
    'faster_whisper',
    'ctranslate2',
    'torch',
    'torchaudio',
    'onnxruntime',
    'numpy',
    'PyQt6',
    'PyQt6.QtCore',
    'PyQt6.QtGui',
    'PyQt6.QtWidgets',
]
