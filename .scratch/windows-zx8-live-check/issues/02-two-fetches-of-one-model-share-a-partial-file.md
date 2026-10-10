# 02: Two downloads of the same model write one partial file

Status: ready-for-agent

Priority: low. The Electron client never sends `models/fetch`, so this
happens only when someone runs `evertranscript models fetch` while the Core
is still downloading at startup.

Found during the live v1.0.1 check on windows-zx8 on 2026-10-09 (Q313). The
code is the same on `main`.

## What happened

- At 22:48:49 UTC the Core started fetching its 4 missing models
  (`provision_missing_models`, `server.rs:3876`).
- At 22:48:55 `evertranscript models fetch whisper-large-v3-turbo-q8_0`
  started a second download of the same model (`server.rs:4294`).
- At 22:50:02 the Core logged `model ready` for that model.
- The CLI download then failed with "whisper-large-v3-turbo-q8_0 failed
  verification and was discarded; run the fetch again". The model on disk
  was fine, and transcription used it.

## Why

- `fetch_models` (`server.rs:3969`) does not stop a second download that
  starts while one is running. Both call `Downloader::fetch`
  (`models/mod.rs:307`) on the same entry.
- The second download finds the first one's partial file, resumes from its
  length, and writes into the same file (`models/mod.rs:362`).
- The first download to finish renames the partial file into place
  (`:422`). The other one then tries to verify a partial file that no longer
  exists, and reports a failed verification (`:412`). Which one wins is a
  race: either the CLI or the Core can be the one that reports the failure.
- `fetch_models` keeps one cancel token in `fetching` (`server.rs:157`). The
  second download replaces the first one's token, and whichever ends first
  clears it. So `cancel_fetch` (`:3983`) stops only the newest download, or
  none.

## Do

- Let only one `fetch_models` run at a time. For example, hold a
  `tokio::sync::Mutex<()>` across `fetch_models_inner`, and set the cancel
  token after the lock is held. A second request then waits, finds the model
  `Ready`, and returns without downloading.
- A request that waits behind a running download must still honor
  `cancel_fetch`. Do not leave a queued download running after a cancel.
- Test: start two `fetch_models` calls for one model against a slow local
  server. Both must succeed, and the server must see one full download.

## Comments
