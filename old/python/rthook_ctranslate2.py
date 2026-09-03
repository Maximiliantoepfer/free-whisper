import sys
import os

if getattr(sys, "frozen", False):
    import ctypes

    meipass = sys._MEIPASS
    ct2_dir = os.path.join(meipass, "ctranslate2")

    # Environment setup (before any DLL loads)
    os.environ["CT2_FORCE_CPU_ISA"] = "GENERIC"
    os.environ["OMP_NUM_THREADS"] = str(min(os.cpu_count() or 4, 4))

    # Register DLL search directories
    if os.path.isdir(ct2_dir):
        os.add_dll_directory(ct2_dir)
    os.add_dll_directory(meipass)
    os.environ["PATH"] = ct2_dir + os.pathsep + meipass + os.pathsep + os.environ.get("PATH", "")

    # Pre-load only the required DLLs in dependency order
    for dll_name in ["libiomp5md.dll", "ctranslate2.dll"]:
        dll_path = os.path.join(ct2_dir, dll_name)
        if os.path.exists(dll_path):
            ctypes.CDLL(dll_path)

    # Safety: delete any CUDA stub that slipped through
    # (ctranslate2.__init__.py loads ALL *.dll blindly via glob)
    for fname in os.listdir(ct2_dir):
        if "cudnn" in fname.lower() and fname.endswith(".dll"):
            try:
                os.remove(os.path.join(ct2_dir, fname))
            except OSError:
                pass
