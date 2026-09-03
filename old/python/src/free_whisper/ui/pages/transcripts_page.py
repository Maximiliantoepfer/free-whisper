from __future__ import annotations

from datetime import datetime
from typing import Any

from PyQt6.QtCore import QAbstractTableModel, QModelIndex, Qt, QTimer
from PyQt6.QtGui import QFont
from PyQt6.QtWidgets import (
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QMessageBox,
    QPushButton,
    QSplitter,
    QTableView,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from ...db.database import Database
from ...db.models import Transcript


class TranscriptTableModel(QAbstractTableModel):
    COLUMNS = ["Time", "Transcript", "Duration", "Model", "Language"]

    def __init__(self, parent=None) -> None:
        super().__init__(parent)
        self._rows: list[Transcript] = []

    def load(self, rows: list[Transcript]) -> None:
        self.beginResetModel()
        self._rows = rows
        self.endResetModel()

    def rowCount(self, parent: QModelIndex = QModelIndex()) -> int:
        return len(self._rows)

    def columnCount(self, parent: QModelIndex = QModelIndex()) -> int:
        return len(self.COLUMNS)

    def headerData(self, section: int, orientation: Qt.Orientation, role: int = Qt.ItemDataRole.DisplayRole) -> Any:
        if role == Qt.ItemDataRole.DisplayRole and orientation == Qt.Orientation.Horizontal:
            return self.COLUMNS[section]
        return None

    def data(self, index: QModelIndex, role: int = Qt.ItemDataRole.DisplayRole) -> Any:
        if not index.isValid() or index.row() >= len(self._rows):
            return None
        row = self._rows[index.row()]
        col = index.column()

        if role == Qt.ItemDataRole.DisplayRole:
            if col == 0:
                return row.created_at.strftime("%Y-%m-%d %H:%M")
            elif col == 1:
                return row.text_preview
            elif col == 2:
                if row.audio_duration_ms:
                    secs = row.audio_duration_ms / 1000
                    return f"{secs:.1f}s"
                return ""
            elif col == 3:
                return row.model_used or ""
            elif col == 4:
                return row.language_detected or "auto"

        if role == Qt.ItemDataRole.UserRole:
            return row  # full object for detail view

        if role == Qt.ItemDataRole.ToolTipRole and col == 1:
            return row.text

        return None

    def get_transcript(self, row: int) -> Transcript | None:
        if 0 <= row < len(self._rows):
            return self._rows[row]
        return None

    def remove_row(self, row: int) -> None:
        if 0 <= row < len(self._rows):
            self.beginRemoveRows(QModelIndex(), row, row)
            self._rows.pop(row)
            self.endRemoveRows()


class TranscriptsPage(QWidget):
    def __init__(self, db: Database, parent=None) -> None:
        super().__init__(parent)
        self._db = db
        self._search_timer = QTimer(self)
        self._search_timer.setSingleShot(True)
        self._search_timer.setInterval(300)
        self._search_timer.timeout.connect(self._run_search)
        self._setup_ui()
        self.refresh()

    def refresh(self) -> None:
        rows = self._db.get_transcripts(limit=100)
        self._model.load(rows)
        count = self._db.get_transcript_count()
        self._count_label.setText(f"{count} transcript{'s' if count != 1 else ''}")

    # ------------------------------------------------------------------
    # UI setup
    # ------------------------------------------------------------------

    def _setup_ui(self) -> None:
        layout = QVBoxLayout(self)
        layout.setContentsMargins(24, 24, 24, 24)
        layout.setSpacing(16)

        # Header
        header = QHBoxLayout()
        title = QLabel("Transcripts")
        title.setObjectName("page_title")
        header.addWidget(title)
        header.addStretch()
        self._count_label = QLabel("")
        self._count_label.setObjectName("status_label")
        header.addWidget(self._count_label)
        layout.addLayout(header)

        # Search bar
        search_row = QHBoxLayout()
        self._search_input = QLineEdit()
        self._search_input.setObjectName("search_bar")
        self._search_input.setPlaceholderText("Search transcripts…")
        self._search_input.textChanged.connect(self._on_search_changed)
        search_row.addWidget(self._search_input)

        clear_btn = QPushButton("Clear")
        clear_btn.setObjectName("secondary_btn")
        clear_btn.setFixedWidth(70)
        clear_btn.clicked.connect(self._search_input.clear)
        search_row.addWidget(clear_btn)
        layout.addLayout(search_row)

        # Splitter: table top, detail bottom
        splitter = QSplitter(Qt.Orientation.Vertical)

        # Table
        self._model = TranscriptTableModel()
        self._table = QTableView()
        self._table.setModel(self._model)
        self._table.setAlternatingRowColors(True)
        self._table.setSelectionBehavior(QTableView.SelectionBehavior.SelectRows)
        self._table.setEditTriggers(QTableView.EditTrigger.NoEditTriggers)
        self._table.verticalHeader().hide()
        self._table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.Stretch)
        self._table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.ResizeToContents)
        self._table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.ResizeToContents)
        self._table.horizontalHeader().setSectionResizeMode(3, QHeaderView.ResizeMode.ResizeToContents)
        self._table.horizontalHeader().setSectionResizeMode(4, QHeaderView.ResizeMode.ResizeToContents)
        self._table.selectionModel().currentRowChanged.connect(self._on_row_selected)
        splitter.addWidget(self._table)

        # Detail panel
        detail_widget = QWidget()
        detail_layout = QVBoxLayout(detail_widget)
        detail_layout.setContentsMargins(0, 8, 0, 0)

        detail_header = QHBoxLayout()
        detail_label = QLabel("Full text")
        detail_label.setFont(QFont("", -1, QFont.Weight.Bold))
        detail_header.addWidget(detail_label)
        detail_header.addStretch()

        self._copy_btn = QPushButton("Copy")
        self._copy_btn.setFixedWidth(70)
        self._copy_btn.setEnabled(False)
        self._copy_btn.clicked.connect(self._copy_selected)
        detail_header.addWidget(self._copy_btn)

        self._delete_btn = QPushButton("Delete")
        self._delete_btn.setObjectName("danger_btn")
        self._delete_btn.setFixedWidth(70)
        self._delete_btn.setEnabled(False)
        self._delete_btn.clicked.connect(self._delete_selected)
        detail_header.addWidget(self._delete_btn)

        detail_layout.addLayout(detail_header)

        self._detail_text = QTextEdit()
        self._detail_text.setReadOnly(True)
        self._detail_text.setPlaceholderText("Select a transcript to view the full text…")
        self._detail_text.setMaximumHeight(150)
        detail_layout.addWidget(self._detail_text)

        splitter.addWidget(detail_widget)
        splitter.setStretchFactor(0, 3)
        splitter.setStretchFactor(1, 1)
        layout.addWidget(splitter)

    # ------------------------------------------------------------------
    # Slots
    # ------------------------------------------------------------------

    def _on_search_changed(self, text: str) -> None:
        self._search_timer.start()

    def _run_search(self) -> None:
        query = self._search_input.text().strip()
        if query:
            try:
                rows = self._db.search_transcripts(query)
            except Exception:
                rows = []
        else:
            rows = self._db.get_transcripts(limit=100)
        self._model.load(rows)

    def _on_row_selected(self, current: QModelIndex, previous: QModelIndex) -> None:
        t = self._model.get_transcript(current.row())
        if t:
            self._detail_text.setPlainText(t.text)
            self._copy_btn.setEnabled(True)
            self._delete_btn.setEnabled(True)
        else:
            self._detail_text.clear()
            self._copy_btn.setEnabled(False)
            self._delete_btn.setEnabled(False)

    def _copy_selected(self) -> None:
        row = self._table.currentIndex().row()
        t = self._model.get_transcript(row)
        if t:
            from PyQt6.QtWidgets import QApplication

            QApplication.clipboard().setText(t.text)

    def _delete_selected(self) -> None:
        row = self._table.currentIndex().row()
        t = self._model.get_transcript(row)
        if not t:
            return
        reply = QMessageBox.question(
            self,
            "Delete transcript",
            "Delete this transcript permanently?",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
        )
        if reply == QMessageBox.StandardButton.Yes:
            self._db.delete_transcript(t.id)
            self._model.remove_row(row)
            self._detail_text.clear()
            self._copy_btn.setEnabled(False)
            self._delete_btn.setEnabled(False)
            count = self._db.get_transcript_count()
            self._count_label.setText(f"{count} transcript{'s' if count != 1 else ''}")
