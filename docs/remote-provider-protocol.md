# Remote-provider protocol v1

The desktop app and `free-whisper-worker` communicate through a versioned HTTP
contract. Each JSON response carries `api_version: 1`; incompatible clients
must fail visibly instead of guessing fields.

## Transport and authentication

- Remote profiles require HTTPS.
- Plain HTTP is accepted only for an explicitly enabled development profile whose
  host is `localhost`, `127.0.0.1`, or `::1`.
- Every endpoint requires `Authorization: Bearer <token>`. The desktop reads the
  token from the platform secret store; it is never stored in SQLite.
- The Provider page first runs a non-persistent health/capability/model test.
  Only a successful test permits saving a profile; the selected ready model and
  opaque Credential Manager reference are the only persisted values.
- On Windows, the reference is scoped to `free-whisper/provider/<uuid>` so the
  application cannot read or alter credentials owned by another product.
- The packaged worker accepts only loopback HTTP. A remote deployment terminates
  TLS at a separately configured reverse proxy; direct public HTTP is refused.
- The worker obtains its token exclusively via stdin, never an environment
  variable or command-line argument.

## Endpoints

| Endpoint | Result |
| --- | --- |
| `GET /healthz` | readiness and worker-instance UUID |
| `GET /v1/capabilities` | execution, timestamp, GPU and request-limit capabilities |
| `GET /v1/models` | installed/ready models |
| `POST /v1/transcriptions` | versioned transcription result |
| `POST /v1/transcriptions/{uuid}/cancel` | explicit cancellation acknowledgement |

`POST /v1/transcriptions` is multipart/form-data with exactly these parts:

- `options`: JSON `TranscriptionOptionsV1`, including the UUID request ID;
- `audio_encoding`: `wav` or `pcm_f32le_mono_16khz`;
- `audio`: WAV or mono 16-kHz f32-LE audio.

The worker limits a request to 32 MiB, ten minutes of decoded audio, one active
transcription and 20 transcription requests per minute. It does not persist the
audio payload. PCM is validated for finite samples and wrapped in an in-memory
WAV container before it reaches the inner whisper.cpp server.

## Failure contract

Errors are JSON `ApiErrorResponseV1` objects with an API version, machine code,
safe human message, retryability and (where available) request ID. Codes include
`unauthorized`, `invalid_request`, `request_too_large`, `audio_too_long`,
`rate_limited`, `busy`, `model_not_ready`, `cancelled`, `not_found` and
`engine_unavailable`. No endpoint uses a text/HTML fallback that a client could
misinterpret as success.

## Local worker lifecycle

`LocalWhisperCppProvider` chooses an ephemeral loopback port, generates an
in-memory bearer token, writes it through the child stdin and polls authenticated
readiness. It passes the selected model and explicit backend to the sidecar.
Missing files, an exited sidecar, startup timeout and failed token handoff return
specific provider errors. Cancelling a running request makes the worker stop and
restart its inner loopback whisper.cpp process; cancellation is never ignored.
