from __future__ import annotations

import sys

from PyQt6.QtCore import QEvent, Qt, pyqtSignal
from PyQt6.QtWidgets import (
    QCheckBox,
    QComboBox,
    QFormLayout,
    QGroupBox,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from ...utils.settings_manager import SettingsManager
from ...core.audio_recorder import AudioRecorder


MODELS = [
    "tiny", "base", "small", "medium", "large-v2", "large-v3", "large-v3-turbo",
]
COMPUTE_TYPES = ["auto", "int8", "float16", "float32"]
LANGUAGES = [
    ("Auto-detect", ""), ("English", "en"), ("German", "de"), ("French", "fr"),
    ("Spanish", "es"), ("Italian", "it"), ("Portuguese", "pt"), ("Dutch", "nl"),
    ("Polish", "pl"), ("Russian", "ru"), ("Chinese", "zh"), ("Japanese", "ja"),
    ("Korean", "ko"), ("Arabic", "ar"), ("Turkish", "tr"),
]

class HotkeyEdit(QWidget):
    """QPushButton-based hotkey capture widget.

    Uses an event filter on the button to intercept keyPressEvent. A single
    keyPressEvent already carries both event.modifiers() and event.key(),
    so there is no timer and no race condition. Modifier-only presses
    (Ctrl, Shift, Alt alone) are ignored — we wait for a real key.

    Emits capture_started / capture_ended so the caller can pause the
    global HotkeyListener while the user is entering a new combination.
    """

    capture_started = pyqtSignal()
    capture_ended = pyqtSignal()
    hotkey_changed = pyqtSignal(str)  # emits e.g. "ctrl+shift+space"

    _MODIFIER_KEYS = {
        Qt.Key.Key_Control, Qt.Key.Key_Shift,
        Qt.Key.Key_Alt, Qt.Key.Key_Meta,
    }

    _QT_MOD_TO_STR = [
        (Qt.KeyboardModifier.ControlModifier, "ctrl"),
        (Qt.KeyboardModifier.ShiftModifier, "shift"),
        (Qt.KeyboardModifier.AltModifier, "alt"),
        (Qt.KeyboardModifier.MetaModifier, "win"),
    ]

    _QT_KEY_TO_STR: dict[int, str] = {
        Qt.Key.Key_Space: "space",
        Qt.Key.Key_Return: "enter",
        Qt.Key.Key_Enter: "enter",
        Qt.Key.Key_Tab: "tab",
        Qt.Key.Key_Escape: "esc",
        Qt.Key.Key_Backspace: "backspace",
        Qt.Key.Key_Delete: "delete",
        Qt.Key.Key_Insert: "insert",
        Qt.Key.Key_Home: "home",
        Qt.Key.Key_End: "end",
        Qt.Key.Key_PageUp: "page up",
        Qt.Key.Key_PageDown: "page down",
        Qt.Key.Key_Up: "up",
        Qt.Key.Key_Down: "down",
        Qt.Key.Key_Left: "left",
        Qt.Key.Key_Right: "right",
        **{getattr(Qt.Key, f"Key_F{i}"): f"f{i}" for i in range(1, 13)},
    }

    def __init__(self, parent=None) -> None:
        super().__init__(parent)
        layout = QHBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)

        self._btn = QPushButton("Click to set hotkey…")
        self._btn.setFocusPolicy(Qt.FocusPolicy.StrongFocus)
        self._btn.installEventFilter(self)
        layout.addWidget(self._btn)

        self._current_hotkey = ""
        self._capturing = False
        self._pending_mods: list[str] = []  # track modifiers during capture

    def set_hotkey(self, hotkey_str: str) -> None:
        """Set the displayed hotkey from keyboard-lib format (e.g. 'ctrl+shift+space')."""
        self._current_hotkey = hotkey_str
        self._btn.setText(hotkey_str if hotkey_str else "Click to set hotkey…")

    def get_hotkey(self) -> str:
        return self._current_hotkey

    def eventFilter(self, obj, event) -> bool:
        if obj is not self._btn:
            return super().eventFilter(obj, event)

        etype = event.type()

        if etype == QEvent.Type.FocusIn:
            if not self._capturing:
                self._capturing = True
                self._pending_mods = []
                self._btn.setText("Press a key combination…")
                self.capture_started.emit()
            return False

        if etype == QEvent.Type.FocusOut:
            if self._capturing:
                self._capturing = False
                # Restore display
                self._btn.setText(
                    self._current_hotkey if self._current_hotkey
                    else "Click to set hotkey…"
                )
                self.capture_ended.emit()
            return False

        if etype == QEvent.Type.KeyPress:
            key = event.key()

            # Modifier-only press — track and show partial state
            if key in self._MODIFIER_KEYS:
                mods = event.modifiers()
                self._pending_mods = [name for qt_mod, name in self._QT_MOD_TO_STR
                                      if mods & qt_mod]
                if self._pending_mods:
                    self._btn.setText("+".join(self._pending_mods) + "+…")
                return True

            # Real key arrived — finalize as modifier(s)+key
            mods = event.modifiers()
            parts = [name for qt_mod, name in self._QT_MOD_TO_STR
                     if mods & qt_mod]

            key_name = self._QT_KEY_TO_STR.get(key)
            if key_name is None:
                text = event.text()
                if text and text.isprintable():
                    key_name = text.lower()
                else:
                    return True  # unknown key, ignore

            parts.append(key_name)
            self._finalize("+".join(parts))
            return True

        # Modifier released — finalize modifier-only hotkey if ≥2 modifiers
        if etype == QEvent.Type.KeyRelease:
            key = event.key()
            if key in self._MODIFIER_KEYS and self._capturing and self._pending_mods:
                remaining = event.modifiers()
                if not remaining and len(self._pending_mods) >= 2:
                    # All modifiers released — use the peak set
                    self._finalize("+".join(self._pending_mods))
                    return True
                elif not remaining:
                    # Only one modifier was used — too easy to trigger accidentally
                    self._pending_mods = []
                    self._btn.setText("Press a key combination…")
            return True

        return super().eventFilter(obj, event)

    def _finalize(self, hotkey: str) -> None:
        """Accept the hotkey, update display, emit signals."""
        self._current_hotkey = hotkey
        self._btn.setText(hotkey)
        self._capturing = False
        self._pending_mods = []
        self.hotkey_changed.emit(hotkey)
        self.capture_ended.emit()
        self._btn.clearFocus()


