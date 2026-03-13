from __future__ import annotations

from PyQt6.QtGui import QIcon
from PyQt6.QtWidgets import QMenu, QSystemTrayIcon

from ..utils.platform_utils import get_assets_dir


class TrayState:
    IDLE = "idle"
    RECORDING = "recording"
    PROCESSING = "processing"
    ERROR = "error"


class TrayIcon(QSystemTrayIcon):
    """System tray icon with context menu.

    States:
      IDLE       — green circle
      RECORDING  — red circle
      PROCESSING — amber circle
      ERROR      — yellow circle
    """

    def __init__(self, parent=None) -> None:
        super().__init__(parent)
        self._icons: dict[str, QIcon] = {}
        self._load_icons()
        self.setIcon(self._icons[TrayState.IDLE])
        self.setToolTip("free-whisper — idle")

        self._menu = QMenu()
        self._setup_menu()
        self.setContextMenu(self._menu)
        self.show()

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def set_state(self, state: str) -> None:
        icon = self._icons.get(state, self._icons[TrayState.IDLE])
        self.setIcon(icon)
        labels = {
            TrayState.IDLE: "free-whisper — idle",
            TrayState.RECORDING: "free-whisper — recording…",
            TrayState.PROCESSING: "free-whisper — transcribing…",
            TrayState.ERROR: "free-whisper — error",
        }
        self.setToolTip(labels.get(state, "free-whisper"))

    def show_notification(self, text: str, duration_ms: int = 3000) -> None:
        preview = text[:80] + "…" if len(text) > 80 else text
        self.showMessage(
            "Transcribed",
            preview,
            QSystemTrayIcon.MessageIcon.Information,
            duration_ms,
        )

    def show_error(self, message: str) -> None:
        self.set_state(TrayState.ERROR)
        self.showMessage(
            "free-whisper — Error",
            message,
            QSystemTrayIcon.MessageIcon.Warning,
            5000,
        )

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    def _load_icons(self) -> None:
        assets = get_assets_dir() / "icons"
        mapping = {
            TrayState.IDLE: "tray_idle.png",
            TrayState.RECORDING: "tray_recording.png",
            TrayState.PROCESSING: "tray_processing.png",
            TrayState.ERROR: "tray_error.png",
        }
        for state, filename in mapping.items():
            path = assets / filename
            if path.exists():
                self._icons[state] = QIcon(str(path))
            else:
                # Fallback: use a colored pixmap generated on the fly
                self._icons[state] = self._make_fallback_icon(state)

    @staticmethod
    def _make_fallback_icon(state: str) -> QIcon:
        from PyQt6.QtCore import Qt
        from PyQt6.QtGui import QColor, QPainter, QPixmap

        color_map = {
            TrayState.IDLE: "#22c55e",
            TrayState.RECORDING: "#ef4444",
            TrayState.PROCESSING: "#f59e0b",
            TrayState.ERROR: "#eab308",
        }
        color = QColor(color_map.get(state, "#22c55e"))
        pix = QPixmap(22, 22)
        pix.fill(Qt.GlobalColor.transparent)
        painter = QPainter(pix)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.setBrush(color)
        painter.setPen(Qt.PenStyle.NoPen)
        painter.drawEllipse(1, 1, 20, 20)
        painter.end()
        return QIcon(pix)

    def _setup_menu(self) -> None:
        self._action_show = self._menu.addAction("Show Window")
        self._menu.addSeparator()
        self._action_settings = self._menu.addAction("Settings")
        self._menu.addSeparator()
        self._action_quit = self._menu.addAction("Quit")

    @property
    def action_show(self):
        return self._action_show

    @property
    def action_settings(self):
        return self._action_settings

    @property
    def action_quit(self):
        return self._action_quit
