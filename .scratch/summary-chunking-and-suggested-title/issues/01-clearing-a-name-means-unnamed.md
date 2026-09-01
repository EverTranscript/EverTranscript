# 01: Clearing a name means unnamed

**What to build:** An Operator who clears a Meeting's name gets a Meeting that is
genuinely unnamed — not one wearing an invisible empty string. Today the two states
render identically and behave differently underneath; after this ticket there is one
state. This is the escape hatch the write-once Suggested Title (ticket 02) depends
on: without it, clearing a name would block the suggestion slot forever.

**Blocked by:** None (can start immediately).

**Status:** done

- [x] Retitling with empty or whitespace-only text stores an absent name, not an empty string. Normalised in `retitle` — the Core is the only writer (ADR-0026), so one place suffices
- [x] Through the protocol, a cleared Meeting and a never-named Meeting are indistinguishable. Asserted both in the write's own answer and over a fresh `meeting/get`, against `""`, spaces and tabs/newlines
- [x] Retitling with real text stores, announces, and renames the Mirror exactly as before — the existing retitle test is untouched and green
- [x] The Client needs no change — verified, not assumed: `displayTitle` guards `title && title.trim()`, and the Mirror's filename builder already filtered empty titles, so a cleared name takes the same path a never-named one always did
- [x] Two behaviour-named tests at the protocol seam; both failed with `Some("")` before the change; the local gate is green
