# 11: Guarantee tests + M1 close-out

**What to build:** The trust story becomes executable and M1 is declared done: the guarantee suite (artifact scan, permission audit, zero-traffic), the consolidated crash suite, frozen protocol schema fixtures, the both-platform CI gate — and one real meeting recorded by the Operator as the dogfood proof.

**Blocked by:** 07, 08, 09, 10.

**Status:** done — the dogfood proof was recorded on a MacBook Air (M4) on 2026-08-28.

- [x] Artifact scan: no analytics/crash-SDK identifiers in any shipped binary; no key material in SQLite, Mirrors, or logs (ADR-0034; keys don't exist yet — the scan proves it stays that way)
- [x] Permission audit: the macOS permission/entitlement set is exactly microphone + system-audio recording (Screen Recording and Calendars absent in M1)
- [x] Zero-network test: with models present, a full record→stop→Mirror cycle produces no network traffic on either platform
- [x] Consolidated crash suite green: kill mid-recording, kill mid-stop, incomplete-copy detection, journal fold on restart
- [x] Protocol schema fixtures frozen (additive-only from here per ADR-0028); CI green on macOS and Windows as a required gate
- [x] **Dogfood proof: one real meeting, recorded on a MacBook Air (M4) by the Operator, 2026-08-28.**
      Recording `01a047ff`, 1m 29s, a human reading a fixed English and Chinese reference aloud.

## What the dogfood run found

Every path below the `AudioSource` seam had only ever met fixture audio. Meeting a real
microphone broke three of them at once, and none of it was visible from the test suite —
fixtures arrive as whole files in large blocks, live capture arrives as CoreAudio callbacks.

1. **The chunker discarded every live sample.** `Chunker::push` judged only whole 480-sample
   frames *within one call* and kept no remainder across calls; a resampled callback is ~160
   samples, so every sample was dropped, every call. No recording on any machine could ever
   have produced a transcript. Regression test:
   `speech_chunks_the_same_when_delivered_in_small_blocks`.
2. **Metal was never enabled** despite the workspace comment (DECISIONS Q6). CPU decode ran
   ~3.5x slower than real time.
3. **Transcription blocked the capture drain** (DECISIONS Q7), so the shortfall was paid in
   dropped frames: 8.7 s of speech became digital silence *in the m4a itself*, and the Mirror
   said nothing, because `audio_notes` tracks a leg ending, not frames lost mid-leg.
4. **`record stop` could deadlock the Core** (DECISIONS Q8) — the capture thread parks in
   `AudioOutputUnitStart` when the default input changes under it, and `stop` joined it
   forever. `status` stopped answering; only SIGKILL recovered.

### Measured quality — real voice, large-v3-turbo

| | reference | result |
|---|---|---|
| English | 40 words | **WER 2.5%** — one error, `defer` heard as `deferred` |
| Chinese, sentence 2 | 21 chars | **CER 0.0%** after Traditional→Simplified normalisation |
| Chinese, sentence 1 | 15 chars | **CER 100%** — decoded as English: "We'll discuss the third week's forecast" |
| Chinese, overall | 36 chars | **CER 41.7%** |

The PRD's ASR-quality risk is **not** retired, but it is now diagnosed rather than guessed.
The Chinese failure is not acoustic: the sentence the model transcribed in Chinese was
character-perfect. It is (a) **language detection** — `previous_text` feeds the previous
chunk's English as whisper's rolling prompt, and the first chunk after a language switch
follows that bias, and (b) **script** — Simplified input is returned as Traditional. Both are
addressable in the prompt/decode path; neither is a model-capacity problem.

### Resolved: the refusal detector reported a false negative

The Mirror for `01a047ff` said **"This recording is incomplete — system audio is being
played but arrives as silence — grant EverTranscript permission…"**. The grant was
present: `audio-check` measured the system leg at peak 0.795 twelve minutes earlier, on
this machine, in this session.

Nothing was playing during the meeting. `SILENCE_PROVES_REFUSAL_MS` rested on the
asymmetry recorded in DECISIONS Q3 — "a global tap's callback fires only while something
is *playing*, so a machine with nothing to record delivers no frames at all". **On macOS
26 that does not hold**: the tap delivered frames of bit-exact zero for 44 s with
nothing playing.

Fixed in DECISIONS Q9, which supersedes Q3. The check now asks the system whether
anything is playing (`kAudioHardwarePropertyProcessObjectList` plus
`kAudioProcessPropertyIsRunningOutput`) and counts silence only against playback, and
the note no longer ends the leg — a diagnosis the Core infers should cost a sentence in
the record, not every remaining minute of the far end.

Verified, both directions, end to end:

- **The false positive is gone.** 60 s of quiet — the scenario that produced the note,
  and longer than the 44 s that triggered it — produces no note at all.
- **A working leg is unaffected.** A played sentence is still captured and attributed to
  **Participants**.
- **A genuine refusal is still caught.** With the grant actually denied and audio
  playing, the note appears and names the permission, `audio-check` reports "captured,
  but all of it silent" rather than the quiet-meeting wording, and the log shows
  `capture leg is degraded; it stays attached` with **no `EndLeg` at all**. The audio
  file is full length with the microphone intact at −8.1 dB and the system channel at
  −91.0 dB, which is the refused tap.

One consequence of a refusal worth recording, because it is not obvious: with the system
leg silent, the echo canceller's reference is digital silence, so it cannot remove the
speaker bleed and the far end is transcribed and attributed to **You**. That is the harm
Q1 and Q2 exist to prevent, and it is unavoidable once the far-end reference is gone —
the incomplete note is what tells the reader the attribution cannot be trusted.

### Confirmed while there

- **Ticket 09's tray icon has now been seen.** It renders `○` when Idle and `●` while
  Recording, and the daemon's Quit stops the Core. It had never appeared to be missing: on a
  multi-display Mac it lands on a non-main display's menu bar. Because the binary is not
  bundled, macOS names the status item after the process PID where other items use bundle ids.
- **The TCC prompt does fire, and no app bundle is needed for it.** This was the
  handoff's main open risk, with packaging a minimal `.app` carrying
  `NSAudioCaptureUsageDescription` named as the next thing to try. It is not necessary: a
  plain terminal-launched binary, unbundled, prompted as soon as the cached
  authorisation was cleared. No prompt appeared during the dogfood run only because the
  responsible process — the terminal, not EverTranscript — had already been granted.
- **Either grant authorises the process tap.** `kTCCServiceScreenCapture` satisfied it
  while no `kTCCServiceAudioCapture` entry existed; with screen capture revoked and
  audio capture allowed, the tap kept working; with both denied, it delivers silence.
  The grant attaches to the responsible process throughout, so a binary run from a
  terminal is authorised as that terminal.
