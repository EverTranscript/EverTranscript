# Meeting Detection is a visible Watchlist AND-ed with microphone activity

> **Amended by ADR-0030:** the shipped Watchlist grows — VooV Meeting joins; a single Browser Meetings entry (any browser with a hot mic, per-process attribution) replaces title-based Google Meet detection after ADR-0027 removed the Screen Recording grant it rode on; WeChat ships suggested-not-default. The AND-mic trigger and the any-mic-use rejection below stand unchanged.

Detection triggers only when a Watchlist app is active AND the microphone is in use — both conditions, so an idle Zoom window doesn't record the office all day and a hot mic alone (dictation, voice memos) doesn't trigger either. The Watchlist ships with Zoom and Microsoft Teams (process detection) and Google Meet (browser window title — readable under the Screen Recording permission the app already holds for system-audio capture, so the sanctioned permission set is unchanged), and it is Operator-visible and extensible in Settings.

## Considered options

Any-mic-use recording (maximum recall, zero configuration) was rejected as a false-positive factory whose story reads as a wiretap. Watchlist-plus-prompt-on-unknown-mic-use is a candidate v1.1 refinement, not v1: it adds a second behavior to explain, and its prompt path reintroduces missed-prompt-equals-missed-meeting for off-list apps.
