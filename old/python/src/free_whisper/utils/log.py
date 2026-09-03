"""Centralised logging for free-whisper.

Usage in any module::

    from free_whisper.utils.log import get_logger
    log = get_logger(__name__)
    log.info("Model loaded")
    log.debug("Detailed info: %s", data)

Call ``setup_logging()`` once at startup (from main.py) to configure
handlers, format, rotation and the initial level.
"""

from __future__ import annotations

import logging
import sys
from logging.handlers import RotatingFileHandler
from pathlib import Path

_LOG_FORMAT = "%(asctime)s [%(levelname)-5s] %(name)s: %(message)s"
_DATE_FORMAT = "%H:%M:%S"
_MAX_BYTES = 2 * 1024 * 1024  # 2 MB per file
_BACKUP_COUNT = 2              # keep 2 old files → max 6 MB on disk

_initialised = False


def setup_logging(level: str = "info", log_dir: Path | None = None) -> None:
    """Configure root logger with console + rotating file handler.

    *level* is one of ``"debug"``, ``"info"``, ``"warning"``.
    """
    global _initialised
    if _initialised:
        # Allow level change at runtime
        set_level(level)
        return

    numeric = _parse_level(level)
    root = logging.getLogger("free_whisper")
    root.setLevel(numeric)

    formatter = logging.Formatter(_LOG_FORMAT, datefmt=_DATE_FORMAT)

    # Console handler (stderr)
    console = logging.StreamHandler(sys.stderr)
    console.setFormatter(formatter)
    root.addHandler(console)

    # File handler (rotating)
    if log_dir is None:
        from .platform_utils import get_app_data_dir
        log_dir = get_app_data_dir() / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    file_path = log_dir / "free_whisper.log"

    file_handler = RotatingFileHandler(
        file_path,
        maxBytes=_MAX_BYTES,
        backupCount=_BACKUP_COUNT,
        encoding="utf-8",
    )
    file_handler.setFormatter(formatter)
    root.addHandler(file_handler)

    _initialised = True
    root.info("Logging initialised  level=%s  file=%s", level, file_path)


def set_level(level: str) -> None:
    """Change the log level at runtime."""
    numeric = _parse_level(level)
    logging.getLogger("free_whisper").setLevel(numeric)


def get_logger(name: str) -> logging.Logger:
    """Return a child logger under the ``free_whisper`` namespace."""
    if not name.startswith("free_whisper"):
        name = f"free_whisper.{name}"
    return logging.getLogger(name)


def _parse_level(level: str) -> int:
    return getattr(logging, level.upper(), logging.INFO)
