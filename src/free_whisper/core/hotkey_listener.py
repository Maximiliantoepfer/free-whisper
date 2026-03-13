from __future__ import annotations

from PyQt6.QtCore import QThread, pyqtSignal

from ..utils.log import get_logger

log = get_logger(__name__)


class HotkeyListener(QThread):
    """Background thread that listens for the global hotkey (OS-level hook).

    set_paused(True) suppresses all callbacks — use while the hotkey
    capture widget is active so the user can type the new combination freely.
    """

    hotkey_pressed = pyqtSignal()
    hotkey_released = pyqtSignal()

    def __init__(self, hotkey: str = "ctrl+shift+space", parent=None) -> None:
        super().__init__(parent)
        self._hotkey = hotkey
        self._running = True
        self._paused = False
        self._is_pressed = False  # tracks whether hotkey is currently held
        self._hook_id = None
        self._release_hook = None

    # ------------------------------------------------------------------
    # Public API (called from main thread)
    # ------------------------------------------------------------------

    def set_paused(self, paused: bool) -> None:
        """Suppress/restore hotkey callbacks. Thread-safe flag."""
        self._paused = paused
        if paused:
            self._is_pressed = False  # cancel any in-flight press
        log.debug("Paused=%s", paused)

    def update_hotkey(self, new_hotkey: str) -> None:
        """Swap the registered hotkey."""
        log.info("Updating hotkey: %r → %r", self._hotkey, new_hotkey)
        self._unregister()
        self._hotkey = new_hotkey
        self._register()

    def stop(self) -> None:
        self._running = False
        self._unregister()
        self.quit()
        self.wait(5_000)

    # ------------------------------------------------------------------
    # QThread.run()
    # ------------------------------------------------------------------

    def run(self) -> None:
        self._register()
        while self._running:
            self.msleep(100)
        self._unregister()

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    def _register(self) -> None:
        import keyboard

        if not self._hotkey:
            return
        try:
            self._hook_id = keyboard.add_hotkey(
                self._hotkey,
                self._on_press,
                suppress=False,
                trigger_on_release=False,
            )
            last_key = self._hotkey.split("+")[-1]
            self._release_hook = keyboard.on_release_key(
                last_key, self._on_key_release
            )
            log.info("Registered hotkey '%s'", self._hotkey)
        except Exception as exc:
            log.error("Failed to register '%s': %s", self._hotkey, exc)

    def _unregister(self) -> None:
        import keyboard

        if self._hook_id is not None:
            try:
                keyboard.remove_hotkey(self._hook_id)
            except Exception:
                pass
            self._hook_id = None
        if self._release_hook is not None:
            try:
                keyboard.unhook(self._release_hook)
            except Exception:
                pass
            self._release_hook = None

    def _on_press(self) -> None:
        if self._paused:
            return
        if not self._is_pressed:
            self._is_pressed = True
            log.debug("hotkey_pressed emitted")
            self.hotkey_pressed.emit()

    def _on_key_release(self, _event) -> None:
        if self._paused:
            return
        if self._is_pressed:
            self._is_pressed = False
            log.debug("hotkey_released emitted")
            self.hotkey_released.emit()
