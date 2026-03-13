import faulthandler
faulthandler.enable()

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "src"))

if getattr(sys, "frozen", False):
    print(f"[DIAG] frozen, _MEIPASS = {sys._MEIPASS}")
    ct2_dir = os.path.join(sys._MEIPASS, "ctranslate2")
    dlls = [f for f in os.listdir(ct2_dir) if f.endswith(".dll")] if os.path.isdir(ct2_dir) else []
    print(f"[DIAG] ctranslate2 DLLs: {dlls}")

from free_whisper.main import main
main()
