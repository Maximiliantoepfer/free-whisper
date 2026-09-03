# ADR 0005: Remote transcription uses a versioned HTTPS protocol

**Status:** Accepted

The worker protocol is versioned from its first endpoint and carries multipart
normalised audio plus typed JSON options. Remote clients require HTTPS and a
bearer token; HTTP is limited to confirmed loopback development use.
