# 11: The Operator is identified by three rules, and there is only ever one

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** "You", aligned with how both reference products identify the same voice, and
a uniqueness defect that is reachable today.

Three rules in order (ADR-0029 as amended):

1. **Isolated mic.** Headphones the only playing output and the microphone not swapped makes
   every mic-channel cluster of that Meeting the Operator, confirmed without any act. The
   capture layer records the fact per Meeting from the output transport; the echo canceller's
   idle signal is the fallback if the Windows probe proves unreliable.
2. **Dominance.** 80% of mic time, the existing margin, and at least **20 seconds** of that
   voice. The floor is new, and it is what stops a ten-second solo test enrolling anyone.
3. **Voiceprint match**, only once the Meeting holds 30 seconds of diarized speech and at least
   two speakers. Below that gate the Operator's Voiceprint is **withheld from the whole
   resolve**, not merely from the flag — the general resolve carries it among all seeds and
   would otherwise match anyway, which would make the gate decorative.

**One flagged row, forever.** The flag has no uniqueness constraint, the lookup takes the first
row it finds, and the diarize path sets the flag without clearing any other. Delete the
Operator's Voiceprint, re-run one Meeting, and the flag lands on a freshly minted row while
the lookup still returns the old one: a second "You". A bootstrap now re-attaches to the
existing row.

**Blocked by:** 03.

**Status:** done

- [x] An isolated-mic Meeting identifies the Operator with no act, and the fact is recorded per Meeting at capture time on both platforms
- [x] A shared room still refuses to call another mic-channel voice the Operator
- [x] A ten-second solo recording enrolls nobody
- [x] A Meeting under the speech or speaker gate does not identify the Operator by voice, and that Voiceprint is absent from the whole resolve — asserted by the other Speakers' attributions, not only by the flag
- [x] Deleting the Operator's Voiceprint and re-running produces no second flagged row, and the schema makes a second one impossible rather than unlikely
- [x] The existing operator tests pass or are re-expressed against the new rules

## What changed

`identify` takes three arguments and returns `Identified` rather than `Option<Cluster>`: the
rule that fired is part of the answer, because the three differ in how far they are trusted
and in how many voices they can name. Rule 1 alone names more than one.

The order is 1 → 2 → 3, and dominance comes before the Voiceprint deliberately: it needs no
prior and cannot be misled by one. `DOMINANCE` went 0.75 → 0.80 and gained `MIN_OPERATOR_MS`
= 20 s, which is what stops a microphone test enrolling a permanent "You". `match_gate_met`
is public because the caller has to know before it assembles the seeds — `persist` gained a
`withheld` parameter, so a Meeting under the gate keeps the Operator's Voiceprint out of the
**whole** resolve rather than out of the flag alone.

The fact behind rule 1 is gathered at capture time. `output_is_headphones()` probes the
default output device — CoreAudio transport type plus data source on macOS, WASAPI
`PKEY_AudioEndpoint_FormFactor` on Windows — and `MicIsolation` accumulates it across the
Meeting alongside `CaptureEvent::DeviceChanged` on the mic leg. One glance at the room, or
one microphone swap, ends the claim for the whole Meeting. The verdict lands on
`meetings.mic_isolated` at stop.

Migration 14 carries both halves: the new column, and a unique partial index that makes a
second flagged Speaker a constraint violation. Any History that already has two is reduced to
one first, keeping the row with a Voiceprint because that is the one recognition has been
using. `attach_operator` re-attaches to the existing row and folds this run's voices into it,
which is what closes the "delete the Voiceprint and re-run" path that minted the second one.

## Measured

- macOS: 541 lib tests pass; the one failure is the pre-existing microphone-detector test
  that needs a TCC grant this machine does not have.
- Windows: cross-compiling locally is blocked by `ring` needing an MSVC C toolchain, so the
  WASAPI probe was compiled on windows-zx8 from a copy of the tree under `%TEMP%` with its
  own `CARGO_TARGET_DIR`, leaving the host's repo untouched. `Win32_UI_Shell_PropertiesSystem`
  had to be added to the workspace's `windows` features for `IMMDevice::OpenPropertyStore`.

## Still owed

Neither probe has been run against real hardware — both are compiled and neither has been
watched classify an actual pair of headphones. The classification tests drive `MicIsolation`
with readings rather than devices, on purpose, but that means the mapping from transport type
to "headphones" is argued rather than observed. The Bluetooth-speaker false positive
(DECISIONS Q127) is the case to watch, and the echo canceller's idle signal is the fallback
the ticket already names if it proves real.
