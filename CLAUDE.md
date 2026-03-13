# free-whisper — Project Knowledge

Known pitfalls and patterns that MUST be preserved to prevent regressions.

## 1. blockSignals Pattern for Combo Boxes

**File:** `src/free_whisper/ui/pages/settings_page.py` (`_load_values()`)

When loading saved values into combo boxes during initialization, ALL combo box signals
must be blocked. Otherwise `currentIndexChanged` fires for each `setCurrentIndex()` call,
triggering spurious model reloads, setting writes, and potential crashes.

```python
for c in combos:
    c.blockSignals(True)
# ... set all indices ...
for c in combos:
    c.blockSignals(False)
```

**If you add a new combo box**, add it to the `combos` list in `_load_values()`.

## 2. faulthandler in Worker Thread

**File:** `src/free_whisper/core/transcriber.py` (`run()`)

The worker thread calls `faulthandler.enable()` at the start of `run()`. This ensures
C++ segfaults from ctranslate2 / ONNX Runtime produce a Python traceback on stderr
instead of silent process death. **Do not remove this.**

## 3. Cross-Thread Model Reload Safety

**File:** `src/free_whisper/core/transcriber.py`

`reload_model()` is called from the **main thread** but `_ensure_model()` runs on the
**worker thread**. These share `_loaded_model_size` and `_loaded_compute_type`.
Access must be protected by `self._lock` (threading.Lock).

**Never** access `self._model` from the main thread — it belongs to the worker thread.

## 4. Icon Requirements (Windows / macOS)

**Files:** `app.py` (lines 33-43), `main_window.py` (lines 39-48), `main.py` (lines 21-28)

- Windows requires `.ico` for proper taskbar / Alt+Tab display
- macOS requires `.icns`
- `SetCurrentProcessExplicitAppUserModelID("com.free-whisper.app")` in `main.py` is
  required for Windows taskbar grouping — must be called before QApplication is created
- Tray icons (`tray_icon.py`) have fallback pixmaps if PNG files are missing

**⛔ Do not modify icon setup code unless explicitly fixing an icon bug.**

## 5. QTimer Safety

**File:** `src/free_whisper/ui/app.py`

Use **bound methods** (e.g. `self._reset_to_idle`) for QTimer callbacks, not lambdas.
Lambdas capture `self` references and can crash if the app quits before the timer fires.

Exception: The `lambda: None` in `main.py` (line 44) is intentionally a no-op for
SIGINT signal delivery and is safe.

## 6. NoScrollComboBox

**File:** `src/free_whisper/ui/pages/settings_page.py`

All combo boxes in settings must use `NoScrollComboBox` (not plain `QComboBox`).
This class uses `FocusPolicy.StrongFocus` and only allows wheel events when the
widget has keyboard focus (i.e. user clicked on it), preventing accidental value
changes when scrolling the page.
