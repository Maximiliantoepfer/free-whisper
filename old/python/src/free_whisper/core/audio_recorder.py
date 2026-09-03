from __future__ import annotations

import threading
from collections import deque

import numpy as np
import sounddevice as sd


SAMPLE_RATE = 16_000  # Hz — faster-whisper expects 16kHz
CHANNELS = 1
DTYPE = "float32"
BLOCK_SIZE = 1024  # frames per callback


class AudioRecorder:
    """Records audio from the microphone using sounddevice.

    Thread-safe: the sounddevice callback runs in a PortAudio thread;
    start/stop are called from the Qt main thread.
    """

    def __init__(self, device: int | None = None) -> None:
        self._device = device
        self._buffer: deque[np.ndarray] = deque()
        self._lock = threading.Lock()
        self._stream: sd.InputStream | None = None
        self._recording = False

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def start(self) -> None:
        if self._recording:
            return
        with self._lock:
            self._buffer.clear()
        self._stream = sd.InputStream(
            samplerate=SAMPLE_RATE,
            channels=CHANNELS,
            dtype=DTYPE,
            blocksize=BLOCK_SIZE,
            device=self._device,
            callback=self._callback,
        )
        self._stream.start()
        self._recording = True

    def stop(self) -> None:
        if not self._recording:
            return
        self._recording = False
        if self._stream:
            self._stream.stop()
            self._stream.close()
            self._stream = None

    def get_audio(self) -> np.ndarray:
        """Return the full recorded audio as a float32 numpy array (16kHz mono)."""
        with self._lock:
            chunks = list(self._buffer)
        if not chunks:
            return np.zeros(0, dtype=np.float32)
        audio = np.concatenate(chunks, axis=0).flatten()
        return audio.astype(np.float32)

    @property
    def is_recording(self) -> bool:
        return self._recording

    @property
    def duration_ms(self) -> int:
        with self._lock:
            total_frames = sum(c.shape[0] for c in self._buffer)
        return int(total_frames * 1000 / SAMPLE_RATE)

    def set_device(self, device: int | None) -> None:
        self._device = device

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    def _callback(
        self,
        indata: np.ndarray,
        frames: int,
        time: object,  # CData
        status: sd.CallbackFlags,
    ) -> None:
        if status:
            pass  # could log status.input_overflow etc.
        if self._recording:
            with self._lock:
                self._buffer.append(indata.copy())

    # ------------------------------------------------------------------
    # Class-level helpers
    # ------------------------------------------------------------------

    @staticmethod
    def list_devices() -> list[dict]:
        """Return list of input devices as dicts with 'index', 'name', 'channels'."""
        devices = []
        for i, d in enumerate(sd.query_devices()):
            if d["max_input_channels"] > 0:  # type: ignore[index]
                devices.append(
                    {
                        "index": i,
                        "name": d["name"],  # type: ignore[index]
                        "channels": d["max_input_channels"],  # type: ignore[index]
                        "default": i == sd.default.device[0],
                    }
                )
        return devices
