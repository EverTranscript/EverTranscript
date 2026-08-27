# One Rust Core daemon; Electron and the CLI are Clients over a typed protocol

> **Amended 2026-08-27:** the protocol is concretized by ADR-0028 (codex app-server blueprint). The **Core owns the always-visible tray/menu-bar presence** — recording indicator plus record/stop controls — as a tiny UI-capable login-item agent, so the indicator exists even with no Client running; clicking it opens the Electron Client. The Client **never launches itself**: Auto-Record surfaces a system notification, and live captions are opt-in per meeting. "Always-on" means launch-at-user-login (SMAppService / Run key), never a pre-login root daemon — capture permissions are per-user by construction.

The product is one Rust binary — the Core — running as an always-on daemon (the login item) that owns detection, capture, ASR, Diarization, and storage, and doubling as the CLI via subcommands. The Electron app is a thin Client connecting over a typed local IPC protocol (unix socket on macOS, named pipe on Windows). The Core is the record's only writer; Clients command and read, never touching storage directly.

Chosen because ADR-0023 demands something always running, and what that something is shapes everything: a Rust daemon costs an order of magnitude less resident memory than Electron-at-login, a UI crash cannot kill a recording, and single-writer storage falls out of the process boundary instead of needing file-locking discipline.

The CLI is a supported control-and-query surface — daemon status, record start/stop, the Auto-Record switch, Watchlist edits, History full-text search, transcript export to stdout — not a second full product. Summaries, Speaker management, and remaining settings stay GUI-only in v1: parity is a treadmill, and the Markdown mirror already serves grep.

## Considered options

Rust-inside-Electron via napi-rs (no IPC to design, but Electron must live at login, a UI crash kills capture, and CLI + GUI become two writers on one SQLite file) and UI-owned core lifecycle (a child process per session — no UI, no detection, which breaks the never-miss promise at the root) were both rejected.
