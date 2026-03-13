from __future__ import annotations

from PyQt6.QtCore import Qt
from PyQt6.QtWidgets import (
    QDialog,
    QDialogButtonBox,
    QFormLayout,
    QHBoxLayout,
    QInputDialog,
    QLabel,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QMessageBox,
    QPushButton,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from ...db.database import Database
from ...db.models import DictEntry


class DictionaryPage(QWidget):
    def __init__(self, db: Database, parent=None) -> None:
        super().__init__(parent)
        self._db = db
        self._setup_ui()
        self.refresh()

    def refresh(self) -> None:
        self._list.clear()
        entries = self._db.get_dictionary()
        for entry in entries:
            display = entry.word
            if entry.category:
                display += f"  [{entry.category}]"
            item = QListWidgetItem(display)
            item.setData(Qt.ItemDataRole.UserRole, entry)
            self._list.addItem(item)
        self._update_count()

    # ------------------------------------------------------------------
    # UI setup
    # ------------------------------------------------------------------

    def _setup_ui(self) -> None:
        layout = QVBoxLayout(self)
        layout.setContentsMargins(24, 24, 24, 24)
        layout.setSpacing(16)

        # Header
        header = QHBoxLayout()
        title = QLabel("Dictionary")
        title.setObjectName("page_title")
        header.addWidget(title)
        header.addStretch()
        self._count_label = QLabel("")
        self._count_label.setObjectName("status_label")
        header.addWidget(self._count_label)
        layout.addLayout(header)

        hint = QLabel(
            "Words in this dictionary are passed to Whisper as vocabulary hints, "
            "improving recognition of technical terms, names, and project-specific words."
        )
        hint.setWordWrap(True)
        hint.setObjectName("status_label")
        layout.addWidget(hint)

        # Add word row
        add_row = QHBoxLayout()
        self._word_input = QLineEdit()
        self._word_input.setPlaceholderText("Add a word or phrase…")
        self._word_input.returnPressed.connect(self._add_word)
        add_row.addWidget(self._word_input)

        self._category_input = QLineEdit()
        self._category_input.setPlaceholderText("Category (optional)")
        self._category_input.setFixedWidth(160)
        add_row.addWidget(self._category_input)

        add_btn = QPushButton("Add")
        add_btn.setFixedWidth(70)
        add_btn.clicked.connect(self._add_word)
        add_row.addWidget(add_btn)
        layout.addLayout(add_row)

        # List
        self._list = QListWidget()
        self._list.itemDoubleClicked.connect(self._edit_entry)
        layout.addWidget(self._list)

        # Buttons row
        btn_row = QHBoxLayout()
        remove_btn = QPushButton("Remove selected")
        remove_btn.setObjectName("danger_btn")
        remove_btn.clicked.connect(self._remove_selected)
        btn_row.addWidget(remove_btn)

        btn_row.addStretch()

        bulk_btn = QPushButton("Bulk import…")
        bulk_btn.setObjectName("secondary_btn")
        bulk_btn.clicked.connect(self._bulk_import)
        btn_row.addWidget(bulk_btn)

        export_btn = QPushButton("Export…")
        export_btn.setObjectName("secondary_btn")
        export_btn.clicked.connect(self._export)
        btn_row.addWidget(export_btn)

        layout.addLayout(btn_row)

    # ------------------------------------------------------------------
    # Slots
    # ------------------------------------------------------------------

    def _add_word(self) -> None:
        word = self._word_input.text().strip()
        if not word:
            return
        category = self._category_input.text().strip()
        try:
            self._db.add_word(word, category)
            self._word_input.clear()
            self._category_input.clear()
            self.refresh()
        except Exception as exc:
            QMessageBox.warning(self, "Error", str(exc))

    def _remove_selected(self) -> None:
        items = self._list.selectedItems()
        if not items:
            return
        reply = QMessageBox.question(
            self,
            "Remove words",
            f"Remove {len(items)} word(s) from the dictionary?",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
        )
        if reply != QMessageBox.StandardButton.Yes:
            return
        for item in items:
            entry: DictEntry = item.data(Qt.ItemDataRole.UserRole)
            self._db.remove_word(entry.id)
        self.refresh()

    def _edit_entry(self, item: QListWidgetItem) -> None:
        entry: DictEntry = item.data(Qt.ItemDataRole.UserRole)
        dialog = _EditWordDialog(entry.word, entry.category, self)
        if dialog.exec() == QDialog.DialogCode.Accepted:
            new_word, new_cat = dialog.get_values()
            if new_word:
                self._db.update_word(entry.id, new_word, new_cat)
                self.refresh()

    def _bulk_import(self) -> None:
        dialog = _BulkImportDialog(self)
        if dialog.exec() == QDialog.DialogCode.Accepted:
            text = dialog.get_text()
            category = dialog.get_category()
            added = 0
            for line in text.splitlines():
                word = line.strip().strip(",")
                if word:
                    try:
                        self._db.add_word(word, category)
                        added += 1
                    except Exception:
                        pass
            self.refresh()
            QMessageBox.information(self, "Import complete", f"Added {added} word(s).")

    def _export(self) -> None:
        entries = self._db.get_dictionary()
        lines = [f"{e.word},{e.category}" for e in entries]
        dialog = QDialog(self)
        dialog.setWindowTitle("Export dictionary")
        dialog.resize(400, 300)
        lay = QVBoxLayout(dialog)
        te = QTextEdit()
        te.setPlainText("\n".join(lines))
        te.setReadOnly(True)
        lay.addWidget(QLabel("Copy the text below:"))
        lay.addWidget(te)
        btns = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        btns.rejected.connect(dialog.reject)
        lay.addWidget(btns)
        dialog.exec()

    def _update_count(self) -> None:
        n = self._list.count()
        self._count_label.setText(f"{n} word{'s' if n != 1 else ''}")


class _EditWordDialog(QDialog):
    def __init__(self, word: str, category: str, parent=None) -> None:
        super().__init__(parent)
        self.setWindowTitle("Edit word")
        self.setFixedWidth(320)
        lay = QFormLayout(self)
        self._word_edit = QLineEdit(word)
        self._cat_edit = QLineEdit(category)
        lay.addRow("Word:", self._word_edit)
        lay.addRow("Category:", self._cat_edit)
        btns = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel
        )
        btns.accepted.connect(self.accept)
        btns.rejected.connect(self.reject)
        lay.addRow(btns)

    def get_values(self) -> tuple[str, str]:
        return self._word_edit.text().strip(), self._cat_edit.text().strip()


class _BulkImportDialog(QDialog):
    def __init__(self, parent=None) -> None:
        super().__init__(parent)
        self.setWindowTitle("Bulk import")
        self.resize(400, 300)
        lay = QVBoxLayout(self)
        lay.addWidget(QLabel("Enter one word per line:"))
        self._text_edit = QTextEdit()
        self._text_edit.setPlaceholderText("kubernetes\nDocker\nCI/CD\nDr. Smith")
        lay.addWidget(self._text_edit)
        self._cat_edit = QLineEdit()
        self._cat_edit.setPlaceholderText("Category for all (optional)")
        lay.addWidget(self._cat_edit)
        btns = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel
        )
        btns.accepted.connect(self.accept)
        btns.rejected.connect(self.reject)
        lay.addWidget(btns)

    def get_text(self) -> str:
        return self._text_edit.toPlainText()

    def get_category(self) -> str:
        return self._cat_edit.text().strip()
