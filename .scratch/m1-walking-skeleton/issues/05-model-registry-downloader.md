# 05: Model registry + downloader

**What to build:** The Core can fetch its models on any network: a registry enum (the default `ggml-large-v3-turbo-q8_0` Whisper model and the AEC ONNX pair, with exact byte sizes and pinned checksums), a downloader with HTTP-Range resume, per-chunk stall detection, and verify-then-promote — plus the Operator-configurable mirror URL and `evertranscript models fetch|status`.

**Blocked by:** 01.

**Status:** done

- [x] Registry entries carry filename, byte size, pinned checksum, and language coverage; the Whisper default and AEC models are listed
- [x] Download resumes after a kill/restart (Range + partial-file validation); a 30s per-chunk stall aborts with a legible network error
- [x] Checksum mismatch deletes the file and reports corruption; success promotes via atomic rename; magic-bytes triage before the full checksum
- [x] Mirror URL setting is honored end-to-end (Hugging Face primary; ModelScope-style mirror configurable)
- [x] `evertranscript models fetch` / `models status` work against the running Core; a not-ready signal is exposed on the protocol (consumed by the tray in ticket 09)
