"""Floating overlay near the text caret during recording/transcription.

Shows three animated pulsing dots — a modern typing-indicator style.
Recording = red dots, Transcribing = amber dots.
"""

from __future__ import annotations

import math
import sys
import time

from PyQt6.QtCore import QPoint, QRectF, Qt, QTimer
from PyQt6.QtGui import QColor, QCursor, QPainter, QPainterPath, QPen
from PyQt6.QtWidgets import QApplication, QWidget


class CursorOverlay(QWidget):
    """Pulsing-dot overlay anchored near the text caret."""

    _CARET_OFFSET = QPoint(4, 20)
    _MOUSE_OFFSET = QPoint(20, 20)
    _FOLLOW_INTERVAL_MS = 80
    _ANIM_INTERVAL_MS = 30          # ~33 fps for smooth animation
    _DOT_COUNT = 3
    _DOT_RADIUS_MIN = 3.0
    _DOT_RADIUS_MAX = 5.5
    _DOT_SPACING = 16               # center-to-center
    _PHASE_OFFSET = 0.7             # radians between each dot

    def __init__(self, parent=None) -> None:
        super().__init__(parent)
        self.setWindowFlags(
            Qt.WindowType.FramelessWindowHint
            | Qt.WindowType.WindowStaysOnTopHint
            | Qt.WindowType.Tool
            | Qt.WindowType.WindowDoesNotAcceptFocus
        )
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
        self.setAttribute(Qt.WidgetAttribute.WA_ShowWithoutActivating)

        # Widget size just enough for the dots + padding
        pad_x = 14
        width = pad_x * 2 + (self._DOT_COUNT - 1) * self._DOT_SPACING
        height = 28
        self.setFixedSize(int(width), height)

        self._dot_color = QColor("#ef4444")
        self._bg_color = QColor(20, 20, 30, 160)   # subtle dark glass
        self._t0 = 0.0                               # animation start time

        # Position tracking timer
        self._follow_timer = QTimer(self)
        self._follow_timer.setInterval(self._FOLLOW_INTERVAL_MS)
        self._follow_timer.timeout.connect(self._update_position)

        # Animation timer
        self._anim_timer = QTimer(self)
        self._anim_timer.setInterval(self._ANIM_INTERVAL_MS)
        self._anim_timer.timeout.connect(self._tick)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def show_recording(self) -> None:
        self._dot_color = QColor("#ef4444")   # red
        self._show()

    def show_processing(self) -> None:
        self._dot_color = QColor("#f59e0b")   # amber
        self.update()

    def hide_overlay(self) -> None:
        self._follow_timer.stop()
        self._anim_timer.stop()
        self.hide()

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _show(self) -> None:
        self._t0 = time.monotonic()
        self._update_position()
        self.show()
        self._follow_timer.start()
        self._anim_timer.start()

    def _tick(self) -> None:
        self.update()  # triggers paintEvent

    def _update_position(self) -> None:
        caret = self._get_caret_screen_pos()
        if caret is not None:
            pos = caret + self._CARET_OFFSET
            anchor = caret
        else:
            pos = QCursor.pos() + self._MOUSE_OFFSET
            anchor = QCursor.pos()

        screen = QApplication.screenAt(anchor)
        if screen:
            geo = screen.availableGeometry()
            if pos.x() + self.width() > geo.right():
                pos.setX(anchor.x() - self.width() - 10)
            if pos.y() + self.height() > geo.bottom():
                pos.setY(anchor.y() - self.height() - 10)
        self.move(pos)

    def _get_caret_screen_pos(self) -> QPoint | None:
        """Get the screen position of the text caret in the foreground window."""
        if sys.platform != "win32":
            return None
        try:
            import ctypes
            import ctypes.wintypes as wt

            class GUITHREADINFO(ctypes.Structure):
                _fields_ = [
                    ("cbSize", wt.DWORD),
                    ("flags", wt.DWORD),
                    ("hwndActive", wt.HWND),
                    ("hwndFocus", wt.HWND),
                    ("hwndCapture", wt.HWND),
                    ("hwndMenuOwner", wt.HWND),
                    ("hwndMoveSize", wt.HWND),
                    ("hwndCaret", wt.HWND),
                    ("rcCaret", wt.RECT),
                ]

            info = GUITHREADINFO()
            info.cbSize = ctypes.sizeof(GUITHREADINFO)

            if not ctypes.windll.user32.GetGUIThreadInfo(0, ctypes.byref(info)):
                return None
            if not info.hwndCaret:
                return None

            pt = wt.POINT(info.rcCaret.left, info.rcCaret.bottom)
            ctypes.windll.user32.ClientToScreen(info.hwndCaret, ctypes.byref(pt))
            return QPoint(pt.x, pt.y)
        except Exception:
            return None

    # ------------------------------------------------------------------
    # Painting
    # ------------------------------------------------------------------

    def paintEvent(self, event) -> None:
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)

        w, h = self.width(), self.height()

        # Subtle rounded background pill
        bg_path = QPainterPath()
        bg_path.addRoundedRect(QRectF(0, 0, w, h), h / 2, h / 2)
        p.fillPath(bg_path, self._bg_color)

        # Thin border for depth
        p.setPen(QPen(QColor(255, 255, 255, 25), 1.0))
        p.drawPath(bg_path)

        # Animated dots
        t = time.monotonic() - self._t0
        speed = 4.0  # radians per second
        cx_start = (w - (self._DOT_COUNT - 1) * self._DOT_SPACING) / 2
        cy = h / 2

        p.setPen(Qt.PenStyle.NoPen)

        for i in range(self._DOT_COUNT):
            phase = t * speed - i * self._PHASE_OFFSET
            # sin wave mapped to [0, 1]
            wave = (math.sin(phase) + 1.0) / 2.0

            # Radius oscillates between min and max
            r = self._DOT_RADIUS_MIN + wave * (self._DOT_RADIUS_MAX - self._DOT_RADIUS_MIN)

            # Opacity oscillates between 0.4 and 1.0
            alpha = 0.4 + wave * 0.6
            color = QColor(self._dot_color)
            color.setAlphaF(alpha)

            # Subtle vertical bounce (-2 to +2 pixels)
            dy = -2.0 * wave

            cx = cx_start + i * self._DOT_SPACING
            p.setBrush(color)
            p.drawEllipse(QRectF(cx - r, cy + dy - r, r * 2, r * 2))

            # Glow effect — larger, very transparent circle behind the dot
            glow = QColor(self._dot_color)
            glow.setAlphaF(alpha * 0.2)
            gr = r * 2.0
            p.setBrush(glow)
            p.drawEllipse(QRectF(cx - gr, cy + dy - gr, gr * 2, gr * 2))

        p.end()
