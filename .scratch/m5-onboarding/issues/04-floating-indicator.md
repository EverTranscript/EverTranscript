# 04: The floating indicator — evaluated, then decided

**What to build:** A decision, and the implementation only if the decision goes that way. ADR-0026 gave the always-visible indicator to the Core-owned tray deliberately; the PRD lists a Core-native floating mini-indicator as an M5 *evaluation*, not a commitment.

**Blocked by:** nothing.

Status: not started

- [ ] Evaluate against what the tray already provides. The tray is always visible, survives the Client being closed, and is owned by the Core — which is the process that actually knows whether recording is happening
- [ ] The catalog has the exact Electron recipe if the answer is yes: `type:'panel'`, `alwaysOnTop`, `focusable:false`, `skipTaskbar`, `hiddenInMissionControl`, `transparent`, runtime `setIgnoreMouseEvents`, and `setVisibleOnAllWorkspaces(true, {visibleOnFullScreen:true, skipTransformProcessType:true})` — the non-obvious combination that neither steals focus nor blocks clicks
- [ ] **A Client-owned indicator has a defect the tray does not**: it disappears when the Client is closed, while the Core keeps recording. An indicator that is absent precisely when someone has closed the window and forgotten they are recording is worse than no second indicator
- [ ] Record the decision either way (`DECISIONS.md`), because "we considered a floating indicator" is what the next person needs, not silence
- [ ] If built: it must never steal focus, never block a click, and appear on fullscreen spaces — the tray's own weakness is fullscreen, which is the case that would justify this
