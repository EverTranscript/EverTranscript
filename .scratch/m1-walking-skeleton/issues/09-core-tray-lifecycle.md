# 09: Core tray + lifecycle

**What to build:** The Core becomes the always-on login item the glossary describes: a UI-capable agent owning the tray (record/stop, state machine with transitional items and a not-ready gate during model downloads, explicit Quit), the registration-only launch-at-login toggle, the first-run acknowledgment gate, and the capture permission requests — nothing captured before the ack, ever.

**Blocked by:** 03.

**Status:** done — the icon has been seen: `○` idle, `●` recording (2026-08-28)

- [x] **Built.** An `NSStatusItem` with an accessory activation policy, so no app bundle is needed and no Dock
      icon appears — which matters because the Core is installed as a LaunchAgent running the binary directly
      (ADR-0026). The daemon now hands the main thread to AppKit and runs the Core on the runtime's threads.
      Record/stop, transitional `Starting…`/`Stopping…` items that are deliberately unclickable, a status line,
      and an explicit Quit that stops the Core rather than hiding the icon.

      **A correction to what this ticket said before:** the claim that there was "no reachable GUI session" was
      wrong. `CGSessionCopyCurrentDictionary` returns a session here; `screencapture` fails on a *permission*,
      which is a different thing. The tray runs on this machine and was verified doing so.

      **Verified:** the state machine and every click path, driven against a real Core in `tray_control.rs` —
      clicking starts a real Meeting, clicking again stops it, a Meeting started from the CLI or the client
      shows up, a refused start restores the previous state with the reason, and Quit cancels the Core's
      shutdown token. Live: the daemon logs the item up, serves normally with it running, and exits cleanly on
      SIGTERM. **Not verified:** that the icon is visible and that a human's click lands. That needs eyes.

      The state machine lives in `tray/mod.rs` with tests; `tray/macos.rs` only draws. A GUI session is checked
      before AppKit is touched, and `EVERTRANSCRIPT_NO_TRAY` turns it off — a Core with no tray serves exactly
      as it did before, which the guarantee suite now exercises on every run.
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
- [x] Autostart is implemented for both platforms; the tray is implemented for macOS. Windows keeps the
      headless path, which is a real path rather than a stub — the fallback is what runs on any machine
      without a menu bar.
