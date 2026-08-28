<!-- AI-maintained, append-only -->

## Q1 — m1/08-aec-dsp-quality — deviation

**Question:** ADR-0029 specifies DTLN-shaped echo cancellation. Should the implementation ship the DTLN models and an inference runtime, or a classic adaptive filter?

**Options considered:** DTLN ONNX models plus an inference runtime (as ratified) / normalized least-mean-squares adaptive filter with a residual suppressor / leave echo cancellation unimplemented and keep recording the ticket gap.

**Chosen:** NLMS adaptive filter (128 ms) followed by a residual echo suppressor, no models and no inference runtime.

**Decided-by:** agent

**Justification:** The usual reason to prefer a learned canceller is alignment — the reference and microphone normally arrive on different clocks with an unknown, drifting delay that has to be estimated before any filter converges. ADR-0029's other half removes that problem here: both legs are stamped on one capture clock and reach the pipeline aligned to the sample, so what is left is the case a linear adaptive filter handles well. Against that, DTLN costs an ONNX runtime, a model to download and version, and inference budget competing with transcription on the same machine. Measured on synthetic echo fixtures: echo-only input is driven to silence, clean audio is preserved at 100.0% of its level, and 115% of near-end power survives double talk. End to end with the real engine, an uncancelled speakerphone reproduces the far end on the microphone channel faithfully (WER 0.08 against the far-end transcript) while the cancelled one does not (WER 0.86). Revisit if real speakerphone recordings show the linear filter failing on nonlinear speaker distortion, which is the case DTLN is genuinely better at and which synthetic fixtures cannot exhibit.

**Outcome:** applied

**Ref:** (pending)

## Q2 — m1/08-aec-dsp-quality — tradeoff

**Question:** A linear filter alone still leaves an echo that a transcription model decodes perfectly well. Accept partial reduction, or suppress the residual and risk attenuating genuine near-end sound?

**Options considered:** linear cancellation only, accepting that a quiet echo still reaches the record / add a residual suppressor gated on echo dominance / suppress whenever the far end is playing (half-duplex).

**Chosen:** Suppress the residual, but only while the filter is demonstrably explaining most of the microphone's energy.

**Decided-by:** agent

**Justification:** The record does not care how many decibels down an echo is — an intelligible echo is transcribed and attributed to the Operator either way, which is the harm ADR-0029 names. Gating on echo dominance rather than on far-end activity is what keeps it from being half-duplex: the Operator's voice is not predictable from the reference, so during double talk the residual stays large and no suppression happens. The cost is that a quiet non-echo sound picked up while the far end is playing may be attenuated with it — the standard tradeoff for this stage, and preferable to crediting people with words they did not say.

**Outcome:** applied

**Ref:** (pending)

## Q3 — m1/03-capture-vertical — gate-resolution

**Question:** macOS grants a process tap whether or not the Operator has allowed audio recording, then delivers digital silence forever when they have not. With no API distinguishing the two, how should a refused permission be reported?

**Options considered:** trust the tap-creation return code / query TCC directly / detect the silence at runtime and report it.

**Chosen:** Detect it during capture: frames arriving steadily while no sample has ever been non-zero.

**Decided-by:** agent

**Justification:** There is no public API for the audio-capture permission's state, and the tap's success proves nothing. What makes detection sound rather than a guess is an asymmetry confirmed on this machine: a global tap's callback fires only while something is playing, so a machine with nothing to record delivers no frames at all. Frames arriving while every sample is bit-exact zero therefore means audio is being played and we are being handed zeros — not that the room is quiet. One real non-zero sample ends the check for the session. Verified against the actual failure: this machine has no grant, and the detector fires with an actionable message after 15 seconds of played-but-silent audio.

**Outcome:** applied

**Ref:** (pending)

## Q4 — m1/03-capture-vertical — deviation

**Question:** A recording that lost a capture leg recorded that fact only to a log line. Should the loss reach the record itself, which means a schema migration and a protocol field neither ticket asked for?

