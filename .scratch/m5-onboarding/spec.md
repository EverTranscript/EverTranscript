# M5 — Onboarding, trust surfaces, and getting it onto a machine

Status: ready-for-agent

Sources of truth: `CONTEXT.md` (glossary — normative), `docs/prd.md` (stories 43–48), ADR-0007 (tool posture: the Briefing and its acknowledgment), ADR-0013 (the Backend picker forces an explicit choice), ADR-0016 as amended by ADR-0025 (signed, notarized, direct, plus winget; electron-updater), ADR-0020 as amended (Nothing Ambient, and the two reversals it has taken), ADR-0023 (Auto-Record on by default, and nothing captured before acknowledgment), ADR-0033 (open source at M2 quality), ADR-0034 (Sanctioned Traffic), ADR-0035 (the History folder), `docs/implementation-notes-2026-08-27.md` (the catalog's Client & UX section has the floating-indicator recipe and the tray state machine).

## Problem Statement

Four milestones have built something that works on this machine, driven by someone who wrote it. Nobody else can install it, and nobody who did could tell what it was about to do.

That second half is the serious one. This product records meetings by itself, stores biometric identifiers for people who never consented, and reads a calendar when granted. Every one of those is defensible — the ADRs argue each carefully — and **none of the arguments has ever been shown to an Operator.** M1 shipped a one-line `acknowledge` command as a placeholder for ADR-0007's blunt legal briefing, and it has stood in ever since. A product that starts recording on its own with a consent story nobody has read is not a tool posture; it is a claim about one.

The distribution half is smaller but blocking: ADR-0016 chose signed direct download precisely so shipping never waits on an app store's opinion of system-audio capture, and none of that pipeline exists.

## Solution

**The Briefing** becomes what ADR-0007 asked for: blunt, one-time, covering recording-consent law, voice profiling, and the fact that Auto-Record is on unless turned off — ending in an explicit acknowledgment, with nothing captured before it. The gate already exists in the Core and is already tested; what is missing is the text and the surface.

**Linear onboarding** (story 44) explains each requirement at the moment it is demanded — permissions, model download, History folder, Summary Backend, calendar — so that the Operator leaves setup armed and is never configuration-prompted mid-meeting again. Every step is skippable where the ADRs say it is skippable, and says what skipping costs.

**The trust surfaces** (stories 46, 47) make the two guarantees checkable rather than asserted: what the app holds, what it has been granted, and what it may say on the wire — enumerable, in the product, not only in a README.

**Distribution** (story 48, ADR-0016) is signed and notarized on macOS, a signed installer plus winget on Windows, with electron-updater covering the bundled Core. The update check is Sanctioned Traffic entry one and is disableable, which means the product must work with updates off.

## User Stories

Numbering follows `docs/prd.md`.

43. As a new Operator, I want a blunt one-time Briefing on recording consent law, voice profiling, and the fact that Auto-Record is on unless I turn it off, ending in an explicit acknowledgment — and nothing captured before that acknowledgment.
44. As a new Operator, I want linear setup where every requirement is explained at the moment it's demanded, so that I exit setup fully armed and am never configuration-prompted at runtime again.
45. As a new Operator, I want the Summary Backend choice made explicitly by me (Local badged "Recommended", never preselected).
46. As a privacy-conscious evaluator, I want the two guarantees stated in plain language and verifiable — the source open to read, entitlements on macOS, and a wire that speaks only enumerable Sanctioned Traffic.
47. As an Operator, I want certainty that the app never indexes my filesystem or contacts, never reads screen content, and reads my calendar only if I granted it.
48. As an Operator, I want a signed direct download (Homebrew cask on macOS, winget on Windows) with in-app updates.

Carried from earlier milestones as explicitly deferred to here: the **floating mini-indicator** (evaluated, per ADR-0026's tray decision and the catalog's recipe), and the tray's **not-ready gate** during onboarding.

## Implementation Decisions

- **The Briefing is text, and the text is the feature.** Its content is the deliverable, not the modal that shows it. It must say plainly that recording without all-party consent is a crime in many jurisdictions, that the product stores voiceprints of people who never agreed to that, that Auto-Record is on by default, and that copies of the History folder carry biometric data (ADR-0035's own stated consequence). Softening any of those to improve conversion would make this a dark pattern with an acknowledgment button.
- **The acknowledgment gate already exists and already works.** `briefing_acknowledged` blocks capture and is one-way (a Client cannot un-accept it). M5 adds the surface, not the invariant — and must not weaken it.
- **Onboarding is linear and every step states its cost.** Permissions, models, folder, Backend, calendar. Skippable steps say what is lost; the Backend step is not skippable, because ADR-0013 requires an explicit choice and "later" is a preselection by another name.
- **The trust surfaces enumerate rather than assure.** "We respect your privacy" is worthless; "here is every grant this app holds, every model on disk, every host it may contact, and the source that proves it" is checkable. This is the surface an evaluator uses to decide, and it should be uncomfortable to write anything false into.
- **The updater is Sanctioned Traffic entry one and is disableable** (ADR-0034). With it off and models downloaded, the product must produce literally zero network traffic — the guarantee test already asserts this and must keep passing.
- **The floating indicator is evaluated, not assumed.** ADR-0026 gave the tray to the Core deliberately. The catalog has the exact Electron recipe, and the question is whether a second always-on-top window earns its cost — a decision to make and record, not a feature to add because it was listed.
- **Signing and notarization need credentials this repository does not have and should not.** The pipeline is built and the human steps named; a build that cannot be signed here is not a failure to hide.

## Testing Decisions

- **Philosophy (unchanged)**: external behavior only. What an Operator sees, what the binary contains, and what reaches the wire.
- **The Briefing's gate is already covered** by the consent-gate suite; M5 must not regress it, and adds the assertion that the acknowledgment cannot be granted by a Client that never displayed the text.
- **The trust surfaces are tested against the binary, not against a document.** The permission-set audit and the no-analytics-SDK test already do this; the surfaces should read from the same facts rather than from a hand-maintained list that can drift from what the binary actually does.
- **The zero-network guarantee gains its final form**: with updates disabled and models present, no traffic at all.
- **Both platforms** (ADR-0025 as amended). Packaging is where "both platforms" stops being a build flag and becomes two toolchains, two signing stories, and two install paths.

## Out of Scope

- Post-v1 and explicitly named as such: the Mac App Store SKU (ADR-0016), Apple Foundation Models as a local Summary tier (ADR-0031), a browser extension for precise URL state (ADR-0030), prompt-on-unknown-mic-use (ADR-0024), shell hooks and the Parakeet spike (catalog post-v1 seeds).
- Monetization, which ADR-0033 defers deliberately rather than leaves undecided.
- Ratified non-goals reaffirmed: no telemetry, no app-store dependency for shipping, no filesystem indexing, no contacts, no screen content, no cloud Transcription or Diarization.

## Further Notes

- **Every milestone here has ended the same way**, and it is worth writing down before this one starts. M2 found six defects in code that compiled and had never run. M3 owed a DER and producing one found three more, including an algorithm that was not the algorithm its name claimed. M4 owed a quality number and got a bad one that confirmed a prediction. In each case the finding came from **running the real thing on real input**, and in each case the unit tests had passed.
- The M5 form of that is the one this session cannot self-serve: **an Operator who is not the author, installing from a package, on a machine that has never built this.** Everything else in this milestone can be checked from here; that cannot, and the close-out should say so plainly rather than approximate it with a local run.
- The Briefing is also the one deliverable where being wrong is not a bug but a harm. It is read once, by someone deciding whether to trust a recorder, and it is the product's only chance to say what it actually does.
