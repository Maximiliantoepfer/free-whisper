from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime


@dataclass
class Transcript:
    id: int
    text: str
    created_at: datetime
    audio_duration_ms: int | None
    model_used: str | None
    language_detected: str | None

    @property
    def word_count(self) -> int:
        return len(self.text.split()) if self.text.strip() else 0

    @property
    def text_preview(self) -> str:
        return self.text[:120] + "..." if len(self.text) > 120 else self.text


@dataclass
class DictEntry:
    id: int
    word: str
    category: str
    created_at: datetime


@dataclass
class AppSettings:
    hotkey: str = "ctrl+shift+space"
    model_size: str = "large-v3-turbo"
    compute_type: str = "auto"
    audio_device_index: int | None = None
    language: str = ""  # empty = auto-detect
    recording_mode: str = "push_to_talk"  # "push_to_talk" | "toggle"
    vad_silence_ms: int = 1500
    initial_prompt: str = ""
    inject_delay_ms: int = 150
    theme: str = "dark"  # "dark" | "light" | "system"
    start_minimized: bool = False
    start_on_login: bool = False
