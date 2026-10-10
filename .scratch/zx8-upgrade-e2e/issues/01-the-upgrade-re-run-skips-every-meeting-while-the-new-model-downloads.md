# 01: The upgrade's re-run skips every Meeting while the new speaker model downloads

Status: ready-for-agent

Priority: high. It breaks the one promise ADR-0037 makes to people upgrading
from 1.0.x, and the 1.1.1 release notes repeat that promise: "Named Speakers
come back as their Meetings are re-run."

Found driving the 1.0.1 → 1.1.1 upgrade on windows-zx8 on 2026-10-10 (Q319,
Q320). The code is the same on `main` (d6ddd18).

## What happened

- 1.0.1 recorded one Meeting with Kept Audio. One voice in it was named
  "Alice Test", which confirmed its Voiceprint.
- The 1.1.1 installer upgraded the app in place. On its first start the Core
  wiped every Voiceprint, as designed, and began the re-run at 10:04:24.8.
- The Voices panel said "Update finished. 1 processed". But the Meeting's
  `diarized_at` was still 09:59:36 (the 1.0.1 run), `speaker_exemplars` was
  empty, and Alice Test had no Voiceprint.
- The new `diarize-embedding.onnx` (18,045,013 bytes) finished writing at
  10:04:34.4, ten seconds after the re-run started.

## Why

- `lib.rs:90` queues the re-run at boot, and the diarization worker starts on
  it straight away.
- At the same time `fetch_models` (`server.rs:4022`) finds the old 25 MB
  WeSpeaker file at the same path, calls it `Corrupted` because of its size,
  deletes it and downloads the new one.
- While the file is missing, the run returns `Skipped` with "diarization
  models are not downloaded" (`server.rs:1961`).
- `leaves_the_line_afterwards` (`server.rs:342`) removes a `Skipped` Meeting
  from the queue for good. Its doc treats "no models" as a Meeting that
  "genuinely cannot be processed", but here the models are missing for only a
  few seconds.
- A skipped Meeting costs nothing, so the whole backlog drains inside the
  download window. On a real History every Meeting would be skipped this way.

## Confirmed

On the test copy only, with the Core stopped, the `diarize_rerun` row was
reset to the stamp the upgrade migration seeds
(`wespeaker-voxceleb-resnet34-LM`/`2`, total 0). With the model already on
disk, the next start diarized the Meeting in 2 s (`diarized_at` 10:09:41), and
Alice Test got a `redimnet2-b3` Voiceprint from one 5,190 ms exemplar. So
`reseed` itself works; only the race defeats it.

## What to do

1. **Fix the race.** A Meeting skipped because the models are missing should
   stay owed, at least while a download is pending or a re-run is running.
   Two candidate shapes: answer "owed" instead of `Skipped` for missing models
   on bulk work and wake the worker when a model becomes ready; or hold the
   worker until every required model is `Ready`. Log the choice.
2. **Recover Histories that already ran 1.1.1.** For them the re-run row
   already carries the new stamp, so nothing will walk History again. One
   option: at start, re-queue every Meeting with Kept Audio whose
   `diarized_at` is older than the current stamp's `started_at`. This needs a
   decision, because it re-runs Meetings for anyone who upgraded.
3. **Test it.** A Core test that starts with the embedding model missing and
   the re-run owed, then provides the model, and asserts the Meeting is
   diarized and a named Speaker gets a Voiceprint. It must fail on `main`.
4. **Fix the stale doc** at the top of `diarize/reseed.rs`, which says
   "Nothing calls this". `server.rs:1985` calls it.

## Not checked

- The macOS upgrade. The code path is shared, so it should behave the same.
- A History with many Meetings. The test had one.
