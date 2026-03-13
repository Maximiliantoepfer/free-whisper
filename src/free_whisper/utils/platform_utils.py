from __future__ import annotations

import os
import sys
from pathlib import Path


def get_app_data_dir() -> Path:
    if sys.platform == "win32":
        base = Path(os.environ.get("APPDATA", Path.home() / "AppData" / "Roaming"))
    elif sys.platform == "darwin":
        base = Path.home() / "Library" / "Application Support"
    else:
        base = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
    path = base / "free-whisper"
    path.mkdir(parents=True, exist_ok=True)
    return path


def get_db_path() -> Path:
    return get_app_data_dir() / "free_whisper.db"


def get_models_cache_dir() -> Path:
    path = get_app_data_dir() / "models"
    path.mkdir(parents=True, exist_ok=True)
    return path


def acquire_single_instance_lock() -> bool:
    """Return True if this is the first instance, False if another is already running."""
    if sys.platform == "win32":
        import ctypes

        ctypes.windll.kernel32.CreateMutexW(None, False, "FreeWhisperSingleInstance")
        return ctypes.windll.kernel32.GetLastError() != 183  # ERROR_ALREADY_EXISTS
    else:
        lock_file = get_app_data_dir() / "instance.lock"
        try:
            import fcntl

            fh = open(lock_file, "w")
            fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
            # Store handle so it isn't garbage-collected (would release the lock)
            acquire_single_instance_lock._lock_fh = fh  # type: ignore[attr-defined]
            return True
        except (OSError, ImportError):
            return False


def set_start_on_login(enabled: bool, app_name: str = "free-whisper") -> None:
    """Add/remove the app from OS startup entries."""
    if sys.platform == "win32":
        import winreg

        key_path = r"Software\Microsoft\Windows\CurrentVersion\Run"
        exe_path = sys.executable if getattr(sys, "frozen", False) else ""
        if not exe_path:
            return  # only works for compiled .exe

        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key_path, 0, winreg.KEY_SET_VALUE) as key:
            if enabled:
                winreg.SetValueEx(key, app_name, 0, winreg.REG_SZ, f'"{exe_path}" --minimized')
            else:
                try:
                    winreg.DeleteValue(key, app_name)
                except FileNotFoundError:
                    pass
    # macOS/Linux: LaunchAgent / .desktop file — out of scope for now


def is_frozen() -> bool:
    """Return True when running as a PyInstaller bundle."""
    return getattr(sys, "frozen", False)


def get_assets_dir() -> Path:
    if is_frozen():
        # PyInstaller unpacks assets next to the executable
        return Path(sys._MEIPASS) / "assets"  # type: ignore[attr-defined]
    # Running from source
    return Path(__file__).parent.parent.parent.parent / "assets"
