# EverTranscript — Meeting Notetaker

Shared language for EverTranscript, a meeting notetaker for macOS and Windows that never misses a meeting and whose defining guarantee is that meeting history never reaches the cloud, by construction.

## Language

### People

**Operator**:
The person running the app — the only party the product can communicate with or act through.
_Avoid_: user, host

**Participant**:
Anyone else on the call. Their voice is recorded, transcribed, and diarized; the product has no channel to them.
_Avoid_: attendee, counterparty

**Speaker**:
A persistent voice identity, created automatically the first time a voice is heard and recognized across Meetings thereafter. Pseudonymous ("Speaker A") until the Operator names it; naming retroactively labels all past appearances.

**Voiceprint**:
The stored voice embedding by which a Speaker is recognized in later Meetings. Biometric data; one exists for every Speaker.
_Avoid_: embedding, voice profile

**Voice Registry**:
The inspectable inventory of every Speaker and Voiceprint, with per-Speaker controls.

### The record

**Meeting**:
One recorded session, from record-start to record-stop; the unit of storage, retrieval, and summary.
_Avoid_: call, session

**Transcript**:
The timestamped text record of a Meeting produced by live transcription.

**Diarization**:
Post-meeting attribution of Transcript segments to Speakers. It is never live.

**Summary**:
Post-meeting notes generated from a single Meeting's Transcript and Notes.

**Notes**:
Operator-authored writing attached to a Meeting — jotted live or added after. Freely editable forever; feeds Summary generation as steering context.
_Avoid_: annotations, comments

**History**:
The corpus of all past Meetings — audio, transcripts, diarization, Summaries, Notes — and the Voiceprints that recognize its Speakers. It exists only on the Operator's machine.
_Avoid_: archive, library

**Mirror**:
The per-Meeting Markdown file — a regenerable projection of the record, greppable and syncable, never independently edited.
_Avoid_: export, markdown copy

**Current Meeting**:
The Meeting in progress. The only content any cloud Backend can ever receive.
_Avoid_: current call

### Detection

**Meeting Detection**:
The product's single ambient sense: identifying an active meeting from application and microphone state. It reads state, never content.
_Avoid_: auto-detect, watching, monitoring

**Auto-Record**:
The standing policy — on by default, revocable — that Meeting Detection starts and stops recording without a per-meeting act.
_Avoid_: auto-start, background recording

**Watchlist**:
The Operator-visible, extensible list of meeting apps that Meeting Detection watches. Ships with Zoom, Microsoft Teams, VooV Meeting, and Browser Meetings; known call apps (WeChat) ship as suggested entries, one tap to add — membership is the per-app switch.
_Avoid_: app list, detection rules

**Browser Meetings**:
The single Watchlist entry standing for every browser-hosted call — it matches a browser in a call rather than a specific site, covering Google Meet and the web variants of the desktop meeting apps.

### System

**Core**:
The always-on Rust process — the login item — that detects, records, transcribes, diarizes, and stores. The record's only writer.
_Avoid_: daemon, engine, service

**Client**:
Any surface that commands and reads the Core — the Electron app and the CLI. Clients never touch storage directly.
_Avoid_: frontend, app

### Backends

**Backend**:
The model provider a feature runs against — local (on-machine) or cloud (remote API).

**Anchor**:
A feature permanently fixed to the local Backend, with no selector. The Anchors are live transcription and Diarization.

**Knob**:
The Backend selector. Exists on exactly one feature: Summary.
_Avoid_: switch, toggle

**Fallback**:
The automatic cloud→local Backend switch on Backend failure. The reverse direction never happens automatically.

**Strict Mode**:
A setting that disables Fallback and surfaces the failure instead.

### Guarantees

**Closed Boundary**:
The property that History reaches the cloud through no code path — the surface is removed, not guarded.
_Avoid_: redaction boundary (nothing is redacted, because nothing crosses)

**Nothing Ambient**:
The input-side twin of the Closed Boundary, narrowed by Auto-Record: the product processes no content it wasn't explicitly handed — no calendar, screen, filesystem, or contacts. Its one ambient sense is Meeting Detection, which reads state, never content.
_Avoid_: passive capture, background monitoring

**Sanctioned Traffic**:
The enumerable, content-free network calls the product may ever make: the disableable update check, checksummed model downloads, and the cloud Backend the Operator chose. Beyond this list, the wire is silent.
_Avoid_: allowlist, phoning home

### Onboarding

**Briefing**:
The one-time first-run legal education — recording consent, voice profiling, and Auto-Record disclosure — ending in an explicit Operator acknowledgment. Nothing is captured before it.
