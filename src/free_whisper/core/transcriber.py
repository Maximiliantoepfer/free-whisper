from __future__ import annotations

import gc
import queue
import threading
from dataclasses import dataclass

import numpy as np
from PyQt6.QtCore import QThread, pyqtSignal

from ..utils.log import get_logger

log = get_logger(__name__)

# Minimum audio worth sending to Whisper (0.5 s at 16 kHz)
MIN_AUDIO_SAMPLES = 8_000


@dataclass
class TranscribeJob:
    audio: np.ndarray
    audio_duration_ms: int
    model_size: str
    compute_type: str
    language: str         # "" = auto-detect
    hotwords: str         # comma-separated custom vocabulary
    initial_prompt: str
    job_id: int = 0


class TranscriberWorker(QThread):
    """Background thread owning the WhisperModel.

    Model is loaded lazily on first job, kept in RAM between jobs.
    If the preferred device/compute_type fails, falls back to cpu+int8.
    """

    transcription_ready = pyqtSignal(str, int, int)   # text, duration_ms, job_id
    transcription_failed = pyqtSignal(str, int)        # error_msg, job_id
    model_loading = pyqtSignal(str)                    # model_size
    model_ready = pyqtSignal(str)                      # model_size
    model_load_failed = pyqtSignal(str)                # error_msg

    def __init__(self, parent=None) -> None:
        super().__init__(parent)
        self._queue: queue.Queue[TranscribeJob | None] = queue.Queue()
        self._lock = threading.Lock()
        self._model = None
        self._loaded_model_size: str | None = None
        self._loaded_compute_type: str | None = None
        self._running = True

    # ------------------------------------------------------------------
    # Public API (called from main thread)
    # ------------------------------------------------------------------

    def enqueue(self, job: TranscribeJob) -> None:
        log.info("Job enqueued: model=%s  samples=%d  duration=%dms",
                 job.model_size, job.audio.size, job.audio_duration_ms)
        self._queue.put(job)

    def stop(self) -> None:
        self._running = False
        self._queue.put(None)
        self.wait(10_000)

    def reload_model(self, model_size: str, compute_type: str) -> None:
        log.info("Model reload requested: %s / %s", model_size, compute_type)
        with self._lock:
            self._loaded_model_size = None  # force reload on next job

    # ------------------------------------------------------------------
    # QThread.run()
    # ------------------------------------------------------------------

    def run(self) -> None:
        # Enable faulthandler so C++ crashes (segfaults in ctranslate2 /
        # ONNX Runtime) produce a Python traceback instead of silent death.
        import faulthandler
        faulthandler.enable()

        log.info("Worker thread started")
        while self._running:
            try:
                job = self._queue.get(timeout=1.0)
            except queue.Empty:
                continue

            if job is None:
                break

            try:
                self._ensure_model(job.model_size, job.compute_type)
                text = self._transcribe(job)
                log.info("Result (%d chars): %r", len(text), text[:80])
                self.transcription_ready.emit(text, job.audio_duration_ms, job.job_id)
            except Exception as exc:
                import traceback
                log.error("Transcription error: %s", exc)
                log.debug("Traceback:\n%s", traceback.format_exc())
                self.transcription_failed.emit(str(exc), job.job_id)

        log.info("Worker thread stopped")

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    def _ensure_model(self, model_size: str, compute_type: str) -> None:
        with self._lock:
            if (
                self._model is not None
                and self._loaded_model_size == model_size
                and self._loaded_compute_type == compute_type
            ):
                return  # already loaded

        if self._model is not None:
            log.info("Unloading %s", self._loaded_model_size)
            try:
                del self._model
                self._model = None
                try:
                    import torch
                    if torch.cuda.is_available():
                        torch.cuda.synchronize()
                        torch.cuda.empty_cache()
                except ImportError:
                    pass
                gc.collect()
            except Exception as exc:
                log.warning("Error unloading model: %s", exc)
                self._model = None

        self.model_loading.emit(model_size)

        from faster_whisper import WhisperModel
        from ..utils.platform_utils import get_models_cache_dir

        cache = str(get_models_cache_dir())
        resolved_compute = self._resolve_compute_type(compute_type)
        resolved_device = self._resolve_device(resolved_compute)

        # Try preferred config, fall back to cpu+int8 on any error
        configs = [(resolved_device, resolved_compute)]
        if (resolved_device, resolved_compute) != ("cpu", "int8"):
            configs.append(("cpu", "int8"))

        last_exc: Exception | None = None
        for device, ct in configs:
            try:
                log.info("Loading '%s' on %s/%s …", model_size, device, ct)
                self._model = WhisperModel(
                    model_size, device=device, compute_type=ct,
                    download_root=cache,
                )
                with self._lock:
                    self._loaded_model_size = model_size
                    self._loaded_compute_type = compute_type
                log.info("Model ready: %s %s/%s", model_size, device, ct)
                self.model_ready.emit(model_size)
                return
            except Exception as exc:
                log.warning("Config %s/%s failed: %s", device, ct, exc)
                last_exc = exc

        self.model_load_failed.emit(str(last_exc))
        raise last_exc  # type: ignore[misc]

    def _transcribe(self, job: TranscribeJob) -> str:
        if self._model is None:
            raise RuntimeError("Model not loaded")

        if job.audio.size < MIN_AUDIO_SAMPLES:
            log.debug("Audio too short (%d samples), skipping", job.audio.size)
            return ""

        kwargs: dict = {
            "beam_size": 5,
            "vad_filter": True,
            "vad_parameters": {"min_silence_duration_ms": 500},
        }
        if job.language:
            kwargs["language"] = job.language
        if job.hotwords:
            kwargs["hotwords"] = job.hotwords
        if job.initial_prompt:
            kwargs["initial_prompt"] = job.initial_prompt

        log.info("Transcribing %.1fs of audio …", job.audio.size / 16000)
        segments, info = self._model.transcribe(job.audio, **kwargs)

        # Consume the lazy generator here (inference runs during iteration)
        try:
            parts = [seg.text for seg in segments]
        except Exception as exc:
            log.error("Error consuming segments: %s", exc)
            raise RuntimeError(f"Segment error: {exc}") from exc
        text = "".join(parts).strip()

        log.info("Language: %s (%.2f) | segments: %d",
                 info.language, info.language_probability, len(parts))
        return text

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _resolve_device(compute_type: str) -> str:
        if compute_type == "float16":
            try:
                import ctranslate2
                if ctranslate2.get_cuda_device_count() > 0:
                    return "cuda"
            except Exception:
                pass
        return "cpu"

    @staticmethod
    def _resolve_compute_type(compute_type: str) -> str:
        if compute_type != "auto":
            return compute_type
        try:
            import ctranslate2
            if ctranslate2.get_cuda_device_count() > 0:
                return "float16"
        except Exception:
            pass
        return "int8"
