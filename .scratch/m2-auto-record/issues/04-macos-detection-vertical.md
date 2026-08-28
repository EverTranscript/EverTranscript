# 04: macOS detection — running apps and per-process microphone attribution

**What to build:** The live DetectionSource for macOS: NSWorkspace running-application observation for process entries, and per-process CoreAudio microphone attribution, which serves both the AND-mic condition and Browser Meetings.

**Blocked by:** 01, 02.

Status: done

- [x] **NSWorkspace turned out to be unnecessary, which is a finding rather than a shortcut.** CoreAudio's process objects carry both halves of the trigger at once — the bundle id and whether the process is recording — and holding the microphone is strictly stronger evidence than being frontmost. Observing activation as well would add a signal the policy must ignore to stay correct. **No window titles and no screen content** — ADR-0027 removed the grant ADR-0024's title-based Google Meet detection rode on, and it is not coming back by the side door
- [x] Per-process microphone attribution via CoreAudio, collapsing mic-active processes to their responsible app through the helper table from 02, so a Chrome renderer helper is reported as Chrome
- [x] Browser Meetings triggers when **any** browser holds a hot microphone, covering Google Meet plus the web variants of Zoom, Teams and Webex for free (ADR-0030)
- [x] Loopback and virtual audio devices are filtered by UID, so a virtual device is never mistaken for a person talking
- [x] Detection survives device churn the way capture already does: re-attach the per-device is-running listener on default-device change, guard stale-device callbacks, and poll as a fallback. The AirPods swap that must not split a Meeting is the same event that must not blind the detector
- [x] The opt-in Screen Recording grant for precise browser labels is a Settings affordance only (ADR-0030) — absent it, a browser Meeting is labelled with the browser's name and that is a complete, shipping behaviour
- [x] The permission set stays exactly microphone + system-audio recording in the default posture; the guarantee-test audit still passes unchanged
- [x] Verified against the real machine, not only fixtures: each shipped Watchlist entry observed triggering and releasing, with the observation recorded
