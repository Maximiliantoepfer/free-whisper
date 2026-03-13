from __future__ import annotations

import sys
import time
from pathlib import Path

from PyQt6.QtCore import QTimer, Qt, pyqtSlot
from PyQt6.QtWidgets import QApplication, QMessageBox, QSystemTrayIcon

from ..core.audio_recorder import AudioRecorder
from ..core.hotkey_listener import HotkeyListener
from ..core.injector import TextInjector
from ..core.transcriber import TranscribeJob, TranscriberWorker
from ..db.database import Database
from ..utils.log import get_logger, setup_logging
from ..utils.platform_utils import get_assets_dir, get_db_path
from ..utils.settings_manager import SettingsManager
from .main_window import MainWindow
from .tray_icon import TrayIcon, TrayState
from .widgets.cursor_overlay import CursorOverlay

log = get_logger(__name__)


class FreeWhisperApp(QApplication):
    def __init__(self, argv: list[str]) -> None:
        super().__init__(argv)
        self.setApplicationName("free-whisper")
        self.setOrganizationName("free-whisper")
        self.setQuitOnLastWindowClosed(False)

        # Load stylesheet
        self._load_stylesheet()

        # Core services
        self._db = Database(get_db_path())
        self._settings = SettingsManager()

        # Apply log level from settings
        setup_logging(self._settings.get_log_level())

        self._injector = TextInjector(self._settings.get_inject_delay_ms())
        self._recorder = AudioRecorder(self._settings.get_audio_device_index())

        # Workers
        self._transcriber = TranscriberWorker()
        self._transcriber.transcription_ready.connect(self._on_transcription_ready)
        self._transcriber.transcription_failed.connect(self._on_transcription_failed)
        self._transcriber.model_loading.connect(self._on_model_loading)
        self._transcriber.model_ready.connect(self._on_model_ready)
        self._transcriber.model_load_failed.connect(self._on_model_load_failed)
        self._transcriber.start()

        self._hotkey_listener = HotkeyListener(self._settings.get_hotkey())
        self._hotkey_listener.hotkey_pressed.connect(self._on_hotkey_pressed)
        self._hotkey_listener.hotkey_released.connect(self._on_hotkey_released)
        self._hotkey_listener.start()

        # UI
        self._tray = TrayIcon()
        self._tray.activated.connect(self._on_tray_activated)
        self._tray.action_show.triggered.connect(self._show_window)
        self._tray.action_settings.triggered.connect(self._show_settings)
        self._tray.action_quit.triggered.connect(self.quit_app)

        self._overlay = CursorOverlay()

        self._window = MainWindow(self._db, self._settings)
        self._window.hotkey_changed.connect(self._hotkey_listener.update_hotkey)
        self._window.model_changed.connect(self._on_model_settings_changed)
        self._window.theme_changed.connect(self._load_stylesheet)

        # Pause the global hotkey while the user is capturing a new one
        sp = self._window._settings_page
        sp.hotkey_capture_started.connect(
            lambda: self._hotkey_listener.set_paused(True)
        )
        sp.hotkey_capture_ended.connect(
            lambda: self._hotkey_listener.set_paused(False)
        )

        # VAD auto-stop timer (for toggle mode)
        self._vad_timer = QTimer(self)
        self._vad_timer.setSingleShot(True)
        self._vad_timer.timeout.connect(self._auto_stop_recording)

        # Settings observer
        self._settings.settings_changed.connect(self._on_setting_changed)

        self._job_counter = 0

        # Show window on startup — minimized to taskbar if requested
        start_min = self._settings.get_start_minimized()
        log.info("start_minimized=%s", start_min)
        if start_min:
            self._window.showMinimized()
        else:
            self._window.show()
            self._window.raise_()
            self._window.activateWindow()

        log.info("App initialised  hotkey=%s  model=%s",
                 self._settings.get_hotkey(), self._settings.get_model_size())

    # ------------------------------------------------------------------
    # Recording state machine
    # ------------------------------------------------------------------

    @pyqtSlot()
    def _on_hotkey_pressed(self) -> None:
        mode = self._settings.get_recording_mode()
        if mode == "push_to_talk":
            if not self._recorder.is_recording:
                self._start_recording()
        else:  # toggle
            if self._recorder.is_recording:
                self._stop_and_transcribe()
            else:
                self._start_recording()
                # Start VAD auto-stop timer
                self._vad_timer.start(self._settings.get_vad_silence_ms() * 3)

    @pyqtSlot()
    def _on_hotkey_released(self) -> None:
        if self._settings.get_recording_mode() == "push_to_talk":
            if self._recorder.is_recording:
                self._stop_and_transcribe()

    def _start_recording(self) -> None:
        log.info("Recording started")
        self._recorder.set_device(self._settings.get_audio_device_index())
        self._recorder.start()
        self._tray.set_state(TrayState.RECORDING)
        self._overlay.show_recording()
        self._window.set_status("● Recording", "#ef4444")

    def _stop_and_transcribe(self) -> None:
        self._vad_timer.stop()
        self._recorder.stop()
        audio = self._recorder.get_audio()
        log.info("Recording stopped — %d samples (%.1fs)",
                 audio.size, audio.size / 16000)

        if audio.size == 0:
            self._tray.set_state(TrayState.IDLE)
            self._overlay.hide_overlay()
            self._window.set_status("● Idle", "#606080")
            return

        duration_ms = self._recorder.duration_ms
        self._tray.set_state(TrayState.PROCESSING)
        self._overlay.show_processing()
        self._window.set_status("⏳ Transcribing…", "#f59e0b")

        self._job_counter += 1
        job = TranscribeJob(
            audio=audio,
            audio_duration_ms=duration_ms,
            model_size=self._settings.get_model_size(),
            compute_type=self._settings.get_compute_type(),
            language=self._settings.get_language(),
            hotwords=self._db.get_hotwords_string(),
            initial_prompt=self._settings.get_initial_prompt(),
            job_id=self._job_counter,
        )
        self._transcriber.enqueue(job)

    @pyqtSlot()
    def _auto_stop_recording(self) -> None:
        """Called when toggle mode silence timer fires."""
        if self._recorder.is_recording:
            self._stop_and_transcribe()

    # ------------------------------------------------------------------
    # Transcriber slots
    # ------------------------------------------------------------------

    @pyqtSlot(str, int, int)
    def _on_transcription_ready(self, text: str, duration_ms: int, job_id: int) -> None:
        self._tray.set_state(TrayState.IDLE)
        self._overlay.hide_overlay()
        self._window.set_status("● Idle", "#606080")

        if not text:
            log.info("Transcription returned empty text (job %d)", job_id)
            return

        log.info("Transcription ready (job %d): %d chars", job_id, len(text))

        # Save to DB
        self._db.save_transcript(
            text=text,
            audio_duration_ms=duration_ms,
            model_used=self._settings.get_model_size(),
            language_detected=self._settings.get_language() or "auto",
        )

        # Update settings (inject delay may have changed)
        self._injector.set_delay(self._settings.get_inject_delay_ms())

        # Inject into focused field
        self._injector.inject(text)

        # Notification
        self._tray.show_notification(text)

        # Refresh UI if window is visible
        if self._window.isVisible():
            self._window.refresh_transcripts()

    @pyqtSlot(str, int)
    def _on_transcription_failed(self, error: str, job_id: int) -> None:
        log.warning("Transcription failed (job %d): %s", job_id, error)
        self._tray.set_state(TrayState.ERROR)
        self._overlay.hide_overlay()
        self._window.set_status("⚠ Error", "#eab308")
        self._tray.show_error(f"Transcription failed: {error}")
        # Reset to idle after a delay
        QTimer.singleShot(5000, lambda: (
            self._tray.set_state(TrayState.IDLE),
            self._window.set_status("● Idle", "#606080"),
        ))

    @pyqtSlot(str)
    def _on_model_loading(self, model_size: str) -> None:
        log.info("Model loading: %s", model_size)
        self._tray.set_state(TrayState.PROCESSING)
        self._window.set_status(f"⏳ Loading {model_size}…", "#f59e0b")

    @pyqtSlot(str)
    def _on_model_ready(self, model_size: str) -> None:
        log.info("Model ready: %s", model_size)
        self._tray.set_state(TrayState.IDLE)
        self._window.set_status("● Idle", "#606080")

    @pyqtSlot(str)
    def _on_model_load_failed(self, error: str) -> None:
        log.error("Model load failed: %s", error)
        self._tray.set_state(TrayState.ERROR)
        self._window.set_status("⚠ Model error", "#eab308")
        self._tray.show_error(f"Model load failed: {error}")

    # ------------------------------------------------------------------
    # Settings changes
    # ------------------------------------------------------------------

    @pyqtSlot(str, str)
    def _on_model_settings_changed(self, model_size: str, compute_type: str) -> None:
        self._transcriber.reload_model(model_size, compute_type)

    @pyqtSlot(str)
    def _on_setting_changed(self, key: str) -> None:
        if key == "inject_delay_ms":
            self._injector.set_delay(self._settings.get_inject_delay_ms())
        elif key == "log_level":
            setup_logging(self._settings.get_log_level())

    # ------------------------------------------------------------------
    # Tray / window
    # ------------------------------------------------------------------

    @pyqtSlot(QSystemTrayIcon.ActivationReason)
    def _on_tray_activated(self, reason) -> None:
        # Only toggle on single-click or double-click, not context menu
        if reason not in (
            QSystemTrayIcon.ActivationReason.Trigger,
            QSystemTrayIcon.ActivationReason.DoubleClick,
        ):
            return
        if self._window.isVisible() and not self._window.isMinimized():
            self._window.hide()
        else:
            self._window.showNormal()
            self._window.raise_()
            self._window.activateWindow()

    def _show_window(self) -> None:
        self._window.show()
        self._window.raise_()
        self._window.activateWindow()

    def _show_settings(self) -> None:
        self._show_window()
        self._window.show_page("settings")

    def quit_app(self) -> None:
        log.info("Shutting down")
        self._hotkey_listener.stop()
        self._transcriber.stop()
        self._recorder.stop()
        self._db.close()
        self.quit()

    # ------------------------------------------------------------------
    # Stylesheet
    # ------------------------------------------------------------------

    @pyqtSlot()
    @pyqtSlot(str)
    def _load_stylesheet(self, theme: str | None = None) -> None:
        if theme is None:
            theme = self._settings.get_theme() if hasattr(self, "_settings") else "dark"

        qss_name = "dark_theme.qss"  # Only dark theme for now
        qss_path = get_assets_dir() / "styles" / qss_name
        if qss_path.exists():
            self.setStyleSheet(qss_path.read_text(encoding="utf-8"))
