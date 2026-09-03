# ADR 0001: Rust backend with Tauri 2 and Svelte

**Status:** Accepted

The desktop product uses a Rust workspace, Tauri 2 and a TypeScript/Svelte UI.
Rust owns persistence, audio, jobs, providers and platform integration. Svelte
is a typed view layer only. This removes the PyQt GPL dependency from the
runtime and makes native error boundaries explicit.
