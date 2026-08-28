# M1 — Walking Skeleton: Core daemon, protocol, manual record → captions → SQLite → Mirror

Status: ready-for-agent

Sources of truth: `CONTEXT.md` (glossary — its vocabulary is normative here), `docs/prd.md`, ADR-0001–0036, `docs/implementation-notes-2026-08-27.md` (the absorption catalog — consult its evidence paths per the reuse rules in `AGENTS.local.md`). Where this spec and an ADR disagree, the ADR wins.

## Problem Statement

EverTranscript exists only as documentation: thirty-six ratified ADRs, a PRD, a glossary — and no binary. The Operator cannot press Record, see a caption, or find a Meeting on disk. Every remaining unknown the docs name (whisper streaming quality on real bilingual meetings, capture resilience through device churn, protocol ergonomics for two Clients, Windows parity) is empirical and can only be answered by a running walking skeleton. Until it exists, nothing can be dogfooded and no later milestone can start.

## Solution

One Rust binary — the Core — running as a daemon with the codex-blueprint protocol, plus its first two Clients (a minimal Electron app and CLI subcommands), delivering the complete manual path end to end on macOS and Windows: press Record (tray, Client, CLI, or hotkey) → dual-channel echo-cancelled capture → live captions streaming to the Client → transcript rows and an incrementally-encoded stereo AAC file landing crash-safe in the History folder → a Mirror at the folder's top level the moment the Meeting persists. Dogfoodable immediately; Auto-Record (M2) plugs into this skeleton without reshaping it.

## User Stories

