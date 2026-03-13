"""Generate simple PNG tray icons using only stdlib (no Pillow required).

Run once: python scripts/generate_icons.py
"""
import struct
import zlib
from pathlib import Path


def _make_png(width: int, height: int, rgba_data: bytes) -> bytes:
    """Create a minimal valid PNG from raw RGBA bytes."""
    def chunk(name: bytes, data: bytes) -> bytes:
        c = name + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))

    raw_rows = b""
    for y in range(height):
        raw_rows += b"\x00"  # filter type None
        raw_rows += rgba_data[y * width * 4 : (y + 1) * width * 4]

    idat = chunk(b"IDAT", zlib.compress(raw_rows))
    iend = chunk(b"IEND", b"")
    return sig + ihdr + idat + iend


def _circle_rgba(size: int, r: int, g: int, b: int, a: int = 255) -> bytes:
    """Draw a filled circle of the given colour on a transparent background."""
    cx = cy = size / 2
    radius = size / 2 - 1
    data = bytearray(size * size * 4)
    for y in range(size):
        for x in range(size):
            dx, dy = x - cx, y - cy
            if dx * dx + dy * dy <= radius * radius:
                i = (y * size + x) * 4
                data[i], data[i + 1], data[i + 2], data[i + 3] = r, g, b, a
    return bytes(data)


def _mic_rgba(size: int, fill_r: int, fill_g: int, fill_b: int) -> bytes:
    """Very simple microphone silhouette on transparent background."""
    data = bytearray(size * size * 4)
    s = size / 32  # scale factor based on 32px design

    def set_pixel(x: int, y: int, r: int, g: int, b: int, a: int = 255) -> None:
        if 0 <= x < size and 0 <= y < size:
            i = (y * size + x) * 4
            data[i], data[i + 1], data[i + 2], data[i + 3] = r, g, b, a

    # Body of mic: vertical rect (x 12–20, y 4–20 in 32px space)
    for y in range(int(4 * s), int(20 * s)):
        for x in range(int(12 * s), int(20 * s)):
            set_pixel(x, y, fill_r, fill_g, fill_b)

    # Arc stand (simple U shape): y 20–26
    for y in range(int(20 * s), int(27 * s)):
        for x in range(int(8 * s), int(24 * s)):
            cx, cy2 = size / 2, int(20 * s)
            dx, dy2 = x - cx, y - cy2
            inner = 6 * s
            outer = 8 * s
            if inner * inner <= dx * dx + dy2 * dy2 <= outer * outer and dy2 >= 0:
                set_pixel(x, y, fill_r, fill_g, fill_b)

    # Stem line
    for y in range(int(27 * s), int(30 * s)):
        for x in range(int(15 * s), int(17 * s)):
            set_pixel(x, y, fill_r, fill_g, fill_b)

    # Base
    for y in range(int(29 * s), int(31 * s)):
        for x in range(int(10 * s), int(22 * s)):
            set_pixel(x, y, fill_r, fill_g, fill_b)

    return bytes(data)


def main() -> None:
    icons_dir = Path(__file__).parent.parent / "assets" / "icons"
    icons_dir.mkdir(parents=True, exist_ok=True)

    size = 32

    # Idle: green circle with white mic
    idle_bg = _circle_rgba(size, 34, 197, 94)  # green-500
    (icons_dir / "tray_idle.png").write_bytes(_make_png(size, size, idle_bg))

    # Recording: red circle
    rec_bg = _circle_rgba(size, 239, 68, 68)  # red-500
    (icons_dir / "tray_recording.png").write_bytes(_make_png(size, size, rec_bg))

    # Processing: amber circle
    proc_bg = _circle_rgba(size, 245, 158, 11)  # amber-500
    (icons_dir / "tray_processing.png").write_bytes(_make_png(size, size, proc_bg))

    # Error: yellow circle
    err_bg = _circle_rgba(size, 234, 179, 8)  # yellow-500
    (icons_dir / "tray_error.png").write_bytes(_make_png(size, size, err_bg))

    print(f"Icons written to {icons_dir}")
    print("Note: app_icon.png is not regenerated (custom icon in use).")


if __name__ == "__main__":
    main()
