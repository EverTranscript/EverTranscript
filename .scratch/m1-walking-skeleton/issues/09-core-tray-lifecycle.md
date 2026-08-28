# 09: Core tray + lifecycle

**What to build:** The Core becomes the always-on login item the glossary describes: a UI-capable agent owning the tray (record/stop, state machine with transitional items and a not-ready gate during model downloads, explicit Quit), the registration-only launch-at-login toggle, the first-run acknowledgment gate, and the capture permission requests — nothing captured before the ack, ever.

**Blocked by:** 03.

**Status:** ready-for-agent

- [ ] Tray shows the state machine (stopped/starting/recording/stopping + transitional disabled items that revert on error); record/stop work from it
- [ ] Not-ready gate: while required models are missing, the menu swaps to a legible "downloading model" state (signal from ticket 05)
- [ ] Quit stops the running Core now; the launch-at-login toggle (Settings-surface protocol method + `evertranscript autostart on|off`) changes SMAppService / Run-key registration only — verified as three separate acts (story 9c)
- [ ] First-run acknowledgment dialog gates all capture: `record start` before acknowledgment is refused with a legible error (the M1 stand-in for the Briefing)
- [ ] Microphone and system-audio permissions requested at first record with plain explanations; silent preflight checks are separate from prompting requests
- [ ] Works on both platforms (tray + Run key on Windows; LSUIElement agent + SMAppService on macOS)
