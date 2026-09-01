# 01: Clearing a name means unnamed

**What to build:** An Operator who clears a Meeting's name gets a Meeting that is
genuinely unnamed — not one wearing an invisible empty string. Today the two states
render identically and behave differently underneath; after this ticket there is one
state. This is the escape hatch the write-once Suggested Title (ticket 02) depends
on: without it, clearing a name would block the suggestion slot forever.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Retitling with empty or whitespace-only text stores an absent name, not an empty string
- [ ] Through the protocol, a cleared Meeting and a never-named Meeting are indistinguishable
- [ ] Retitling with real text stores, announces, and renames the Mirror exactly as before
- [ ] The Client needs no change — it already renders both states as the placeholder; verified rather than assumed
- [ ] Behavior-named tests at the protocol seam; the local gate is green
