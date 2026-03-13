from __future__ import annotations

import signal
import sys


def main() -> None:
    from .utils.log import setup_logging
    from .utils.platform_utils import acquire_single_instance_lock

    # Initialise logging early (default level, overridden after settings load)
    setup_logging("info")

    if not acquire_single_instance_lock():
        print("free-whisper is already running.")
        sys.exit(0)

    # Windows: set AppUserModelID so the taskbar groups the window with our
    # icon instead of the generic Python icon.  Must be called before the
    # QApplication is created.
    if sys.platform == "win32":
        try:
            import ctypes
            ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID(
                "com.free-whisper.app"
            )
        except Exception:
            pass

    from PyQt6.QtCore import QTimer
    from .ui.app import FreeWhisperApp

    app = FreeWhisperApp(sys.argv)

    # Allow Ctrl+C in the terminal to quit the app cleanly.
    # Qt's event loop blocks Python signal delivery, so we use a short timer
    # to let the interpreter check for pending signals periodically.
    def _sigint_handler(sig, frame):
        app.quit_app()

    signal.signal(signal.SIGINT, _sigint_handler)
    _sig_timer = QTimer()
    _sig_timer.start(200)
    _sig_timer.timeout.connect(lambda: None)

    sys.exit(app.exec())


if __name__ == "__main__":
    main()
