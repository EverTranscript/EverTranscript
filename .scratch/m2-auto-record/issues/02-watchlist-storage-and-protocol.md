# 02: The Watchlist — storage, protocol, CLI

**What to build:** The Operator-visible, extensible list of meeting apps Meeting Detection watches, with the shipped defaults of ADR-0030, reachable and editable from the CLI. This is the data detection consults; the surfaces that make it pretty are ticket 08.

Status: ready-for-agent

- [ ] Watchlist rows in the machine store (not the History folder — the Watchlist describes this installation, the way settings do), with the platform identity per row: bundle id on macOS, exe name on Windows, and the `browser-meetings` sentinel
- [ ] Shipped defaults exactly per ADR-0030: Zoom, Microsoft Teams, VooV Meeting (both VooV International and 腾讯会议), and **Browser Meetings** as a single entry
- [ ] WeChat ships as a **suggested** entry — present, off, one act to add. Membership is the per-app switch: no per-app toggle column appears beside the single Auto-Record switch (ADR-0023)
- [ ] The false-trigger blocklist ships as negative seed data (dictation apps, IDEs, screen recorders, AI assistants) — a hot microphone in Cursor is not a meeting
- [ ] The helper→responsible-app table ships as seed data, ported with attribution and a `PORTS.md` entry (~25 Chromium/Electron helper bundle ids; the Windows exe→id twin lands with 05)
- [ ] Protocol: read and edit the Watchlist over the ADR-0028 surface, additively — `watchlist/list`, add, remove, and the suggested-entry promotion. Bindings and schema fixtures regenerate and are committed
- [ ] CLI: `evertranscript watchlist [list|add|remove]`, speaking to the running Core like every other subcommand (story 16)
- [ ] Adding or removing an app takes effect without restarting the Core: the Operator's edit is a live act, not a next-launch one
