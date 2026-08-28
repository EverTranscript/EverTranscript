# 08: The Watchlist and Auto-Record in the Client

**What to build:** The surfaces that make the ambient behaviour legible: a single Auto-Record switch, and a Watchlist the Operator can read and edit. The CLI equivalents shipped in 02; this is their visible half.

**Blocked by:** 02, 03.

Status: ready-for-agent

- [ ] A **single** visible Auto-Record switch (story 14, ADR-0023) — one legible act to turn the ambient behaviour off and back on. Not a per-app matrix
- [ ] The Watchlist is visible and editable in the Client (story 13): the Operator can always answer "what does this app watch?" by looking, and can add or remove a row
- [ ] Suggested entries (WeChat) appear as suggestions with one act to add, distinguishable from active rows
- [ ] The Client consumes only generated protocol types, unchanged from M1's rule — no hand-written wire types
- [ ] Editing the Watchlist from the Client and from the CLI concurrently behaves: the Core is the single writer and both surfaces see the same list (story 26 still holds)
- [ ] Every user-facing string externalized in both catalogs. **The settings surface this ticket introduces is the first one the Client has had** — the M1 settings that only the CLI can reach (`chinese_script`, `auto_record`) belong on it, or the Client has a settings screen that hides settings
- [ ] Visually verified on a machine with a display, and recorded as such — M1's ticket 10 was checked from behaviour alone and the box that said "launching the Client starts the Core" was exactly false
