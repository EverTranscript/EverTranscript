# 06: Notifications — the heads-up, and the meeting that never started

**What to build:** The two moments Auto-Record has to speak: a heads-up when a scheduled meeting starts, and a single follow-up when a calendar-armed Meeting never triggered. Gated so the product is never a nag.

**Blocked by:** 03.

Status: ready-for-agent

- [ ] The heads-up notification at a calendar-armed meeting's scheduled start (ADR-0036). The Core **never auto-launches the Client** (ADR-0026) — a notification is what it does instead
- [ ] The armed-but-untriggered follow-up: a calendar-armed Meeting with no trigger by ~2 minutes past start prompts **once** ("'<title>' seems to be happening — nothing is recording"), and the pre-created Meeting is discarded if ignored
- [ ] Gates from the prior art: a cooldown between prompts (~2 min), a per-app silence list, and suppress-while-recording — the product does not narrate a meeting it is already capturing
- [ ] DND is honoured **best-effort by construction**: the macOS mechanism is undocumented and the catalog flags one competitor's check as hardcoded `false`. Failure to read DND must degrade to notifying, never to silence — a missed notification is recoverable, a silently non-recording product is not
- [ ] Dedup keys so one meeting cannot produce two heads-ups from two senses agreeing with each other
- [ ] Every string externalized (English + Simplified Chinese catalogs), unchanged from the M1 rule
- [ ] No notification fires before the Briefing acknowledgment