**Options considered:** leave it in the log and let the Operator infer / add a note to the Mirror only / carry it on the Meeting through store, protocol, Mirror, CLI and client.

**Chosen:** Carry it on the Meeting: a new `audio_notes` column (migration 4), an additive `audioNotes` protocol field, and rendering in the Mirror, `show`, and the Electron client.

**Decided-by:** agent

**Justification:** Not asked for by tickets 03 or 09, but it is what makes the rest of their work legible. A Meeting recorded with one leg missing produced a one-sided transcript and nothing anywhere to explain it — indistinguishable, to the person reading their notes a week later, from a meeting where nobody else spoke. On any machine without the system-audio permission that is every meeting, so the gap is not an edge case. The protocol change is additive, which ADR-0028 permits. Verified end to end against a real daemon on this machine, which has neither leg available: both reasons reach `show` and the Mirror.

**Outcome:** applied

**Ref:** (pending)

## Q5 — m1/09-core-tray-lifecycle — deviation

**Question:** Ticket 09 asks the tray for "a not-ready gate during model downloads". Should the tray refuse to record while a transcription model is missing?

**Options considered:** block recording until models are ready, as the word "gate" suggests / show the not-ready state but still allow recording / say nothing about models at all.

**Chosen:** Show it, do not block. `NotReady` is a visible state whose menu item remains clickable, and its status line says the recording will have no captions.

**Decided-by:** agent

**Justification:** The first draft blocked, and the tests caught it on a machine with no models downloaded. Blocking contradicts the Core: `start_meeting` deliberately does not require a model, because ADR-0019 holds that a Meeting recorded without captions still beats no Meeting. A tray that refused would lose the meeting to save the transcript, which is backwards, and would make the menu bar stricter than the interface it is a face for. The gate is therefore informational — the Operator learns what they will not get, and still gets the recording.

**Outcome:** applied

**Ref:** (pending)

## Q6 — m1/11-guarantee-tests-closeout — deviation

**Question:** The workspace comment claims "Metal on Apple Silicon", but `whisper-rs` is pinned with `default-features = false` and that crate's `default` feature set is empty, so Metal was never compiled in. Leave the CPU build, or enable the GPU the comment already promised?

**Options considered:** leave it on CPU and correct the comment / enable `metal` for every target that has it / enable it only where the target can be tested, and say so.

**Chosen:** Enable the `metal` feature from `evertranscript-core` under `cfg(all(target_os = "macos", target_arch = "aarch64"))`, and rewrite the workspace comment to describe what the build actually does.

**Decided-by:** agent

**Justification:** Not a preference — a correctness threshold. On CPU this machine decoded a padded 30 s window in ~10 s, so transcription ran about 3.5x slower than real time; because decoding then blocked the capture drain (Q7), the shortfall was paid in dropped capture frames rather than in late captions. Metal takes the same window to ~2.4 s, which is what puts the pipeline under real time and stops the loss. The feature is scoped to the target it was measured on rather than to `macos` generally, because Intel Macs were not tested here and the workspace ships Windows too. Verified: `english_speech_transcribes_with_a_reported_error_rate` reports 0.0% WER with the Metal build, so the speedup costs no accuracy.

**Outcome:** applied

**Ref:** (pending)

## Q7 — m1/11-guarantee-tests-closeout — deviation

**Question:** Transcription ran synchronously inside the loop that drains the capture channel. Leave it inline and accept that a slow decode stalls capture, or move it off the loop — a change neither ticket asked for?

**Options considered:** leave it inline and rely on the model being fast enough / enlarge the capture channel so bursts fit / run transcription on its own thread and let the queue, not the recording, absorb the pressure.

**Chosen:** Transcription runs on a dedicated thread fed by a bounded queue; the capture loop hands blocks over and never waits. Blocks that will not fit are counted and reported as a degraded note.

**Decided-by:** agent

