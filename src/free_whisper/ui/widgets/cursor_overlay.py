"""Floating overlay near the text caret during recording/transcription.

Shows three small pulsing dots — minimal, unobtrusive typing indicator.
Anchored to the text caret when available, otherwise centred at the
bottom of the current screen.
"""

from __future__ import annotations

import math
import sys
import time

from PyQt6.QtCore import QPoint, QRectF, Qt, QTimer
from PyQt6.QtGui import QColor, QPainter, QPainterPath
from PyQt6.QtWidgets import QApplication, QWidget


class CursorOverlay(QWidget):
    """Minimal pulsing-dot overlay anchored near the text caret."""

    _CARET_OFFSET = QPoint(4, 20)
    _FOLLOW_INTERVAL_MS = 80
    _ANIM_INTERVAL_MS = 30          # ~33 fps
    _DOT_COUNT = 3
    _DOT_RADIUS = 3.0               # constant size — no pulsing resize
    _DOT_SPACING = 12               # tighter spacing for compact look
    _PHASE_OFFSET = 0.8             # radians between each dot

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

        # Widget size — compact pill
        pad_x = 10
        width = pad_x * 2 + (self._DOT_COUNT - 1) * self._DOT_SPACING
        height = 22
        self.setFixedSize(int(width), height)

        self._dot_color = QColor(255, 255, 255)       # neutral white dots
        self._bg_color = QColor(28, 28, 30, 140)      # very transparent dark
        self._t0 = 0.0

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
        self._dot_color = QColor(255, 255, 255)        # white
        self._show()

    def show_processing(self) -> None:
        self._dot_color = QColor(200, 200, 205)        # slightly dimmer
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
            # Clamp to screen edges
            screen = QApplication.screenAt(anchor)
            if screen:
                geo = screen.availableGeometry()
                if pos.x() + self.width() > geo.right():
                    pos.setX(anchor.x() - self.width() - 10)
                if pos.y() + self.height() > geo.bottom():
                    pos.setY(anchor.y() - self.height() - 10)
            self.move(pos)
        else:
            # Fallback: bottom-centre of the current screen
            screen = QApplication.screenAt(self.pos())
            if screen is None:
                screen = QApplication.primaryScreen()
            if screen:
                geo = screen.availableGeometry()
                x = geo.center().x() - self.width() // 2
                y = geo.bottom() - self.height() - 48   # 48px above taskbar
                self.move(x, y)

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

        # Animated dots — opacity-only pulse, no size change, no bounce
        t = time.monotonic() - self._t0
        speed = 3.0  # slower, calmer
        cx_start = (w - (self._DOT_COUNT - 1) * self._DOT_SPACING) / 2
        cy = h / 2
        r = self._DOT_RADIUS

        p.setPen(Qt.PenStyle.NoPen)

        for i in range(self._DOT_COUNT):
            phase = t * speed - i * self._PHASE_OFFSET
            wave = (math.sin(phase) + 1.0) / 2.0

            # Gentle opacity oscillation (0.3 → 0.85)
            alpha = 0.3 + wave * 0.55
            color = QColor(self._dot_color)
            color.setAlphaF(alpha)

            cx = cx_start + i * self._DOT_SPACING
            p.setBrush(color)
            p.drawEllipse(QRectF(cx - r, cy - r, r * 2, r * 2))

        p.end()
