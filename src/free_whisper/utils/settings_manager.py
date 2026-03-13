from __future__ import annotations

from PyQt6.QtCore import QObject, QSettings, pyqtSignal

from ..db.models import AppSettings


class SettingsManager(QObject):
    settings_changed = pyqtSignal(str)  # key that changed

    _DEFAULTS = {
        "hotkey": "ctrl+shift+space",
        "model_size": "small",
        "compute_type": "auto",
        "audio_device_index": "",
        "language": "",
        "recording_mode": "push_to_talk",
        "vad_silence_ms": "1500",
        "initial_prompt": "",
        "inject_delay_ms": "150",
        "theme": "dark",
        "start_minimized": "false",
        "start_on_login": "false",
        "log_level": "info",
    }

    def __init__(self, parent: QObject | None = None) -> None:
        super().__init__(parent)
        self._qs = QSettings("free-whisper", "free-whisper")

    # ------------------------------------------------------------------
    # Generic get/set
    # ------------------------------------------------------------------

    def get(self, key: str) -> str:
        return str(self._qs.value(key, self._DEFAULTS.get(key, "")))

    def set(self, key: str, value: str) -> None:
        old = self.get(key)
        if old != value:
            self._qs.setValue(key, value)
            self._qs.sync()
            self.settings_changed.emit(key)

    # ------------------------------------------------------------------
    # Typed accessors
    # ------------------------------------------------------------------

    def get_hotkey(self) -> str:
        return self.get("hotkey")

    def set_hotkey(self, value: str) -> None:
        self.set("hotkey", value)

    def get_model_size(self) -> str:
        return self.get("model_size")

    def set_model_size(self, value: str) -> None:
        self.set("model_size", value)

    def get_compute_type(self) -> str:
        return self.get("compute_type")

    def set_compute_type(self, value: str) -> None:
        self.set("compute_type", value)

    def get_audio_device_index(self) -> int | None:
        val = self.get("audio_device_index")
        return int(val) if val else None

    def set_audio_device_index(self, value: int | None) -> None:
        self.set("audio_device_index", str(value) if value is not None else "")

    def get_language(self) -> str:
        return self.get("language")

    def set_language(self, value: str) -> None:
        self.set("language", value)

    def get_recording_mode(self) -> str:
        return self.get("recording_mode")

    def set_recording_mode(self, value: str) -> None:
        self.set("recording_mode", value)

    def get_vad_silence_ms(self) -> int:
        return int(self.get("vad_silence_ms"))

    def set_vad_silence_ms(self, value: int) -> None:
        self.set("vad_silence_ms", str(value))

    def get_initial_prompt(self) -> str:
        return self.get("initial_prompt")

    def set_initial_prompt(self, value: str) -> None:
        self.set("initial_prompt", value)

    def get_inject_delay_ms(self) -> int:
        return int(self.get("inject_delay_ms"))

    def set_inject_delay_ms(self, value: int) -> None:
        self.set("inject_delay_ms", str(value))

    def get_theme(self) -> str:
        return self.get("theme")

    def set_theme(self, value: str) -> None:
        self.set("theme", value)

    def get_start_minimized(self) -> bool:
        return self.get("start_minimized").lower() == "true"

    def set_start_minimized(self, value: bool) -> None:
        self.set("start_minimized", "true" if value else "false")

    def get_start_on_login(self) -> bool:
        return self.get("start_on_login").lower() == "true"

    def set_start_on_login(self, value: bool) -> None:
        self.set("start_on_login", "true" if value else "false")

    def get_log_level(self) -> str:
        return self.get("log_level")

    def set_log_level(self, value: str) -> None:
        self.set("log_level", value)

    def to_app_settings(self) -> AppSettings:
        return AppSettings(
            hotkey=self.get_hotkey(),
            model_size=self.get_model_size(),
            compute_type=self.get_compute_type(),
            audio_device_index=self.get_audio_device_index(),
            language=self.get_language(),
            recording_mode=self.get_recording_mode(),
            vad_silence_ms=self.get_vad_silence_ms(),
            initial_prompt=self.get_initial_prompt(),
            inject_delay_ms=self.get_inject_delay_ms(),
            theme=self.get_theme(),
            start_minimized=self.get_start_minimized(),
            start_on_login=self.get_start_on_login(),
        )
