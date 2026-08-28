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

### Still open: the refusal detector reports a false negative

The Mirror for this recording says **"This recording is incomplete — system audio is being
played but arrives as silence — grant EverTranscript permission…"**. The grant is present:
`audio-check` measured the system leg at peak 0.795 twelve minutes earlier, on this machine,
in this session.

Nothing was playing during the meeting. `SILENCE_PROVES_REFUSAL_MS` rests on the asymmetry
recorded in DECISIONS Q3 — "a global tap's callback fires only while something is *playing*,
so a machine with nothing to record delivers no frames at all". **On macOS 26.x that does not
hold**: the tap delivered frames of bit-exact zero for 44 s with nothing playing, and the
detector concluded the permission was refused. Two harms follow:

- a correct recording is permanently marked incomplete, with a confident instruction to grant
  a permission the Operator already granted;
- the leg is **ended**, not paused, so a meeting that opens with 44 s of quiet loses system
  audio for its entire remaining duration.

Q3's conclusion needs re-deriving against this machine's behaviour — the detector is sound
only if the asymmetry it rests on is real.

### Confirmed while there

- **Ticket 09's tray icon has now been seen.** It renders `○` when Idle and `●` while
  Recording, and the daemon's Quit stops the Core. It had never appeared to be missing: on a
  multi-display Mac it lands on a non-main display's menu bar. Because the binary is not
  bundled, macOS names the status item after the process PID where other items use bundle ids.
- **No TCC prompt ever appeared** — the grants attach to the *responsible process*, which for
  a terminal-launched binary is the terminal (Ghostty held both already).
  `kTCCServiceAudioCapture` holds no entry for it, so the process tap is authorised by the
  Screen & System Audio Recording grant. Whether the prompt fires for an unprivileged launch
  is therefore **still unanswered** — it never needed to fire.
