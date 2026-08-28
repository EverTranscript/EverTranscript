# 01: Workspace scaffold + wire tier (codex ports)

**What to build:** The repo becomes a buildable product skeleton: the three `evertranscript-*` crates plus the Electron app scaffold, CI compiling and testing both platforms, and the Core daemon speaking the ADR-0028 protocol well enough that `evertranscript status` gets an answer. The first codex ports land with full attribution (`NOTICE` + `PORTS.md`, pinned rev `5f49aba`): socket lifecycle, JSONL framing, RPC envelope, and the protocol-macro/codegen structure.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Cargo workspace (`evertranscript-core`, `evertranscript-protocol`, `evertranscript`) + pnpm Electron scaffold build green in CI on macOS and Windows
- [ ] Daemon listens on UDS (0600, stale-socket cleanup, startup lock — second instance refuses cleanly) on macOS and a named pipe on Windows
- [ ] JSONL JSON-RPC-shaped framing; per-connection `initialize` handshake enforced (requests before it are rejected)
- [ ] `evertranscript status` connects to the running daemon and reports version + uptime as JSON
- [ ] ts-rs + schemars codegen produces TypeScript bindings and JSON-schema fixtures, committed and test-enforced (drift fails CI)
- [ ] Every ported file keeps its upstream Apache-2.0 header; `PORTS.md` lists each with upstream path + rev; `NOTICE` exists at the root
