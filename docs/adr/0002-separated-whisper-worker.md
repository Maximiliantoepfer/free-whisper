# ADR 0002: Run whisper.cpp in a separate worker process

**Status:** Accepted

The application does not link directly to whisper.cpp through Rust FFI. A
managed `free-whisper-worker` process owns an inner whisper.cpp server on
loopback and exposes the stable v1 protocol. This isolates native crashes,
permits controlled cancellation by engine restart, and is reusable on a remote
machine.