class SettingsPage(QWidget):
    hotkey_changed = pyqtSignal(str)
    model_changed = pyqtSignal(str, str)
    theme_changed = pyqtSignal(str)
    recording_mode_changed = pyqtSignal(str)
    start_on_login_changed = pyqtSignal(bool)
    # Forwarded from HotkeyCaptureWidget — app.py uses these to pause the listener
    hotkey_capture_started = pyqtSignal()
    hotkey_capture_ended = pyqtSignal()

    def __init__(self, settings: SettingsManager, parent=None) -> None:
        super().__init__(parent)
        self._settings = settings
        self._setup_ui()
        self._load_values()

    def _setup_ui(self) -> None:
        outer = QVBoxLayout(self)
        outer.setContentsMargins(24, 24, 24, 24)
        outer.setSpacing(0)

        title = QLabel("Settings")
        title.setObjectName("page_title")
        outer.addWidget(title)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(scroll.Shape.NoFrame)
        outer.addWidget(scroll)

        container = QWidget()
        scroll.setWidget(container)
        layout = QVBoxLayout(container)
        layout.setSpacing(20)
        layout.setContentsMargins(0, 16, 0, 0)

        layout.addWidget(self._build_recording_group())
        layout.addWidget(self._build_transcription_group())
        layout.addWidget(self._build_injection_group())
        layout.addWidget(self._build_app_group())
        layout.addStretch()

    def _build_recording_group(self) -> QGroupBox:
        group = QGroupBox("Recording")
        form = QFormLayout(group)
        form.setSpacing(12)

        self._hotkey_widget = HotkeyEdit()
        self._hotkey_widget.hotkey_changed.connect(self._on_hotkey_changed)
        self._hotkey_widget.capture_started.connect(self.hotkey_capture_started)
        self._hotkey_widget.capture_ended.connect(self.hotkey_capture_ended)
        form.addRow("Hotkey:", self._hotkey_widget)

        self._mode_combo = QComboBox()
        self._mode_combo.addItem("Push to talk", "push_to_talk")
        self._mode_combo.addItem("Toggle + auto-stop", "toggle")
        self._mode_combo.currentIndexChanged.connect(self._on_mode_changed)
        form.addRow("Recording mode:", self._mode_combo)

        self._silence_spin = QSpinBox()
        self._silence_spin.setRange(500, 10000)
        self._silence_spin.setSuffix(" ms")
        self._silence_spin.valueChanged.connect(lambda v: self._settings.set_vad_silence_ms(v))
        form.addRow("Silence timeout:", self._silence_spin)

        self._device_combo = QComboBox()
        self._populate_audio_devices()
        self._device_combo.currentIndexChanged.connect(self._on_device_changed)
        form.addRow("Audio device:", self._device_combo)

        return group

    def _build_transcription_group(self) -> QGroupBox:
        group = QGroupBox("Transcription")
        form = QFormLayout(group)
        form.setSpacing(12)

        self._model_combo = QComboBox()
        for m in MODELS:
            self._model_combo.addItem(m, m)
        self._model_combo.currentIndexChanged.connect(self._on_model_changed)
        form.addRow("Model:", self._model_combo)

        self._compute_combo = QComboBox()
        for c in COMPUTE_TYPES:
            self._compute_combo.addItem(c, c)
        self._compute_combo.currentIndexChanged.connect(self._on_model_changed)
        form.addRow("Compute type:", self._compute_combo)

        self._lang_combo = QComboBox()
        for label, code in LANGUAGES:
            self._lang_combo.addItem(label, code)
        self._lang_combo.currentIndexChanged.connect(self._on_lang_changed)
        form.addRow("Language:", self._lang_combo)

        self._prompt_edit = QTextEdit()
        self._prompt_edit.setPlaceholderText(
            "Optional context hint, e.g. 'Software engineering discussion.'"
        )
        self._prompt_edit.setFixedHeight(80)
        self._prompt_edit.textChanged.connect(self._on_prompt_changed)
        form.addRow("Context prompt:", self._prompt_edit)

        return group

    def _build_injection_group(self) -> QGroupBox:
        group = QGroupBox("Text Injection")
        form = QFormLayout(group)
        form.setSpacing(12)

        self._delay_spin = QSpinBox()
        self._delay_spin.setRange(50, 2000)
        self._delay_spin.setSuffix(" ms")
        self._delay_spin.setToolTip(
            "Increase if text is not pasting correctly in some apps."
        )
        self._delay_spin.valueChanged.connect(lambda v: self._settings.set_inject_delay_ms(v))
        form.addRow("Inject delay:", self._delay_spin)

        return group

    def _build_app_group(self) -> QGroupBox:
        group = QGroupBox("Application")
        form = QFormLayout(group)
        form.setSpacing(12)

        self._theme_combo = QComboBox()
        self._theme_combo.addItem("Dark", "dark")
        self._theme_combo.addItem("Light", "light")
        self._theme_combo.currentIndexChanged.connect(self._on_theme_changed)
        form.addRow("Theme:", self._theme_combo)

        self._start_min_cb = QCheckBox("Start minimized to tray")
        self._start_min_cb.stateChanged.connect(
            lambda s: self._settings.set_start_minimized(s == Qt.CheckState.Checked.value)
        )
        form.addRow("", self._start_min_cb)

        if sys.platform == "win32":
            self._login_cb = QCheckBox("Start on Windows login")
            self._login_cb.stateChanged.connect(self._on_start_on_login_changed)
            form.addRow("", self._login_cb)
        else:
            self._login_cb = None

        self._log_level_combo = QComboBox()
        self._log_level_combo.addItem("Debug", "debug")
        self._log_level_combo.addItem("Info", "info")
        self._log_level_combo.addItem("Warning", "warning")
        self._log_level_combo.currentIndexChanged.connect(self._on_log_level_changed)
        form.addRow("Log level:", self._log_level_combo)

        return group

    def _load_values(self) -> None:
        s = self._settings
        self._hotkey_widget.set_hotkey(s.get_hotkey())

        idx = self._mode_combo.findData(s.get_recording_mode())
        if idx >= 0:
            self._mode_combo.setCurrentIndex(idx)
        self._silence_spin.setValue(s.get_vad_silence_ms())

        idx = self._model_combo.findData(s.get_model_size())
        if idx >= 0:
            self._model_combo.setCurrentIndex(idx)
        idx = self._compute_combo.findData(s.get_compute_type())
        if idx >= 0:
            self._compute_combo.setCurrentIndex(idx)
        idx = self._lang_combo.findData(s.get_language())
        if idx >= 0:
            self._lang_combo.setCurrentIndex(idx)

        self._prompt_edit.setPlainText(s.get_initial_prompt())
        self._delay_spin.setValue(s.get_inject_delay_ms())

        idx = self._theme_combo.findData(s.get_theme())
        if idx >= 0:
            self._theme_combo.setCurrentIndex(idx)
        self._start_min_cb.setChecked(s.get_start_minimized())
        if self._login_cb:
            self._login_cb.setChecked(s.get_start_on_login())
        idx = self._log_level_combo.findData(s.get_log_level())
        if idx >= 0:
            self._log_level_combo.setCurrentIndex(idx)

    def _on_hotkey_changed(self, hotkey: str) -> None:
        if hotkey:
            self._settings.set_hotkey(hotkey)
            self.hotkey_changed.emit(hotkey)

    def _on_mode_changed(self, _: int) -> None:
        mode = self._mode_combo.currentData()
        self._settings.set_recording_mode(mode)
        self._silence_spin.setEnabled(mode == "toggle")
        self.recording_mode_changed.emit(mode)

    def _on_device_changed(self, idx: int) -> None:
        self._settings.set_audio_device_index(self._device_combo.itemData(idx))

    def _on_model_changed(self, _: int) -> None:
        model = self._model_combo.currentData()
        compute = self._compute_combo.currentData()
        self._settings.set_model_size(model)
        self._settings.set_compute_type(compute)
        self.model_changed.emit(model, compute)

    def _on_lang_changed(self, _: int) -> None:
        self._settings.set_language(self._lang_combo.currentData())

    def _on_prompt_changed(self) -> None:
        self._settings.set_initial_prompt(self._prompt_edit.toPlainText())

    def _on_theme_changed(self, _: int) -> None:
        theme = self._theme_combo.currentData()
        self._settings.set_theme(theme)
        self.theme_changed.emit(theme)

    def _on_log_level_changed(self, _: int) -> None:
        level = self._log_level_combo.currentData()
        self._settings.set_log_level(level)

    def _on_start_on_login_changed(self, state: int) -> None:
        enabled = state == Qt.CheckState.Checked.value
        self._settings.set_start_on_login(enabled)
        from ...utils.platform_utils import set_start_on_login
        set_start_on_login(enabled)
        self.start_on_login_changed.emit(enabled)

    def _populate_audio_devices(self) -> None:
        self._device_combo.addItem("System default", None)
        try:
            for d in AudioRecorder.list_devices():
                self._device_combo.addItem(d["name"], d["index"])
        except Exception:
            pass
        saved = self._settings.get_audio_device_index()
        if saved is not None:
            idx = self._device_combo.findData(saved)
            if idx >= 0:
                self._device_combo.setCurrentIndex(idx)
