# What v1 is not

Every milestone here left named gaps. A release that does not enumerate
them hands the next person a discovery instead of a list. This is that
list, assembled from the open criteria across M1–M5 and grouped by what
would actually be needed to close each.

Counts at the time of writing: **M1 65/65, M2 85/85, M3 69/71, M4 66/74,
M5 52/60.**

---

## 1. Things that need a machine or a person this session did not have

These are not unfinished work. They are work whose evidence can only come
from somewhere else, and each is already labelled that way in its ticket.

- **A clean-machine install by someone who is not the author.** The single
  most valuable untaken step in the project. Every milestone found its real
  defects by running the real thing on real input — M2 found six, M3 three,
  M4 two — and every one of those had passed its unit tests. The M5 form of
  that is an Operator installing from a package on a machine that has never
  built this, and it is the one thing the repository cannot self-serve.
- **The Briefing read by someone other than its author.** It has one job: to
  make a stranger able to say what the product does before they let it
  record. Nobody has tried.
- **Signing, notarization, and both installers.** Needs a Developer ID
  certificate, an App Store Connect key, and a Windows code-signing
  certificate. Those are the Operator's and should not live here.
- **Homebrew cask and winget manifests.** Blocked on the above and on a
  tagged release that does not exist.
- **The Summary sidecar has never run on Windows.** It cross-compiles with
  llama.cpp included, which is worth exactly what the same sentence was
  worth in M3 — nothing about runtime.
- **Counsel review of the Briefing.** The PRD makes it mandatory before v1
  and calls it per-jurisdiction work rather than translation. `AWAITING_COUNSEL`
  says so in the product.
- **Arc's audio path.** Arc needs an account to open a window; its helper
  bundle ids were read from the shipped app and the casing bug that made it
  unmatched was fixed, but whether its audio comes from a `.helper` process
  is unobserved — and that is precisely what Teams turned out to get wrong.

## 2. Things measured and found wanting

- **Summary quality is bad.** On a real recording containing two plain
  commitments, the shipped local model produced `None noted.` — zero of two
  action items. The registered 0.5B is "the model that was verified, not the
  model that should ship", and choosing the real default is the work M4's
  close-out is still owed.
- **No Summary measured on a long meeting.** The recording used is 89
  seconds, so map-reduce never engaged. Chunk-boundary behaviour on ninety
  minutes — where the M4 failure mode actually lives — is exercised only
  against the fake.
- **DER 3.9% is on a construction, not a conversation.** The second speaker
  is the first one resampled. It shows the pipeline separates two
  acoustically distinct voices; it is not a DER on a real multi-person
  meeting, and should never be quoted as one.
- **The embedding bake-off was never run.** One entrant is a preference, not
  a bake-off.

## 3. Things deliberately not built, with the reason

- **The floating indicator** (Q40). A Client-owned indicator vanishes when
  the Client is closed while the Core keeps recording, which is this
  product's normal state.
- **Ollama and LM Studio detection.** The presets exist and loopback
  classification is correct; nothing probes for a running instance.
- **Sidecar idle self-exit and lazy-reload.** Optimisations for a resident
  process, and nothing keeps one resident yet.
- **Arc and Edge live browser matrix beyond what was run.** Both were
  installed, driven and uninstalled; Arc's window needs an account.

## 4. Things that ship in a state the product itself admits to

These are live in the build and say so where an Operator can see it.

- **Cloud provider data-handling labels read `unverified`**, with a test
  keeping them honest. ADR-0010 wants a human to have read the terms at
  release time; nobody has.
- **Four Windows executable names in `WINDOWS_EXECUTABLES`** were checked
  against a shipping competitor's table rather than read off a running
  machine — and one of the two Tencent products' names was deleted on a bad
  inference before another session caught it (Q37).
- **腾讯会议's Windows executable** is confirmed; Zoom's and VooV's rows are
  believed-good and only partly observed.

## 5. Not built at all in M5

- **The Windows NSIS installer.** The build script handles the `.exe`
  suffix and the manifest generator expects the artifact; nothing produces
  it. The winget manifest is consequently generated but points at a file
  that does not exist.
- **electron-updater.** The Core-side check, its host, the version
  comparison and the switch all exist and are tested — that is the half the
  zero-network guarantee rests on. Its remaining job is downloading and
  *installing* a signed artifact, and pointed at unsigned local builds it
  would either refuse the signature or install something unsigned; neither
  outcome says whether it works. A short job the day a signed release
  exists.
- **A `.app` bundle.** The three binaries and the built renderer are staged
  side by side; wrapping them is the remaining step before anything can be
  signed or notarized.

## 6. A mistake worth keeping (Q42)

A blanket replace of `- [ ]` with `- [x]` across the M5 close-out ticked
five criteria nobody had met — and they were precisely the five this
repository structurally cannot self-serve. Caught and corrected within the
session. Recorded because of what it nearly did: had it survived, this
milestone would have claimed validation by a person who does not exist.
The narrow lesson is that an edit which ticks boxes should never be able to
tick one its author did not read.

---

## What is actually solid

Worth stating too, because a list of gaps with no counterweight is its own
kind of distortion.

The record works and is honest about itself: dual-channel capture that
survives device churn, crash-safe persistence, a Mirror that is a
regenerable projection, and an immutable transcript with an Operator
correction layer above it rather than inside it. Auto-Record works on both
platforms and every shipped Watchlist row has been observed triggering on
at least one of them. Diarization runs end to end on real audio and
recognises a returning voice. The Knob's asymmetry is structural — there is
no code path from local to cloud, and the tests assert the cloud Backend was
never *called* rather than that the right one answered. Keys live only in
the OS credential store. And with updates off and models present, the
product makes no network calls at all.
