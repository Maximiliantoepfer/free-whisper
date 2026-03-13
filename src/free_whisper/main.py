from __future__ import annotations

import sys


def main() -> None:
    from .utils.log import setup_logging
    from .utils.platform_utils import acquire_single_instance_lock

    # Initialise logging early (default level, overridden after settings load)
    setup_logging("info")

    if not acquire_single_instance_lock():
        print("free-whisper is already running.")
        sys.exit(0)

    from .ui.app import FreeWhisperApp

    app = FreeWhisperApp(sys.argv)
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
