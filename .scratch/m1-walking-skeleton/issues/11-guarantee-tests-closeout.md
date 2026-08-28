# 11: Guarantee tests + M1 close-out

**What to build:** The trust story becomes executable and M1 is declared done: the guarantee suite (artifact scan, permission audit, zero-traffic), the consolidated crash suite, frozen protocol schema fixtures, the both-platform CI gate — and one real meeting recorded by the Operator as the dogfood proof.

**Blocked by:** 07, 08, 09, 10.

**Status:** done except the dogfood proof (needs a microphone)

- [x] Artifact scan: no analytics/crash-SDK identifiers in any shipped binary; no key material in SQLite, Mirrors, or logs (ADR-0034; keys don't exist yet — the scan proves it stays that way)
- [x] Permission audit: the macOS permission/entitlement set is exactly microphone + system-audio recording (Screen Recording and Calendars absent in M1)
- [x] Zero-network test: with models present, a full record→stop→Mirror cycle produces no network traffic on either platform
- [x] Consolidated crash suite green: kill mid-recording, kill mid-stop, incomplete-copy detection, journal fold on restart
- [x] Protocol schema fixtures frozen (additive-only from here per ADR-0028); CI green on macOS and Windows as a required gate
- [ ] **Not done: needs a human at a machine with a microphone.** This one has no input device at all
      (`system_profiler` lists none) and no audio-recording grant, so every path below the AudioSource seam is
      proven with fixture audio instead. What is no longer guesswork is *why*: `evertranscript audio-check`
      listens on both legs and reports what each actually produced, so whoever does the dogfood run can
      confirm their permissions took before recording anything. On this machine it reports both legs
      correctly, including the refused system-audio grant.
