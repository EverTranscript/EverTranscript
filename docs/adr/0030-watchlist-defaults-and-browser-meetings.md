# Watchlist defaults: VooV joins, WeChat ships suggested-not-default, browser meetings are one entry

> **Amended by ADR-0036:** when a granted local-calendar event overlaps a Meeting, its title names the Meeting at birth — the title chain is manual > calendar > transcript suggestion > detected-app placeholder.

The shipped default Watchlist becomes **Zoom, Microsoft Teams, VooV Meeting (both VooV International and 腾讯会议), and Browser Meetings** — a single entry that triggers when any browser process holds a hot microphone (per-process CoreAudio attribution, helper processes mapped to the responsible app; the mechanism Granola and anarlog both ship). Browser Meetings replaces ADR-0024's window-title Google Meet detection, whose permission ADR-0027 removed — and covers Zoom-web, Teams-web, and Webex-web for free, which the process-name entries never did. It reads state, never content: Nothing Ambient holds.

**WeChat ships as a suggested entry, off by default, one tap to add** — membership is the per-app switch, so no new mechanism and no per-app toggle column beside ADR-0023's single Auto-Record switch. Default-recording personal 1:1 calls is the wiretap story ADR-0024's any-mic-use rejection named, and widens all-party-consent exposure well beyond meetings; "record basically all calls" stays reachable by the Operator's own act, never the shipped posture. ADR-0024's AND-mic trigger and its any-mic-use rejection stand unchanged.

## Considered options

WeChat on by default (maximal never-miss; rejected for the consent posture and for voice-message false positives — an 8-second push-to-talk message becoming a Meeting). A per-app enable column (new mechanism, muddies the single Auto-Record switch). Keeping title-based Meet detection via a required Screen Recording grant (drags the scariest permission back into every install). A browser extension with a native-messaging host (anarlog's approach — precise URL/mute state, but per-browser store distribution; deferred post-v1).

## Consequences

- A Browser Meetings recording labels as the browser ("Chrome") until the post-meeting title suggestion names it; precise per-site labels are available via an **opt-in** Screen Recording grant in Settings ("name browser meetings precisely") — ADR-0024's mechanism demoted from load-bearing to optional.
- A browser voice app that isn't a meeting (Discord web, a dictation site) can trigger; the entry is a visible, removable row and whole-Meeting delete is the triage.
- The M2 detection test matrix becomes per-browser mic attribution (Chrome, Safari, Arc, Edge — helpers correctly attributed) instead of locale-sensitive title parsing.
- Windows mic attribution is v1 work in M2 (ADR-0025 as amended) — anarlog's Windows detector is a no-op stub, but Granola ships Windows mic-session attribution (`mic_monitor_v2`, mined from the shared JS bundle: backoff, give-up, exe→app table) as real prior art; the Windows column cannot be hollow, and it is the ship gate.
