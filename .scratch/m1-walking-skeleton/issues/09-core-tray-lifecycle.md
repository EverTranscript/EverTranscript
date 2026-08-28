# 09: Core tray + lifecycle

**What to build:** The Core becomes the always-on login item the glossary describes: a UI-capable agent owning the tray (record/stop, state machine with transitional items and a not-ready gate during model downloads, explicit Quit), the registration-only launch-at-login toggle, the first-run acknowledgment gate, and the capture permission requests — nothing captured before the ack, ever.

**Blocked by:** 03.

**Status:** partly done — the tray UI itself is not built

- [ ] **Not done, and deliberately not built blind.** A tray needs the macOS main-thread event loop, which
      restructures how the daemon starts — the most load-bearing path there is — in exchange for a control
      nobody here can see. This machine has no reachable GUI session: `screencapture` fails with "could not
      create image from display", so neither the icon nor a menu click could be verified, and "it compiles"
      is not evidence that a menu bar item works. Everything it would display is already on the protocol
      (`status`, `core/stateChanged`, `models/status`, and now a Meeting's `audioNotes`), so this is
      presentation over a finished interface and is the right work for someone at a screen. No bundle is
      needed — a bare binary can take an accessory activation policy — so packaging is not the blocker.
- [~] The not-ready signal exists and is correct (`models/status` reports `ready`); nothing renders it yet.
      What a recording *lost* now renders in three places that do exist: the Mirror, `evertranscript show`,
      and the Electron client, carried on the Meeting as `audioNotes`.
- [x] Launch-at-login is a protocol method and `evertranscript autostart on|off`, registration-only (a LaunchAgent
      plist on macOS, the Run key on Windows), leaving a running Core alone. Quit-from-tray awaits the tray;
      SIGTERM stops the Core today.
- [x] The acknowledgment gates all capture, enforced in the Core so no Client can route around it; verified live
      (a fresh install refuses to record and says how to fix it). `evertranscript acknowledge` is the M1
      stand-in for the Briefing.
- [x] `evertranscript audio-check` is the preflight: it runs without the Core, listens on both legs, and reports what
      each actually produced. It deliberately *records* rather than asking the OS whether it may — on macOS the two
      answers differ, since a tap is granted whether or not audio recording is allowed and a refused one is silent
      but successful. Asking would report a working system-audio leg on a machine that will record nothing.
      Requesting permission is still implicit: capture triggers the prompt, and no explicit request API is called.
- [~] Autostart is implemented for both platforms; the tray is implemented for neither.
