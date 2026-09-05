# Kept audio: reopening the codec behind ADR-0032

Status: ready-for-agent

**Decided 2026-09-05 and written into ADR-0032, which now reads "Kept audio is one MP3 per Meeting, encoded in-process by LAME".** This file is the working record behind that ADR — the evidence, the options weighed, and the recommendation that was not taken. Implementation has not started.

Sources of truth: ADR-0032 (kept audio is stereo AAC via bundled ffmpeg — **reopened**, this is its review), ADR-0019 (meeting audio persists by default; the record is the transcript, audio is the bonus), ADR-0029 (dual-channel, mic is the Operator), ADR-0031 (the sidecar boundary and why it exists), ADR-0026 (the Core is the record's only writer), `CONTEXT.md`. Where this spec and an ADR disagree, the ADR wins.

## Problem Statement

ADR-0032 chose a bundled ffmpeg over a "recommended" pure-Rust Ogg/Opus route, pricing the choice at "a ~7MB+ sidecar and its LGPL-build/update surface". Packaging the product for the first time revealed that the sidecar had never actually been bundled, and that the surface is priced wrong.

**Nothing ever staged an ffmpeg binary.** `ffmpeg_binary()` returned the bare string `"ffmpeg"`, `packaging/build.sh` never copied one, `extraResources` never carried one, and the CI packaging guard — written after an installer shipped with no product inside it — checked the other two binaries and not this one. A developer's Homebrew ffmpeg answered on every machine anyone tested on. A Core spawned by a Finder-launched Client inherits almost no `PATH`, finds nothing, and records every Meeting with no audio at all, behind a single `warn!` that reaches no log file and no window. That is now fixed (bundle before `PATH`, staged, signed, guarded, and the failure reaches `audio_notes`) — but it is also the evidence that the sidecar was decided on and never shipped, so the ADR's cost line was never tested.

Testing it now: **there is no mainstream prebuilt LGPL ffmpeg for macOS.** Windows has one; macOS does not. Honouring ADR-0032's own LGPL requirement therefore means building ffmpeg from source in CI for two platforms and signing it with the Operator's Developer ID — a supply-chain and licensing commitment the ADR did not price.

Meanwhile the two things the sidecar was bought for are, in the shipped code, carried by something else.

## Evidence

Verified 2026-09-05 against this tree, this machine, and the named upstreams.

**What ffmpeg actually does here.** Two invocations, one file, `audio/sink.rs`: `Encoder::spawn` (`sink.rs:100`) pipes raw `f32le` stereo @48 kHz to AAC-LC 192k `.m4a` with `+faststart`, one process per 30s checkpoint; `merge_checkpoints` (`sink.rs:396`) concatenates them with `-c copy`. No filters, no scaling, no probing.

**The churn rationale is not carried by ffmpeg.** Device churn is resolved upstream by `audio/supervisor.rs` and `audio/joiner.rs`. The sink receives already-joined `StereoBlock`s at a fixed rate, and the encoder is spawned with hard-coded `-f f32le -ar 48000 -ac 2`. ffmpeg never sees a device, a rate change, or a reconnect.

**The decode rationale is not carried by ffmpeg either.** Reading a finished Meeting's audio back — the Diarization path — is symphonia, in-process (`diarize/runner.rs:66`), with `features = ["aac", "isomp4"]`: exactly the format ffmpeg writes and nothing more.

**The licence surface, concretely.** The Homebrew build on this machine is `--enable-gpl --enable-libx264 --enable-libx265 --enable-version3`. [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases) publishes explicit `win64-lgpl` / `winarm64-lgpl` artifacts, so Windows is solved. macOS has no equivalent: evermeet.cx and OSXExperts are GPL, [ColorsWind/FFmpeg-macOS](https://github.com/ColorsWind/FFmpeg-macOS) is LGPLv2 but shared-library, [Martin-Riedl.de](https://ffmpeg.martin-riedl.de/) is unaudited. Build-it-yourself is the only defensible macOS answer.

**What the category does.** Only one of the four ships ffmpeg, and it is the one this design was borrowed from.

| | ffmpeg | How |
| --- | --- | --- |
| **Meetily** | **bundles it** | `src-tauri/build/ffmpeg.rs` downloads at build time from their own mirror `Zackriya-Solutions/ffmpeg-binaries@0.0.1`, plus a runtime installer. The mirror holds `ffmpeg-8.0.1-essentials_build.zip` (gyan *essentials* = GPL) and `ffmpeg-release-{amd64,arm64}-static.tar.xz` (johnvansickle = GPL) — **GPL builds vendored into an MIT app**. |
| **anarlog** | no | `hound` (WAV) + `vorbis_rs` (Ogg Vorbis) + `mp3lame-encoder`, in-process, dual-track. Every ffmpeg reference in the repo is dev scripts, e2e, or CI. |
| **Cluely** | no | Electron; bundle inspected on this machine — only Chromium's `libffmpeg.dylib`, zero sidecar executables. |
| **Granola** | not recorded | The 2026-08-27 bundle mine (v7.515.1) found ~16 native N-API addons and `utilityProcess` children for audio, and no ffmpeg. Silence, not a positive check — it is uninstalled now. |

anarlog is the closest architectural peer — Rust, dual-track, local-first — and it needed no sidecar.

**What Rust offers.** Every crate under the `ffmpeg` keyword on crates.io is either FFI bindings to libav\* (`ffmpeg-next` 6.8M) or a wrapper around the CLI (`ffmpeg-sidecar` 1.77M). Bindings would make the licence position *worse* — LGPL linked into the Core — and dissolve the process boundary ADR-0031 argues for. And the decisive fact:

- **AAC**: `symphonia-codec-aac` is a *decoder*; symphonia is decode-only by design. **There is no pure-Rust AAC encoder.** AAC is the single reason a sidecar is needed at all.
- **Opus**: `opus` (MIT/Apache-2.0) and `audiopus` (ISC) bind libopus (BSD-3). Statically linkable, in-process.
- **Vorbis**: `vorbis_rs` (BSD-3) encodes; `ogg` (BSD-3) is a pure-Rust container encoder.
- **Symphonia decode coverage**: Vorbis "excellent" and Ogg "great", both default-on; **Opus is "not started"**.

Nothing in that list is GPL or LGPL.

## Options

**A. Keep ffmpeg; build a minimal LGPL binary in CI.** `--disable-everything` plus the `f32le` demuxer, native `aac` encoder, `mov` muxer, `concat` demuxer and `pipe`/`file` protocols — a few MB, licence-clean by construction, reproducible from a pinned tag, one recipe for both platforms. Keeps ADR-0032 whole and keeps the door open to the post-v1 import surface (build full-LGPL instead of minimal if that surface matters — FFmpeg's own decoders are all LGPL, so only x264/x265 encode is lost, which is video). Costs: a fourth signed binary, a build job, and the licence obligation forever.

**B. Ogg Vorbis via `vorbis_rs`.** anarlog's choice. Symphonia already decodes it, so the readback path gains a feature flag and loses nothing. One new permissive dependency, no sidecar.

Prior art, read 2026-09-05 (not copied — see the note on reuse below): anarlog's `crates/audio-utils/src/vorbis.rs`, 302 lines over `vorbis_rs::{VorbisEncoderBuilder, VorbisBitrateManagementStrategy, VorbisDecoder}`. It offers `encode_vorbis_from_channels(&[&[f32]], …)`, `encode_vorbis_from_interleaved(…)` and `encode_vorbis_mono(…)`, so **interleaved input is a first-class entry point** — an earlier note here claimed Vorbis forced a deinterleave step that Opus does not; it does not, and the two are even on that point. `VorbisBitrateManagementStrategy` is where their answer to open question 4 lives and is worth reading before picking a bitrate. `ogg_has_identical_channels` is also worth a look: they check whether the two channels came out identical, which is the same channel-independence worry recorded above.

**C. Ogg Opus via `opus`/`audiopus`.** ADR-0032's own recommended-then-declined option, and the better speech codec per byte — a 69-hour recording that cost 6 GB in AAC-192k would be a fraction of that. Symphonia demuxes Ogg but cannot decode Opus, so readback demuxes with symphonia and decodes packets with the same `opus` crate that encodes them. Two uses of one permissive dependency, no sidecar.

## The question that decides B vs C

**Does the L=mic / R=system split survive the encoder?** ADR-0029's split is "preserved into storage" precisely so that "Enhance-era re-transcription and re-diarization get cleanly separated per-channel sources forever". Both Vorbis and Opus apply joint-stereo coupling by default, which is designed for *correlated* channels — and these two are deliberately uncorrelated sources. Coupling would smear one leg into the other and quietly break the guarantee the stereo layout exists to provide.

Three ways out, and this needs settling before a codec is chosen:

1. **Opus uncoupled multistream** — **available, and verified 2026-09-05.** The `opus` crate exposes `MSEncoder::new(sample_rate, streams, coupled_streams, mapping, application)`; `streams=2, coupled_streams=0, mapping=&[0,1]` encodes two fully independent mono streams into one file, and `MSDecoder` reads them back. `audiopus_sys` binds the whole C multistream API as a fallback. Exact independence, still one file, no schema change.
2. **Two mono tracks**, one per leg — what anarlog does ("dual-track"). Exact independence under any codec, at the cost of two files per Meeting and a schema question (`audio_path` is currently one column).
3. **Accept coupling** — only if someone establishes the smearing is below what Diarization and re-transcription care about. That is a measurement, not an opinion.

## What collapses if the codec changes

Worth pricing, because it is most of `audio/sink.rs`:

- **The checkpoint-and-merge design exists because MP4 needs a moov atom.** A killed encoder leaves an unplayable container, so audio is written in 30s sealed segments and concatenated at the end. Ogg is page-based: a truncated file plays to its last complete page. `CheckpointSink`'s rolling encoder, `merge_checkpoints`, `recover_interrupted`, `Recovery`, and the audio half of `reconcile_after_restart` all reduce to "keep writing, and on restart the file is already valid."
- **Crash loss drops from ≤30 s to ≤ one page** (milliseconds).
- **The whole packaging surface added on 2026-09-05 deletes**: the staging step, the fourth signed binary, `extraResources`, the CI guard entry, the `FFMPEG_URL`/`FFMPEG_SHA256` variables, and the LGPL decision in `packaging/README.md`.
- **`ffmpeg_available()` and `EVERTRANSCRIPT_FFMPEG` go with it.**

## Migration

The record is immutable (ADR-0009) and portable (ADR-0035), so existing `.m4a` files must keep playing forever:

- symphonia keeps `aac` + `isomp4` for reading Meetings recorded before the switch, and gains `vorbis`/`ogg` (option B) or the Ogg demux plus an `opus` decoder (option C).
- New Meetings write the new container; `meetings.audio_path` already carries the filename with its extension, so nothing else in the schema or the Mirror needs to know which era a Meeting belongs to.
- No re-encoding of anything already recorded. A mixed History is the expected steady state.

## Decision (Operator, 2026-09-05)

**Kept Audio becomes one MP3 per Meeting, encoded in-process by `mp3lame-encoder` as capture runs. ffmpeg goes.**

Concretely: the sink pipes the joiner's interleaved `f32` straight into LAME the way it pipes into ffmpeg today — no WAV on disk, no intermediate, no post-meeting encode step. `Mode::DaulChannel` (the crate spells it that way) is LAME's *uncoupled* stereo, so the ADR-0029 split survives storage: left is the microphone, right is the system, and neither is smeared into the other by joint-stereo coupling. Readback for Diarization is `symphonia-bundle-mp3`, a feature flag on a dependency already present.

Of anarlog's three crates, this takes one. `hound` is already here and stays where it is — a dev-dependency of the Core and a real one of `evertranscript-fixtures`, both test-only, never shipped. `vorbis_rs` has no role once MP3 is the kept format, and is not adopted.

### Why this shape

Streaming rather than anarlog's record-WAV-then-encode. Their intermediate is load-bearing for them — it is the safety net when an MP3 encode fails (`disk.rs`: `Err(error) => tracing::error!("Encoding to mp3 failed, keeping WAV: …")`, and the WAV is deleted only on success) and it feeds two output formats. Here it would buy neither, and cost a great deal: the 69.4-hour Meeting recorded on this machine on 2026-09-01 — silent Auto-Record, left open across three days, 6.0 GB as AAC-192k — would have carried a WAV intermediate of 48 GB at i16 or 96 GB at f32. Streaming MP3 at ~128k stereo puts that same Meeting near 4 GB, in the same range as what is already on disk, and the disk-exhaustion question mostly dissolves with it.

It also keeps the crash story. MP3 is a frame stream: a file truncated by a kill plays up to its last complete frame, the same property that made Ogg attractive and that MP4 lacks. So `CheckpointSink`'s rolling encoder, `merge_checkpoints`, `recover_interrupted` and `Recovery` still dissolve into "keep writing", and crash loss drops from ≤30 s to one frame.

### What it costs, recorded so it is not rediscovered

- **The licence position gets heavier, not lighter.** `mp3lame-sys` compiles LAME statically into the Core, so LGPL-3.0 attaches to the binary this product signs and notarizes — the relink obligation squarely. Today's ffmpeg is LGPL/GPL behind a *process boundary*: a separate executable, substitutable by an Operator through `EVERTRANSCRIPT_FFMPEG`. That boundary is what made the obligation light, and linking gives it up. Recorded in `packaging/README.md` as an Operator responsibility. There is no permissive escape: every MP3 encoder reachable from Rust is LGPL at the library level — `mp3lame-encoder`/`-sys` (LGPL-3.0), `shine-rs` (LGPL-2.0); the `lame` crate declares MIT, but that covers its binding code, not libmp3lame.
- **MP3 is the weakest of the four codecs per byte for speech.** Opus was ADR-0032's own recommendation and remains the better technical answer; this trades that for MP3's universal playability. Bitrate wants a listening check against Transcription and Diarization quality rather than a guess.
- **MP3 was chosen over a format nothing else needs.** anarlog encodes MP3 to upload to cloud STT. ADR-0002 makes Transcription an Anchor and ADR-0001 closes the History boundary by removing the surface, so no such path exists here — the value MP3 carries for this product is that an Operator can play or send the file anywhere, which Ogg and AAC-in-MP4 do less well.

### What deletes with ffmpeg

The sidecar, `packaging/build.sh`'s staging step, the fourth signed binary, the `extraResources` entry, the CI guard entry, the `FFMPEG_URL`/`FFMPEG_SHA256` variables, the LGPL-ffmpeg hunt for macOS, `ffmpeg_available()` and `EVERTRANSCRIPT_FFMPEG` — and, with the checkpoint machinery, the `server.rs`-into-`sink` reaches recorded below.

### Verified before deciding (2026-09-05)

`Mode::DaulChannel` and `InterleavedPcm` both exist in `mp3lame-encoder`'s public API, so uncoupled stereo and the joiner's native buffer shape are both first-class. `symphonia-bundle-mp3` (11.1M downloads) covers readback. MP3's patents expired in 2017.

### Still open

1. **Bitrate**, per the note above.
2. **Disk exhaustion mid-Meeting.** Much smaller now, but not zero — a silent recorder still grows a file nobody is watching. Neither competitor guards this at all (anarlog propagates the write error and fails its recorder actor; Meetily has nothing), and this product already has the parts: `models::free_space_bytes()` and the `checked_add(HEADROOM)` pattern at `provision.rs:76`. The behaviour that matches ADR-0019 is to stop writing audio, keep transcribing, and put the reason in `audio_notes` — which is what the sink already does when its encoder will not start.
3. **Migration.** Existing `.m4a` Meetings must keep playing: symphonia keeps `aac` + `isomp4` and gains `mp3`. New Meetings write `.mp3`; `meetings.audio_path` already carries the extension, so nothing else needs to know which era a Meeting belongs to. Nothing is re-encoded, and a mixed History is the steady state.

## Recommendation (not taken)

**Option C — Ogg Opus, uncoupled multistream.** The one thing that could have ruled it out is available and fits the existing pipeline exactly:

- Capture is already 48 kHz, which is libopus's native rate — no resampling anywhere.
- The joiner already produces interleaved `f32`, which is what `MSEncoder::encode_float` takes — no conversion.
- `coupled_streams=0` preserves the ADR-0029 split exactly, in one file, with no schema change.

It is also what ADR-0032 recommended before declining it, the strongest speech codec per byte for a product that keeps every meeting forever, and the only option whose container removes the crash-recovery machinery rather than requiring it.

**Option B is the lower-risk fallback** — symphonia decodes Vorbis today, so the readback path needs no new decoder, and anarlog is a working precedent.

**Option A stays live** and is the right answer if the post-v1 Enhance/import surface is judged near enough to matter, because that surface is decode breadth and no Rust codec crate provides it.

## On reusing anarlog rather than reading it

Settled 2026-09-05: **read, never depend.** None of anarlog's eleven audio crates are published — they are workspace-internal path members, so a dependency would be a git pin on a competitor's internals with no version, changelog or stability promise, dragging `specta` (Tauri) and `libpulse-binding` (Linux) into a macOS/Windows Electron product. The licence is compatible (anarlog MIT, this repo Apache-2.0) and is not the obstacle. This continues the precedent ADR-0032 set with Meetily's chunk-and-merge and ADR-0025 with Granola's `mic_monitor_v2`: competitors are prior art, and prior art gets read.

## Open questions

1. ~~Does `opus`/`audiopus` expose uncoupled multistream encoding?~~ **Answered 2026-09-05: yes** — `opus::MSEncoder`, with the coupling parameter in its constructor. This no longer blocks C.
2. Is the post-v1 Enhance/import surface near enough to keep ffmpeg for? It is the one thing no Rust codec crate provides, and the only remaining argument for A.
3. If A: minimal build, or full-LGPL to keep the decode breadth that is the reason for choosing A at all?
4. Bitrate for Opus. AAC-192k produced 6 GB for a 69-hour orphan; speech-tuned Opus at 48–64k/stream would be a fraction. Wants a listening check against Transcription and Diarization quality, not a guess.

## One thing rides on this decision

`server.rs` reaches past `Recorder` into its collaborator the sink — `recover_interrupted`, `Recovery::Recovered`, `CHECKPOINT_SECONDS` — which is the only real API-discipline leak in the audio module (measured 2026-09-05: the compile-time and file-size complaints did not survive checking). **The cleanup is deliberately deferred to this decision**, because options B and C delete `CheckpointSink`'s rolling encoder, `merge_checkpoints`, `recover_interrupted` and `Recovery` outright, and with them every one of those reaches. Refactoring a seam around code that is a candidate for deletion would be doing it twice.

The risk of that sequencing, recorded so it cannot become permanent by default: **if this decision stalls, the leak stays.** If option A wins, or if no decision lands by the time capture is next touched, move recovery behind `Recorder::recover(audio_dir)` and have capture return the note's wording rather than exporting the constant.

## Until this resolves

No ffmpeg build is pinned. The code fixes from 2026-09-05 stand on their own and are correct under either outcome — resolving the encoder beside the binary before `PATH`, and making an encoder that will not start reach `audio_notes` instead of a `warn!` nobody reads. The CI packaging guard now requires an encoder in the artifact, which is true under ADR-0032 as written: a release built today without one records silence. It will fail a tagged release until either an ffmpeg is provisioned (A) or the dependency is removed (B/C).
