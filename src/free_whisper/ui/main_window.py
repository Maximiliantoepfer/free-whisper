from __future__ import annotations

from PyQt6.QtCore import QSize, Qt, pyqtSignal
from PyQt6.QtGui import QFont, QIcon
from PyQt6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QMainWindow,
    QPushButton,
    QStackedWidget,
    QVBoxLayout,
    QWidget,
)

from ..db.database import Database
from ..utils.settings_manager import SettingsManager
from ..utils.platform_utils import get_assets_dir
from .pages.transcripts_page import TranscriptsPage
from .pages.dictionary_page import DictionaryPage
from .pages.settings_page import SettingsPage


class MainWindow(QMainWindow):
    hotkey_changed = pyqtSignal(str)
    model_changed = pyqtSignal(str, str)
    theme_changed = pyqtSignal(str)

    def __init__(self, db: Database, settings: SettingsManager, parent=None) -> None:
        super().__init__(parent)
        self._db = db
        self._settings = settings
        self.setWindowTitle("free-whisper")
        self.setMinimumSize(800, 560)
        self.resize(960, 640)

        # Set app icon
        icon_path = get_assets_dir() / "icons" / "app_icon.png"
        if icon_path.exists():
            self.setWindowIcon(QIcon(str(icon_path)))

        self._setup_ui()

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def show_page(self, name: str) -> None:
        mapping = {"transcripts": 0, "dictionary": 1, "settings": 2}
        idx = mapping.get(name, 0)
        self._stack.setCurrentIndex(idx)
        self._update_nav_buttons(idx)

    def refresh_transcripts(self) -> None:
        self._transcripts_page.refresh()

    # ------------------------------------------------------------------
    # UI setup
    # ------------------------------------------------------------------

    def _setup_ui(self) -> None:
        central = QWidget()
        self.setCentralWidget(central)
        root = QHBoxLayout(central)
        root.setContentsMargins(0, 0, 0, 0)
        root.setSpacing(0)

        # Sidebar
        sidebar = self._build_sidebar()
        root.addWidget(sidebar)

        # Content
        self._stack = QStackedWidget()
        self._stack.setObjectName("content_area")

        self._transcripts_page = TranscriptsPage(self._db)
        self._dictionary_page = DictionaryPage(self._db)
        self._settings_page = SettingsPage(self._settings)

        self._stack.addWidget(self._transcripts_page)  # 0
        self._stack.addWidget(self._dictionary_page)   # 1
        self._stack.addWidget(self._settings_page)     # 2

        # Wire settings signals through to app
        self._settings_page.hotkey_changed.connect(self.hotkey_changed)
        self._settings_page.model_changed.connect(self.model_changed)
        self._settings_page.theme_changed.connect(self.theme_changed)

        root.addWidget(self._stack, 1)

    def _build_sidebar(self) -> QWidget:
        sidebar = QWidget()
        sidebar.setObjectName("sidebar")
        layout = QVBoxLayout(sidebar)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(4)

        # App title
        title = QLabel("free-whisper")
        title.setObjectName("app_title")
        layout.addWidget(title)

        version = QLabel("v0.1.0")
        version.setObjectName("version_label")
        layout.addWidget(version)

        # Nav buttons
        self._nav_buttons: list[QPushButton] = []

        for icon_name, label, page_idx in [
            ("📝", "Transcripts", 0),
            ("📖", "Dictionary", 1),
            ("⚙️", "Settings", 2),
        ]:
            btn = QPushButton(f"  {icon_name}  {label}")
            btn.setCheckable(True)
            btn.setChecked(page_idx == 0)
            btn.clicked.connect(lambda checked, idx=page_idx: self._switch_page(idx))
            layout.addWidget(btn)
            self._nav_buttons.append(btn)

        layout.addStretch()

        # Recording status indicator
        self._status_label = QLabel("● Idle")
        self._status_label.setObjectName("status_label")
        self._status_label.setContentsMargins(16, 0, 16, 16)
        layout.addWidget(self._status_label)

        return sidebar

    # ------------------------------------------------------------------
    # Slots / helpers
    # ------------------------------------------------------------------

    def _switch_page(self, idx: int) -> None:
        self._stack.setCurrentIndex(idx)
        self._update_nav_buttons(idx)
        # Refresh data when switching to dynamic pages
        if idx == 0:
            self._transcripts_page.refresh()
        elif idx == 1:
            self._dictionary_page.refresh()

    def _update_nav_buttons(self, active: int) -> None:
        for i, btn in enumerate(self._nav_buttons):
            btn.setChecked(i == active)

    def set_status(self, text: str, color: str = "#606080") -> None:
        self._status_label.setText(text)
        self._status_label.setStyleSheet(f"color: {color}; padding: 0 16px 16px 16px;")

    # ------------------------------------------------------------------
    # Close → hide (keep alive in tray)
    # ------------------------------------------------------------------

    def closeEvent(self, event) -> None:
        event.ignore()
        self.hide()
