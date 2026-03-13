from __future__ import annotations

import sys
import time

from ..utils.log import get_logger

log = get_logger(__name__)


class TextInjector:
    """Injects text into the currently focused text field via clipboard paste.

    Strategy: save clipboard → set clipboard to transcribed text →
    simulate Ctrl+V (or Cmd+V on macOS) → restore clipboard.

    This approach handles all Unicode reliably on all platforms.
    """

    def __init__(self, delay_ms: int = 150) -> None:
        self._delay_ms = delay_ms

    def set_delay(self, delay_ms: int) -> None:
        self._delay_ms = delay_ms

    def inject(self, text: str) -> bool:
        """Paste *text* into the focused field. Returns True on success."""
        if not text:
            return False
        try:
            if sys.platform == "win32":
                return self._inject_windows(text)
            elif sys.platform == "darwin":
                return self._inject_macos(text)
            else:
                return self._inject_linux(text)
        except Exception as exc:
            log.error("Injection error: %s", exc)
            return False

    # ------------------------------------------------------------------
    # Platform implementations
    # ------------------------------------------------------------------

    def _inject_windows(self, text: str) -> bool:
        import pyperclip
        import pyautogui

        original = self._safe_get_clipboard()
        try:
            pyperclip.copy(text)
            time.sleep(0.05)
            # pyautogui uses SendInput — works for all normal (non-elevated) windows
            pyautogui.hotkey("ctrl", "v")
            time.sleep(self._delay_ms / 1000)
        finally:
            if original is not None:
                time.sleep(0.05)
                pyperclip.copy(original)
        log.info("Injected %d chars via Ctrl+V", len(text))
        return True

    def _inject_macos(self, text: str) -> bool:
        import pyperclip
        import pyautogui

        original = self._safe_get_clipboard()
        try:
            pyperclip.copy(text)
            time.sleep(0.05)
            pyautogui.hotkey("command", "v")
            time.sleep(self._delay_ms / 1000)
        finally:
            if original is not None:
                time.sleep(0.05)
                pyperclip.copy(original)
        log.info("Injected %d chars via Cmd+V", len(text))
        return True

    def _inject_linux(self, text: str) -> bool:
        import pyperclip
        import pyautogui

        original = self._safe_get_clipboard()
        try:
            pyperclip.copy(text)
            time.sleep(0.05)
            pyautogui.hotkey("ctrl", "v")
            time.sleep(self._delay_ms / 1000)
        finally:
            if original is not None:
                time.sleep(0.05)
                pyperclip.copy(original)
        log.info("Injected %d chars via Ctrl+V", len(text))
        return True

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _safe_get_clipboard() -> str | None:
        try:
            import pyperclip

            return pyperclip.paste()
        except Exception:
            return None
