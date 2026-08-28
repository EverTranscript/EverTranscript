# 06: Notifications — the heads-up, and the meeting that never started

**What to build:** The two moments Auto-Record has to speak: a heads-up when a scheduled meeting starts, and a single follow-up when a calendar-armed Meeting never triggered. Gated so the product is never a nag.

**Blocked by:** 03.

Status: done — delivery is a seam, not a desktop banner

- [x] The heads-up at a calendar-armed meeting's scheduled start, and the follow-up, both reach a `Notifier` seam with the gates in front of it. **The gates and the catalog are done; a desktop delivery is not** — the shipped implementation is `SilentNotifier`, so nothing is claimed to appear on screen that has not been seen there
- [x] The armed-but-untriggered follow-up: a calendar-armed Meeting with no trigger by ~2 minutes past start prompts **once** ("'<title>' seems to be happening — nothing is recording"), and the pre-created Meeting is discarded if ignored
- [x] Gates from the prior art: a cooldown between prompts (~2 min), a per-app silence list, and suppress-while-recording — the product does not narrate a meeting it is already capturing
- [x] DND is honoured **best-effort by construction**: the macOS mechanism is undocumented and the catalog flags one competitor's check as hardcoded `false`. Failure to read DND must degrade to notifying, never to silence — a missed notification is recoverable, a silently non-recording product is not
- [x] Dedup keys so one meeting cannot produce two heads-ups from two senses agreeing with each other
- [x] Every string externalized (English + Simplified Chinese catalogs), unchanged from the M1 rule
- [x] No notification fires before the Briefing acknowledgment
