# ADR 0007: CPAL capture, Rubato normalisation and WebRTC VAD

## Status

Accepted for the CPU-only Alpha vertical slice.

## Context

Microphones expose different sample rates, channel counts and sample formats.
The local whisper.cpp worker accepts only 16-kHz mono PCM-f32. Toggle recording
must end on actual silence without an ONNX model download or an opaque cloud
service. Capture must have a strict memory bound and must make a device error
or callback contention visible rather than quietly sending incomplete audio.

## Decision

- Use CPAL for Windows input-device enumeration and capture.
- Select a supported capture format preferring 48/32/16/8 kHz, down-mix each
  callback frame to mono and keep it only in a preallocated bounded memory
  buffer. The default limit is 180 seconds; the accepted range is 15–600
  seconds. Reaching it ends the recording visibly.
- Convert completed capture with Rubato to 16-kHz PCM-f32 before any provider
  request. Audio bytes are never persisted.
- Use `wavekat-vad` with its WebRTC backend only. The dependency is pinned
  without ONNX-related features, uses 30-ms frames, defaults to aggressiveness
  2, 180 ms minimum speech and 1,200 ms trailing silence, and may be disabled
  explicitly.
- Run the same WebRTC decision live at a compatible input rate to mark toggle
  capture ready for stopping, then run it again after normalisation as the
  authoritative preflight before inference.

## Consequences

The implementation needs no inference-model download besides the chosen
Whisper weight and has an auditable VAD configuration. The bounded source-rate
buffer consumes up to roughly 34 MiB at the default 180-second/48-kHz mono
configuration (and more at the explicitly chosen 600-second maximum); this is
a deliberate trade-off for deterministic capture without writing audio to disk.
The UI must report microphone loss, callback overflow, VAD failure and limit
reached as errors. Global hotkeys and system tray recording use this same
backend in a later Windows-integration milestone.
