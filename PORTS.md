# Ports ledger

Every file in this repository that was copied or adapted from another
project, with its upstream path and the revision it came from. Adding a
ported file means adding a row here and an attribution header to the file
itself; this is the discipline ADR-0028's consumption amendment sets, and it
applies to all three upstreams.

Upstream licenses and how we may use them:

| Upstream | License | How we consume it |
| --- | --- | --- |
| [openai/codex](https://github.com/openai/codex) | Apache-2.0 | Read-and-port with attribution. **Never linked** — not from a registry, a git rev, or a local path. A local clone is a reference rig only. |
| [fastrepl/anarlog](https://github.com/fastrepl/anarlog) | MIT (excluding `enterprise/`, which is commercially licensed and off-limits) | Port with attribution. |
| [Zackriya-Solutions/meetily](https://github.com/Zackriya-Solutions/meetily) | MIT | Port with attribution. |

MIT and Apache-2.0 are both compatible with this project's Apache-2.0
license; preserving the upstream notice is the obligation, and these headers
plus this ledger are how we meet it.

## Ported files

| File | Upstream | Upstream path | Rev | What was taken |
| --- | --- | --- | --- | --- |
| `crates/evertranscript-protocol/src/rpc.rs` | openai/codex | `codex-rs/app-server-protocol/src/rpc.rs` | `5f49aba` | The JSON-RPC-shaped envelope: `RequestId`, request / response / error / notification shapes, and the decision to omit the `jsonrpc` field. Trace-context field dropped; names adapted to our conventions. |
| `crates/evertranscript-protocol/src/protocol.rs` | openai/codex | `codex-rs/app-server-protocol/src/protocol/common.rs` | `5f49aba` | The macro-table structure (`client_request_definitions!` / `server_notification_definitions!`) that generates the request enum, method table, and wire decoder. The method tables themselves are ours. |
| `crates/evertranscript-core/src/transport.rs` | openai/codex | `codex-rs/app-server-transport/src/transport/{unix_socket,stdio}.rs` | `5f49aba` | Socket lifecycle (refuse-if-live, clean-if-stale, 0600 permissions, unlink-on-drop), the startup lock that serializes competing launches, and the JSONL read/write framing. Deviations: plain JSONL instead of WebSocket-over-UDS, and a named pipe on Windows. |
| `crates/evertranscript-core/src/detect/calendar.rs` | fastrepl/anarlog | `crates/calendar/src/convert.rs`, `apps/desktop/src/stt/scheduled-meeting-attendance.ts` | `b7586718d5` | The self-declined skip: the Operator's own RSVP read from EventKit as `apple_self_attendance_status` reads it (organizer is never declined; otherwise the current-user attendee's `participantStatus`), and the rule that a declined event is not acted on. The WinRT `UserResponse` half is ours; anarlog has no Windows calendar. |
| `crates/evertranscript-core/src/detect/calendar.rs` | fastrepl/anarlog | `crates/apple-calendar/src/apple/transforms/utils.rs` | `d5fa9979b8` | `objc_title`: an EventKit `title` read through `msg_send!` as `Option<Retained<NSString>>`, nil as empty, so a nil title cannot panic the poll. |
| `crates/evertranscript-core/src/audio/live.rs` | fastrepl/anarlog | `crates/audio-actual/src/capture/recovery.rs`, `crates/audio-actual/src/mic.rs` | `693500220d` | The microphone's reopen loop (`run_channel`): end an attempt on a stream error or on 5s without frames, wait 1s/2s/4s, reopen, forgive the failures after 30s of health; reopening while skipping the device that just failed (`new_excluding`); one start in flight at a time (`StartupPermit`). Adapted from an async pulled stream to a thread watching cpal's pushed callbacks, and the stall timer starts at the leg's first frame. The permit waits up to 1s instead of refusing at once, because a Meeting's first start is not retried here. |
| `crates/evertranscript-core/src/audio/supervisor.rs` | fastrepl/anarlog | `crates/supervisor/src/restart.rs` | `693500220d` | The consecutive restart count and its reset on health (`record_consecutive_restart`, `reset`), replacing a time window. The reset arrives as `CaptureEvent::Healthy` from the leg that judged it, as upstream's `SessionMsg::SourceHealthy` does. |

## Not ported, deliberately

- codex's per-scope request serialization (`request_serialization.rs`): it exists because N clients mutate N independent threads. The Core has one engine and a single writer, so a command actor is enough.
- codex's pidfile daemonizer (`app-server-daemon`): launchd and the Windows Run key supervise us, which makes self-daemonization unnecessary — and it is Unix-only besides.
- codex's in-process client embedding: our CLI connects over the socket like any other Client, so the record keeps exactly one writer.
- anarlog's strict attendance gate from `b7586718d5` — organizer-or-accepted only, the `all_others_declined` check, and the attendance snapshot with its freshness check. It guards a calendar that *starts* recordings; ours only arms and names (ADR-0036), and blocking pending or unknown would stop naming every invite nobody answers and every personal event with no attendees. Only the decisive case, declined, is taken.
- anarlog's manual `retry_audio_capture` command and its float-bar button from `693500220d`: there is no such surface here, so a microphone that cannot come back ends its leg and the record says why.
- anarlog's joiner `pop_available_pair` from `693500220d`: our joiner fills gaps from absolute capture stamps, so a reopened leg needs nothing new from it.
- anarlog's speaker-channel recovery from `693500220d`: the system leg here is a CoreAudio process tap that delivers nothing at all while nothing plays (`judge_silent_leg`), so a 5s frame timeout would convict every quiet stretch. System-leg self-heal stays its own question.