**Justification:** The recorder already stated this contract in a comment — "Audio to disk first: the recording must survive even if transcription is slow or broken" — but did not keep it: `pipeline.push` blocked the same task that drains the 256-slot capture channel, and both capture callbacks `try_send` and drop frames when it fills. Measured before the change: 8.7 s of speech replaced by digital silence in the finished m4a, with the Mirror reporting nothing, because `audio_notes` tracks a leg *ending* and not frames lost mid-leg. Enlarging the channel only moves the cliff, since the deficit is per-decode and cumulative. Making the ordering structural is what makes ADR-0019's priority true rather than merely intended. Verified after: the same script records with the system leg continuous and no gaps.

**Outcome:** applied

**Ref:** (pending)

## Q8 — m1/11-guarantee-tests-closeout — tradeoff

**Question:** `ThreadStream::stop` joins the microphone thread unconditionally, but that thread can be stuck inside `AudioOutputUnitStart` — before the loop that reads the stop flag. Wait for a thread that may never return, or abandon it?

**Options considered:** keep the unbounded join / wait a bounded time and then abandon the thread / restructure capture start so the flag is checked before `play()`.

**Chosen:** Wait up to 5 s for the thread to signal it has finished, then abandon it and let the Meeting finalize.

**Decided-by:** human

**Justification:** Observed twice on this machine: the capture thread parked in `AudioOutputUnitStart` — plugging in AirPods changes the default input under a starting stream — and the join then never returned. The cost is not one lost recording but the whole Core: `record stop` never completed, `status` stopped answering, and only SIGKILL recovered it. Abandoning the thread risks a stream that lingers briefly after stop, which is bounded and invisible, against a hang that is neither. Five seconds is far beyond healthy teardown, so a normal stop is unaffected. Regression test: `a_capture_thread_that_never_notices_the_flag_does_not_hang_the_stop`.

**Outcome:** applied

**Ref:** (pending)

## Q9 — m1/11-guarantee-tests-closeout — gate-resolution

**Question:** Q3 concluded that a refused system-audio permission can be recognised from delivered-but-silent frames, on the premise that a global tap only fires while something is playing. The dogfood run falsified that premise. How should a refusal be recognised now?

**Options considered:** keep counting silence and raise the threshold / drop the check and let a refused tap record silence unexplained / ask the system whether anything is actually playing, and count silence only against that.

**Chosen:** Ask. `kAudioHardwarePropertyProcessObjectList` and `kAudioProcessPropertyIsRunningOutput` answer "is anything playing right now", and `silent_ms` accumulates only while the answer is yes. The refusal note also stops ending the leg: a new `CaptureEvent::Degraded` records the reason and leaves capture attached.

**Decided-by:** agent

**Justification:** Q3's asymmetry was the right idea resting on an assumption nobody measured. On macOS 26 the tap delivers zero-filled frames continuously with nothing playing, so meeting `01a047ff` — a correct recording on a machine whose grant `audio-check` had measured at peak 0.795 twelve minutes earlier — was stamped incomplete and told the Operator to grant a permission they already had. Raising the threshold only moves the accusation later; dropping the check restores the silent failure Q3 existed to end. The signal Q3 assumed the tap provided implicitly turns out to exist as an API, so the fix is to stop inferring it. Measured on this machine before being relied on, which is the step Q3 skipped: false with nothing playing, true while a process plays, and still false while our own tap is capturing — so the recorder does not mistake itself for playback and no self-exclusion is needed. Verified end to end: 60 s of quiet now produces no note where the same scenario produced one before, and a played sentence is still captured and attributed. Not ending the leg is the second half and matters independently of the gate: this is a diagnosis the Core infers rather than reads, so when it is wrong it should cost a sentence in the record, not every remaining minute of the far end. The one path this machine cannot exercise is a genuine refusal, because the grant is present; that half rests on unit tests and is recorded in ticket 11 as untested end to end.

**Outcome:** applied

**Supersedes:** Q3 — its stated asymmetry does not hold on macOS 26; the conclusion it drew is kept, the evidence it drew it from is replaced.

**Ref:** (pending)

