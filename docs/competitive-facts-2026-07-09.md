# Competitive facts — verified 2026-07-09

Live-checked via product sites, docs, and GitHub API. For the future spec note; re-verify before quoting later.

| Product | Live transcription | Local record | Diarization | Notes/summary |
|---|---|---|---|---|
| Granola (cloud) | ✅ | ❌ cloud | ❌ (participant metadata) | ✅ cloud |
| anarlog (8,795★, MIT) | ✅ local | ✅ .md per meeting | ❌ | ✅ local/BYO |
| Meetily (21,873★, MIT) | ✅ local | ✅ | ⚠️ "planned for PRO" | ✅ local |

Notable:
- anarlog: team now builds **char** (char.com); anarlog remains MIT-maintained ("Granola, rearranged").
- Granola raised **$125M** ("put your company's context to work"); Briefs = pre-meeting history-aware prep, cloud-side, with inline citations.

Sources: granola.ai, granola.ai blog (2026-04-21 "Granola Chat just got smarter"), gh api fastrepl/anarlog + Zackriya-Solutions/meetily (2026-07-09).

## Feature-gap sweep (same day, deeper pass)

Verified feature inventories:

- **Granola**: calendar sync + pre-meeting Briefs; SIGNATURE: operator jots rough notes, AI enhances by weaving transcript + jottings; templates; agentic history chat w/ inline citations; action items pushed to Linear/CRM; Team Spaces; mobile (iOS/Android incl. phone calls); no bot (local capture, cloud processing).
- **anarlog** (v1.1.10 2026-07-08, near-daily releases): minimal local notetaker — local ASR, .md per meeting (sync via Dropbox/iCloud/git), BYO-LLM, self-host. Rust/Tauri.
- **Meetily**: Whisper OR Parakeet model choice; Import & Enhance (BETA: import audio files; RE-transcribe stored recordings with different model/language — implies AUDIO IS PERSISTED locally); pro audio mixing (ducking/clipping prevention); GPU accel. PRO: custom summary templates, PDF/DOCX/MD export, auto-detect AND auto-join meetings, speaker ID (coming), chat-with-meetings (coming), calendar (coming), self-host, GDPR audit trails. Borrowed code from whisper.cpp, Screenpipe, transcribe-rs; Parakeet via ONNX.

Gap decisions queued for grilling: operator notes, audio persistence, calendar, briefs, screen awareness, custom prompts, action items, templates (post-v1), coaching/mobile/team (non-goals).