1. As an Operator, I want to start a recording from the Core's tray menu, the Client, the CLI, or a global hotkey, so that capture is one act from anywhere.
2. As an Operator, I want microphone and system audio captured together as two channels, so that both sides of the call are in the Transcript.
3. As an Operator, I want live captions in the Client while recording, so that I can confirm transcription works and glance back at what was just said.
4. As an Operator, I want stopping the recording to immediately persist the Meeting, so that a crash after the call cannot lose the record.
5. As an Operator, I want a crash *during* a recording to lose at most the last few seconds — audio checkpoints merged, transcript-so-far folded from the delta journal, a valid Mirror on next start — so that the record survives the machine.
6. As an Operator, I want an AirPods swap or default-device change mid-recording to continue the same Meeting with no split and no audio↔transcript drift, so that device churn never corrupts the record.
7. As an Operator, I want a system-audio hiccup to self-heal while the microphone keeps flowing, so that one leg's failure never silences the other.
8. As an Operator, I want capture and disk writing to continue even if live transcription fails, so that the recording never depends on ASR health.
9. As an Operator, I want transcription to run entirely on my machine with no configuration able to change that, so that raw meeting audio can never leave my device.
10. As a bilingual Operator, I want the default model to handle my Chinese/English meetings including code-switching, so that the record is usable.
11. As an Operator, I want the recording indicator visible at a glance via the Core's tray states, so that I am never recording without knowing it.
12. As an Operator, I want the tray's Quit to stop the Core now, separate from the launch-at-login toggle (default on, registration-only, also `evertranscript autostart on|off`), so that leaving the always-on posture is legible acts, never conflated.
13. As an Operator, I want nothing captured before an explicit first-run acknowledgment, so that the pre-capture invariant holds from the first dogfood build.
14. As an Operator, I want the microphone and system-audio permissions requested at first record with plain explanations, so that grants trace to my acts.
15. As an Operator, I want the default Whisper model fetched at first need — checksummed, resumable, stall-detected, with a configurable mirror URL — so that onboarding works on my network.
16. As an Operator, I want every Meeting in a local full-text-searchable store, queryable from the Client and `evertranscript search`, so that I can find any conversation.
17. As an Operator, I want each Meeting auto-mirrored to a Markdown file at the top of my History folder — frontmatter (id, date, app, duration, speakers, audio path), Title, Summary ("None yet"), Notes ("None yet"), Transcript — so that my record is greppable and usable without the app.
18. As an Operator, I want Mirror filenames born as `YYYY-MM-DD-<app>-<id8>.md` and renamed when I retitle (stale names GC'd by id8), so that my folder reads as meeting notes.
19. As an Operator, I want to retitle a Meeting in the Client, so that my History isn't a list of "zoom, 10:02".
20. As an Operator, I want the record to be immutable — no edit path exists for Transcript text, so that my History is trustworthy evidence.
21. As an Operator, I want to delete a whole Meeting (rows, Mirror, audio) as one act from the Client or CLI, so that removal is exact and complete.
22. As an Operator, I want meeting audio kept by default as one stereo AAC file (mic left, system right) under `.data/audio/`, with a global keep-audio setting, so that future re-transcription stays possible.
23. As an Operator, I want the Core to flag a History folder with Mirrors but no `.data/` as an incomplete copy with recovery guidance, so that a bad copy is never silent.
24. As an Operator, I want a Client attaching mid-recording to receive the Meeting state and transcript-so-far, then live deltas, so that opening the app late loses nothing.
25. As an Operator, I want captions delivered lossily to a slow Client — degraded captions, never a killed connection or blocked capture — so that UI performance can't harm the record.
26. As an Operator, I want the Client and CLI attached concurrently, so that surfaces never fight over the Core.
27. As an Operator, I want a Client crash or window close to never affect a recording, so that the UI is disposable.
28. As an Operator, I want opening the Client to start the Core if it isn't running, so that the app always works even after a Quit.
29. As an Operator, I want `evertranscript status | record start | record stop | search | export | autostart` speaking to the running Core, so that my record is scriptable and pipeable.
30. As a Windows Operator, I want all of the above on Windows 10+ x64 (WASAPI capture, named pipe, Run key, tray), so that my platform decides neither my privacy nor my wait.
31. As a privacy-conscious evaluator, I want an all-local M1 build with models downloaded to produce literally zero network traffic, so that Sanctioned Traffic is verifiable from the first build.
32. As a zh-CN Operator-to-be, I want every UI string externalized from the first commit, so that localization is never a retrofit.
33. As a contributor, I want CI compiling and testing both platforms from the first commit, with ported files carrying upstream notices in `PORTS.md` beside `NOTICE`, so that the parity gate and license hygiene start at line one.

## Implementation Decisions

- **Workspace**: one Cargo workspace — `evertranscript-core` (engine), `evertranscript-protocol` (wire types + codegen), `evertranscript` (the one binary: daemon + CLI subcommands) — with the Electron client as a pnpm app beside it. React 19 + Vite + Tailwind, strict TypeScript, minimal state; protocol types consumed from generated ts-rs bindings.
- **Codex ports (ADR-0028 discipline)**: wire tier + protocol-macro/codegen structure ported now against pinned rev `5f49aba` — unix-socket lifecycle (0600, stale-socket cleanup, startup lock), JSONL framing, RPC envelope, ts-rs + schemars codegen with committed schema fixtures test-enforced. Multi-client fanout pieces port when the Client lands. Every ported file: upstream header kept, `PORTS.md` entry. Never link codex; anarlog/Meetily reuse is port-with-attribution, `enterprise/` excluded.
- **Protocol (ADR-0028)**: JSON-RPC-shaped JSONL over UDS (macOS) / named pipe (Windows); per-connection initialize with capability gating; Meeting-scoped vs broadcast notifications with per-connection subscriptions; snapshot-then-tail on attach; caption deltas as an opt-in lossy/conflating subscription; word-level deltas with stable ids and Partial→Final states.
- **Capture (ADR-0027/0029 as amended)**: CoreAudio process taps + private aggregate device (macOS 14.4+) and cpal mic; WASAPI loopback + capture (Windows 10+). Dual-channel end to end; DTLN-shape AEC via ort on the mic channel with the system channel as reference; absolute capture timestamps on one shared clock; mic hot-swap + in-place format renegotiation where cheap, supervised budget-free respawn on device change, budgeted respawn on error, system-audio self-heal with bounded backoff; no pre-roll buffering in any form. Loudness normalization on mic (EBU R128 −23 LUFS); no noise suppression.
- **ASR (ADR-0014 as amended)**: whisper-rs with `ggml-large-v3-turbo-q8_0` as the shipped default (Settings escape hatch); duration-adaptive Silero chunking (3s/20s/25s envelope, live redemption); hallucination defenses (VAD masking on mic, energy gate, `[_BEG_]` logits pin, rolling initial-prompt, blocklist + repetition-ratio drops); prefix-commit caption stabilization with a gap-skip clock; `no_timestamps` + token-timestamps decode workaround, timing from VAD boundaries; tail integrity via queued-vs-completed accounting, pipeline drains to channel-close.
- **Storage (ADR-0005/0035 as amended)**: rusqlite on a dedicated writer thread plus a small read-only pool; STRICT tables, CHECK-validated enums, UUIDv7 ids; the live-transcript delta journal (unique sequence, CAS fold) doubles as the crash story and the snapshot-then-tail source; trigger-fed dirty queue with generation acks drives Mirror regeneration and rename/GC; atomic temp-file+rename writes; Voiceprint columns present but unused until M3. History folder defaults to `~/Documents/EverTranscript` (Mirrors top-level, machine store in hidden `.data/` — Windows hidden attribute set; incomplete-copy detection).
- **Recording sink (ADR-0032)**: bundled ffmpeg encoding stereo AAC from piped f32 PCM in 30s checkpoint files, lossless-concat merged at finalize; recovery scans and merges with success/partial states.
- **Tray & lifecycle (ADR-0026 as amended)**: the Core is a UI-capable login-item agent owning the tray (state machine with optimistic transitional items and a not-ready gate during model download); Quit stops the Core; the Client never auto-launches; SMAppService / Run key registration behind the autostart toggle.
- **Supervision**: heartbeat ping/pong with bounded finalize-on-stop; restart budgets with ordered meltdown (source→listener→recorder so buffers flush); deterministic-jitter retry ladder; capture continues when the ASR leg dies.
- **Models**: registry enum (filename, bytes, pinned checksum, languages); HF primary + Operator-configurable mirror URL; HTTP Range resume, per-chunk stall timeout, magic-bytes triage, rename-promote.
- **First-run gate**: a minimal acknowledgment dialog stands in for the full M5 Briefing so the nothing-before-acknowledgment invariant holds from the first build.
- **Posture invariants live from M1**: no telemetry or crash SDK (local crash reports only); traffic beyond model downloads is zero; keys/keyring untouched (no cloud features exist yet); strings externalized (lingui-style) with English rendered.

## Testing Decisions

- **Philosophy (per the PRD, unchanged)**: external behavior only — observable outputs are SQLite content, Mirror files, and protocol responses/events as seen by Clients. No mocking of storage or the ASR engine in the default suite. Harness is `cargo test`; the Electron Client stays thin enough that the protocol contract, not a GUI rig, is the tested surface.
- **Seams (ratified in the PRD; none new)**: **AudioSource** — live capture and fixture WAV/PCM playback interchange at one trait boundary; end-to-end tests feed fixture meeting audio through real whisper → store → Mirror and assert on artifacts (transcript rows exist, captions streamed, Mirror composed). Stream-death and device-change events inject at the same boundary to drive the churn contract (same Meeting, gap accounting, self-heal). The **protocol surface** is exercised by a test client: initialize, snapshot-then-tail on mid-recording attach, lossy caption backpressure, concurrent CLI+Client. DetectionSource and the LLM-endpoint seams arrive with M2/M4.
- **This milestone builds the shared harness**: the fixture-audio library (real bilingual meeting clips as compile-time constants, multi-format/multi-rate), feature-based audio similarity assertions (RMS/peak/ZCR/centroid bands, not bit-exactness), WER/CER metrics for ASR quality tracking, hallucination canaries (silence-heavy fixtures must produce no blocklist phrases), and RMS-preservation checks on resampling.
- **Crash tests**: kill the Core mid-recording → next start recovers checkpoints into a playable file, folds the delta journal, regenerates a valid Mirror; kill mid-stop → tail accounting reports zero lost chunks or flags loss.
- **Guarantee tests start now**: artifact scan (no analytics/crash SDK in the binary, no key material anywhere), permission-set audit (mic + system-audio only in M1), and zero-network-traffic with models present.
- **Prior art**: greenfield by design — this harness is the prior art for every later milestone; shapes come from the absorption catalog's Testing section.

## Out of Scope

- M2: Meeting Detection, Auto-Record, Watchlist, calendar arming/titles/attendees, armed-but-untriggered follow-up, DND-aware notifications.
- M3: Diarization, Speakers, "You", Voiceprints (schema columns exist, unpopulated), correction hints, attribution in Mirrors beyond channel labels.
- M4: Summary, Notes pane, the sidecar, keyring, the Knob/Fallback/Strict Mode, auto-applied transcript titles (M1 titles are `date+app` placeholder plus manual retitle), per-segment language voting.
- M5: the full Briefing, linear onboarding, History-folder relocation UI, distribution (signing, notarization, installers, winget/Homebrew, updater), the floating mini-indicator, zh-CN rendering.
  Distribution consumes the committed brand assets: `clients/electron/resources/` (icns/ico/png) and `brand/generated/` (per-platform icon sets, `EverTranscript.icon` for the macOS 26 layered icon) — regenerate with `pnpm -C brand render`, never by hand.
- Ratified non-goals reaffirmed: in-app audio playback, external audio import, pre-roll buffering, telemetry.

## Further Notes

- The absorption catalog (`docs/implementation-notes-2026-08-27.md`) is the implementation companion: every M1 area above has an entry with upstream evidence paths — consult the referenced source before writing new code (standing rule in `AGENTS.local.md`).
- M1 is where the PRD's top empirical risk gets its first data: whisper streaming quality on the Operator's real meetings. The WER/caption-latency numbers from the fixture harness are a deliverable, not a byproduct.
- The Windows parity gate (ADR-0025 as amended) applies from the first commit: CI builds and tests both targets; a milestone is not done until both platforms pass.
- Reference rigs available on this machine: the pinned codex clone, anarlog, Meetily, and the Granola bundle (paths in `AGENTS.local.md`).