## Q10 — m1/11-guarantee-tests-closeout — gate-resolution

**Question:** Q9 replaced the refused-permission check but could not exercise the refusal itself, because the machine held the grant, and recorded that half as resting on unit tests alone. Does the check still fire when a permission is genuinely denied?

**Options considered:** leave it on unit tests and say so / package an `.app` and test a fresh identity / revoke the grant on this machine and run it.

**Chosen:** Revoke and run it. Verified on 2026-08-28: the check fires, and the leg is noted rather than ended.

**Decided-by:** agent

**Justification:** Q9's caveat was the honest position at the time and a poor one to leave standing, because a refusal that never fires is the failure the check exists to prevent and would look exactly like success. The Operator cleared the stored decision with `tccutil reset AudioCapture`, answered the prompt with Don't allow, and a 34 s meeting was recorded with audio playing throughout. The note appeared and named the permission; `audio-check` chose the played-but-silent wording rather than the quiet-meeting one; the log showed `capture leg is degraded; it stays attached` with **no `EndLeg` at all**; and the audio came out full length with the microphone at -8.1 dB and the system channel at -91.0 dB, which is the refused tap. Checked first, because it is the one way this fix could have failed silently: `anything_is_playing` keeps working with the grant denied, so the gate cannot go quiet and take the detector with it. Q9's decision is unchanged — only the caveat in its justification is closed, and it is left standing there as written.

The same run settled two things the ticket records in full. The TCC prompt does fire for a plain unbundled binary, so the `.app` carrying `NSAudioCaptureUsageDescription` that the M1 handoff named as the next thing to try is not needed; and either grant authorises the process tap, screen capture or audio capture, which corrects an earlier reading of the same evidence. One consequence of a refusal is worth knowing before it is met in a real meeting: with the system leg silent the echo canceller has no reference, so the far end arrives through the microphone and is attributed to the Operator — the harm Q1 and Q2 exist to prevent, unavoidable once the reference is gone, and the real reason the incomplete note matters.

**Outcome:** applied

**Ref:** (pending)

## Q11 — m1/11-guarantee-tests-closeout — deviation

**Question:** The dogfood run measured Mandarin coming back in Traditional characters when the speaker had read Simplified. Transcription stays on automatic language detection for code-switching, so the script is whatever the model's training data favoured. Should the record be left as decoded, or written in a script the Operator chooses?

**Options considered:** leave the model's output untouched / pin the language to `zh` and hope the script follows / seed whisper's initial prompt with Simplified text / normalise the script after decoding, with the choice as a setting.

**Chosen:** Normalise after decoding. `Settings::chinese_script` ships Simplified and can be set to Traditional; conversion runs inside `filters::clean`, using `hanconv` (MIT, no dependencies of its own).

**Decided-by:** human

**Justification:** Pinning the language would break story 7 — meetings code-switch, and `Language::Auto` is deliberate. Seeding the prompt only biases the decoder, and this session has already paid for shipping a probabilistic assumption as though it were a guarantee (Q9); it also collides with the rolling `previous_text` prompt. Converting is deterministic and verifiable, and it is orthography rather than translation: the words are identical in either script, which is why this does not offend the immutability the record depends on. Conversion is by phrase and not by character, which is what makes the ambiguous direction safe — Simplified 发 is 發 in 发送 and 髮 in 头发, and a per-character table would have to guess; both are covered by tests. Simplified is the default because more people read it, and it is a setting because that is a preference and not a fact about the speaker. Placing it before the invention filters rather than after fixed a second bug for free: `KNOWN_INVENTIONS` lists its Chinese boilerplate in Simplified, so a Traditional decode of the same subtitle spam used to walk straight past it. Measured end to end after the change: a spoken Simplified sentence is recorded at CER 0.0% against its reference, where the same sentence previously came back Traditional. The protocol change is additive, which ADR-0028 permits, and was checked to be so before the fixtures were regenerated.

**Outcome:** applied

**Ref:** (pending)
