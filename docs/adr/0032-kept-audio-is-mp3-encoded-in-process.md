# Kept audio is one MP3 per Meeting, encoded in-process by LAME

> **Reversed 2026-09-05, from stereo AAC written by a bundled ffmpeg.** The
> original decision and the reasoning that produced it are kept below rather
> than deleted: a record that quietly reflects only its current state is the
> first thing a reader stops trusting. The working notes are
> `.scratch/kept-audio-codec/spec.md`.

Each Meeting's kept audio (ADR-0019) is a single **MP3** file — **left = mic channel, right = system channel** (the ADR-0029 split, preserved into storage) — encoded **in the Core's own process** by LAME through `mp3lame-encoder`, streaming as capture runs. There is no encoder sidecar, no intermediate file, and no post-meeting encode step: the joiner's interleaved `f32` goes to the encoder the way it went to ffmpeg's stdin, and what lands on disk is the finished file.

Three properties make MP3 carry this rather than merely tolerate it. **`Mode::DaulChannel`** (LAME's uncoupled stereo, spelled that way by the crate) encodes the two channels independently, so the ADR-0029 split is preserved by the encoding itself rather than by how bits happened to be allocated — the guarantee that lets Enhance-era re-transcription and re-diarization get cleanly separated per-channel sources forever. Measured honestly: joint stereo would probably also survive at this bitrate, because mid/side reconstructs a hard-panned signal cleanly with bits to spare. It is chosen because these two legs are uncorrelated by construction, which is the case mid/side is worst at, and independence costs nothing. **It is a frame stream**, so a Core killed mid-Meeting leaves a file playable to its last complete frame; there is no moov atom to lose, and therefore no chunk-and-merge machinery, no checkpoint directory, and no recovery pass. **And it plays everywhere** — the one thing MP3 is unambiguously better at than AAC, Vorbis or Opus, which matters for a file the Operator owns and may hand to somebody.

Readback for Diarization is `symphonia-bundle-mp3`, a feature on a dependency already present; nothing new decodes.

## Considered options

**Ogg Opus via `opus::MSEncoder`** was recommended twice and declined twice. It is the better technical answer — 48 kHz native (our capture rate exactly), interleaved `f32` input (our buffer shape exactly), `coupled_streams=0` for exact channel independence, permissively licensed (MIT/Apache-2.0 over BSD-3 libopus), and the strongest speech codec per byte for a product that keeps every meeting forever. It loses on reach: an Operator who wants to send a recording to someone is better served by a file that opens anywhere.

**Ogg Vorbis via `vorbis_rs`** (BSD-3, and what anarlog ships) has the same shape and the advantage that Symphonia already decodes it. Set aside once MP3 was chosen as the kept format, since a second lossy format earns nothing.

**Keeping ffmpeg, with a minimal LGPL build produced in CI**, was the alternative that preserved the original decision. It stays viable and is the right answer if the post-v1 Enhance/import surface arrives, because decode breadth is the one thing no Rust codec crate provides.

**anarlog's record-WAV-then-encode** was declined again, and the measurement is why. The 69.4-hour Meeting this machine actually recorded on 2026-09-01 — silent Auto-Record, left open across three days — would have carried a WAV intermediate of 48 GB at i16 or 96 GB at f32. Their intermediate is load-bearing for them (it is the fallback when an MP3 encode fails, and it feeds two output formats); here it would buy neither.

## What this reverses, and why it went the other way in August

The 2026-08-27 decision was a **bundled ffmpeg binary managed by the Core**, chosen over the then-recommended pure-Rust Ogg/Opus route: ffmpeg cost "a ~7MB+ sidecar and its LGPL-build/update surface" and bought "battle-tested encoding across real-world device churn (the Bluetooth reconnect class of bugs) plus broad decode for the post-v1 Enhance/import family."

Packaging it for the first time falsified most of that. **Nothing had ever staged an ffmpeg binary** — `ffmpeg_binary()` returned a bare name, so a developer's Homebrew build answered on every machine anyone tested on while a Finder-launched Core, inheriting almost no `PATH`, found nothing and recorded every Meeting with no audio at all, behind one `warn!` that reached no log and no window. The two things the sidecar was bought for turned out to be carried by something else: **churn** is resolved upstream by `supervisor.rs` and `joiner.rs`, so the encoder receives already-joined blocks at a fixed rate and never sees a device; **decode** is served by symphonia in `diarize/runner.rs`. And the cost was priced low — there is no mainstream prebuilt *LGPL* ffmpeg for macOS at all, so honouring the licence meant building one from source in CI for two platforms and signing it.

## Consequences

- **The sidecar goes, and everything that carried it**: `packaging/build.sh`'s staging step, the fourth signed binary, the `extraResources` entry, the CI artifact-guard entry, the `FFMPEG_URL`/`FFMPEG_SHA256` variables, `ffmpeg_available()`, and `EVERTRANSCRIPT_FFMPEG`.
- **The crash-safety machinery goes with it.** `CheckpointSink`'s rolling encoder, `merge_checkpoints`, `recover_interrupted` and `Recovery` exist because MP4 needs a moov atom. Crash loss drops from at most one 30s checkpoint to one frame, and the `server.rs`-into-`sink` reaches disappear with the code they reached for.
- **The licence obligation gets heavier, not lighter, and this was chosen knowing that.** `mp3lame-sys` compiles LAME statically into the Core, so LGPL-3.0 attaches to the binary this product signs and notarizes — the relink obligation squarely, where ffmpeg's was light precisely because it was a separate executable an Operator could substitute. There is no permissive escape: every MP3 encoder reachable from Rust is LGPL at the library level, and the `lame` crate's MIT covers its binding code rather than libmp3lame. `packaging/README.md` carries this as an Operator responsibility.
- **ADR-0019's size estimate is corrected by measurement.** This ADR previously recorded "roughly 20–30MB/hr for the two channels"; the AAC-192k it specified is **86 MB/hr**, confirmed by a 69.4-hour recording occupying 6.0 GB. At stereo MP3 that is 58 MB/hr at 128k, 43 at 96k, and 29 at 64k — so the original figure was right about the budget and wrong about the bitrate that fits it. `docs/prd.md` cites this number.
- **Bitrate is 128 kbps** (Operator, 2026-09-05), split across two independently coded channels, so 64 kbps each. That is comfortable for speech and costs 58 MB/hr — a third less than the 86 MB/hr the AAC-192k it replaces actually took. It stays a named constant (`sink::BITRATE`) because it is the one lever over both disk and fidelity, and a listening check against Transcription and Diarization quality could still move it.
- **Existing `.m4a` Meetings keep playing forever** (ADR-0009, ADR-0035). Symphonia keeps `aac` + `isomp4` and gains `mp3`; new Meetings write `.mp3`; `meetings.audio_path` already carries the extension, so nothing else needs to know which era a Meeting belongs to. Nothing is re-encoded, and a mixed History is the expected steady state.
- **A silent recorder still grows a file nobody watches.** Smaller now — that same Meeting lands near 4 GB rather than 48–96 — but not zero. Neither open-source competitor guards disk at all; the parts to do better already exist here in `models::free_space_bytes()` and the checked-add pattern at `provision.rs:76`, and ADR-0019 settles the behaviour: stop the audio, keep the transcript, say so in the record.
