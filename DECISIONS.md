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

## Q12 — m1/11-guarantee-tests-closeout — gate-resolution

**Question:** Q11 records `hanconv` as having "no dependencies of its own". A review of the diff found that untrue. Does the dependency still hold, now that its actual surface is known?

**Options considered:** keep `hanconv` and state its real dependency surface / hand-write a conversion table to avoid the dependency entirely / drop the feature.

**Chosen:** Keep it, and correct the record. `hanconv` pulls `ahash`, and under that `cfg-if`, `getrandom`, `libc`, `once_cell`, `zerocopy` and a `version_check` build dependency.

**Decided-by:** agent

**Justification:** The claim came from reading `cargo tree --depth 1`, which shows only the first level and hid everything below it — a measurement that answered a narrower question than the one being asked, which is the same mistake in miniature that Q3 made and Q9 corrected. The dependency still holds on its merits: what this product promises is that nothing leaves the machine (ADR-0002), and the added crates are hashing, RNG seeding and platform glue, with no HTTP, TLS or socket crate among them — the guarantee suite's zero-network test continues to pass. A hand-written table was the alternative and is worse: the conversion has to be phrase-aware to be correct at all, and a partial table improvised here is exactly the kind of untested claim this journal keeps recording the cost of. Q11's decision stands; only its parenthesis was wrong, and it stays as written because the journal is append-only.

**Outcome:** applied

**Ref:** (pending)

## Q13 — m1/11-guarantee-tests-closeout — deviation

**Question:** The dogfood run measured the first sentence after a language switch decoded into the wrong language, at CER 100%. The rolling prompt was the suspect, but whisper's language detection runs on the audio and should not be affected by it. What is actually happening, and what should be done about it?

**Options considered:** drop the rolling prompt entirely / detect the language acoustically before every decode and prompt only in the matching language / decode with the prompt and ask again without it when the engine reports a different language than the prompt was in / leave it and record the finding.

**Chosen:** Decode with the prompt, and decode again without it when the language the engine reports disagrees with the language the prompt was in. The prompt also becomes per-channel.

**Decided-by:** agent

**Justification:** Measured before choosing, against the real model, because the mechanism was genuinely unknown. The same Mandarin audio decodes as `我们今天开会讨论第三季度的预算...` with no prompt and as "We will discuss the third year's plan..." with an English one — so the prompt is the cause beyond doubt. The useful part is what does *not* break: the engine reported `zh` in both cases. Detection is acoustic and stays correct while the words do not, which turns an invisible corruption into a disagreement the pipeline can see, with no extra work to find it. That ruled out pre-detection on cost: `pcm_to_mel` plus `lang_detect` measured 1173 ms against a 2469 ms decode, about 47% added to every chunk, where asking again costs one decode only at a switch — and switches are rare. Dropping the prompt entirely would have fixed the bias by discarding the thing it was for, which is a name or piece of jargon keeping its spelling across a meeting; that benefit is real within a language run and is kept. Verified end to end: the sentence that measured CER 100% now measures 0.0%, the retry fires exactly once for the one switch in the recording, and the log shows `prompted_in="en" heard="zh"`.

Separately and with no measurement needed, `previous_text` was a single field used for both capture legs, so the Operator's words steered the far end's decode and the far end's steered theirs. The two legs are different people — that separation is the whole attribution model in M1 — so each now keeps its own.

**Outcome:** applied

**Ref:** (pending)

## Q14 — m1/11-guarantee-tests-closeout — gate-resolution

**Question:** Decodes consisting of a single `.` were being stored as speech. `is_meaningless` judges only text longer than ten characters, so nothing caught them. What is the right test?

**Options considered:** lower the length threshold / reject anything shorter than some minimum / reject text with no linguistic content at all, whatever its length.

**Chosen:** Reject text containing no alphanumeric character. Length is not consulted.

**Decided-by:** agent

**Justification:** A length rule cannot express this without doing damage: "Yes", "好" and "да" are whole turns in a meeting, and any threshold high enough to catch "." discards them. Content is the property actually being tested, and `char::is_alphanumeric` draws the line where it belongs — true for Han, kana, hangul, Cyrillic and digits, false for punctuation in both Latin and CJK, which was checked against each of those cases rather than assumed. The record is immutable (ADR-0009), so a stored "." is permanent and uncorrectable, which is what makes a filter that only judges long text the wrong shape for the problem.

**Outcome:** applied

**Ref:** (pending)

## Q15 — brand-identity/02-concepts-and-pick — gate-resolution

**Question:** Which of the three candidate marks does EverTranscript ship?
**Options considered:** A voice-line (three transcript lines, the first a wave) / B letterform e (a monoline e whose crossbar runs out as a line) / C loop-into-line (an open ring exiting into a line)
**Chosen:** B — the letterform e.
**Decided-by:** human
**Justification:** The Operator picked B from the rendered contact sheet after all three were mocked into the Dock, both menu bars, and a browser tab (`brand/explorations/`, review page linked in the ticket). B was also the recommendation: the only candidate that stays itself at 18 pt — A collapses to three bars and borrows the ≡ menu glyph's meaning, C reads as the letter Q at every size rendered.
**Outcome:** applied
**Ref:** (pending)

## Q16 — brand-identity/01-asset-pipeline — tradeoff

**Question:** Are the generated icons committed, or rebuilt by CI from the masters?
**Options considered:** commit `brand/generated/`, the Electron copies, and the tray TIFFs / rebuild everything in CI on each run / ship only masters and let each consumer render its own
**Chosen:** Commit the generated binaries; `brand/render.mjs` stays the only way they change.
**Decided-by:** agent
**Justification:** The tray embeds its glyphs with `include_bytes!`, so the files must exist at `cargo build` time on every machine and CI leg — a render step before every Rust build would put Node in the Rust toolchain's path (both reference repos, anarlog and Meetily, commit their Tauri icon sets for the same reason). The cost, binary diffs, is bounded: the render is deterministic (vector-rasterized per size, no system fonts, byte-compare before write; re-running produces no diff), so a diff can only mean the masters or the script changed.
**Outcome:** applied
**Ref:** (pending)

## Q17 — brand-identity/04-tray-and-electron-integration — deviation

**Question:** The tray's always-visible indicator was the text "●"/"○". Replace it with what?
**Options considered:** keep the Unicode dots / an SF Symbol (zero assets, but a stock drawing) / the mark itself as a template NSImage per state
**Chosen:** The mark as a template image, in four states: Ready (mark), Recording (mark + solid dot), Busy (mark dimmed), Attention (mark + hollow dot, for NotReady and NotPermitted). `TrayView.indicator` became the `TrayIndicator` enum; the drawings are 18 pt multi-representation TIFFs embedded in the binary, since the Core has no bundle to load from.
**Decided-by:** agent
**Justification:** Ticket 09 left the dots as a placeholder, and both studied competitors identify themselves in the menu bar with their mark as a template (their tray is also a state indicator — the model followed here). Attention is a new visible state the dots could not express: NotReady/NotPermitted previously looked identical to Idle until the menu was opened. The enum keeps the artwork out of the tested state machine; a decode test pins that every embedded TIFF is a 2-rep 18 pt template, and the Windows CI leg is unaffected because the bytes live inside the cfg(macos) module (`cargo tree --target x86_64-pc-windows-msvc` shows no objc2).
**Outcome:** applied
**Ref:** (pending)

## Q18 — brand-identity/03-final-mark-and-outputs — gate-resolution

**Question:** What palette and construction does the identity commit to?
**Options considered:** petrol teal tile + paper glyph / charcoal ink tile (crowded: several dev tools) / coral-red tile (reads as an alert/recorder)
**Chosen:** Petrol teal (#158580→#094F4C tile) with a paper (#F5F1E8) monoline glyph; ink #1F1D1B on light grounds; #E5484D reserved for the recording accent in UI and banned from the icon. Wordmark in Geist SemiBold (OFL), always shipped as outlines.
**Decided-by:** human
**Justification:** The Operator chose the teal direction from the option set before the concepts were drawn. Constraint honoured throughout: the mark may suggest nothing the guarantees forbid (ADR-0001/0020/0034 — no clouds, sync arrows, globes, padlocks, sparkles), and it must not collide with the three neighbours studied (Granola chartreuse, Anarlog cream, Meetily purple) or the Zoom/Teams blues beside it in a Dock.
**Outcome:** applied
**Ref:** (pending)

## Q19 — brand-identity/reference-logos — deviation

**Question:** The extracted Granola/Anarlog/Meetily logos were asked into `brand/` — committed like the rest of that directory, or kept out of git?
**Options considered:** commit them like every other brand asset / save them untracked with provenance and re-extraction paths / refuse to store them in the repo tree at all
**Chosen:** Saved to `brand/reference/` but gitignored, with a README recording exact source paths and versions so re-extraction is one command.
**Decided-by:** agent
**Justification:** They are other companies' copyrighted artwork and trademarks, and ADR-0033 makes this tree public at M2 — committing them would make the repo redistribute marks that are not ours (a different thing from *naming* the products, which `docs/competitive-facts-*.md` already does). Untracked-with-provenance keeps the requested local convenience and loses nothing that cannot be regenerated from the named paths. Easy to override: `git add -f brand/reference` if the Operator wants them tracked.
**Outcome:** assumed
**Ref:** (pending)

## Q20 — m2-auto-record/09-m2-closeout — finding

**Question:** Teams was installed, its bundle id verified against the real app, and every Watchlist row was proven through the Core — so was a live Teams run worth the Operator's sign-in, or was it confirmation of something already known?
**Options considered:** close the row on the verified bundle id and the Core-level proof / ask the Operator to sign in and drive a real Teams call
**Chosen:** Ran it live. Teams held the microphone and Auto-Record did nothing: the recording process is `com.microsoft.teams2.modulehost`, which has no `.helper` in it, so `responsible_app` passed it through unchanged and the shipped `com.microsoft.teams2` row never matched. Mapped it in `HELPER_EXCEPTIONS`; the same call then triggered in ~6 s and auto-stopped at ~45 s.
**Decided-by:** human
**Justification:** The closeout ticket had written down, in advance, that a live run "would add only whether the platform reports *that application* holding the microphone". That sentence names the entire failure mode and then dismisses it. It is the second time this milestone: Safari's audio processes report `com.apple.WebKit.*`, and that row was dead too. Both apps passed every unit test, because the tests and the code were written from the same wrong belief about the name — a fixture can only ever assert the id you already thought of. Before these two fixes, 2 of 6 Watchlist rows could not have triggered on macOS, in a product whose headline promise is never missing a meeting. The measured 0% false-negative rate was real and was measured on Chrome, which is why it caught neither. What remains unobserved is not reassuring by analogy: Arc and Edge (declined) and all of Windows are exactly where a third instance of this would hide.
**Outcome:** applied
**Ref:** (pending)

## Q21 — m2-auto-record/05-windows-detection-vertical — finding

**Question:** Q20 ended by naming Windows as where a third instance of the wrong-identifier bug would hide. Wait for the Operator's Windows run to find out, or go looking without a machine?
**Options considered:** wait — the criterion is already open and honestly labelled / read the Windows path for the same shape and fix what reading can prove
**Chosen:** Read it. The Windows detector reports a lowercased executable name, `Watchlist::watches` compares ids exactly, and the shipped rows for Zoom, Teams and VooV are macOS bundle ids — so all three could only ever have failed to match. Added `WINDOWS_EXECUTABLES` mapping the executables onto those row ids, through `responsible_app`, which the Windows detector already routes every holder through.
**Decided-by:** agent
**Justification:** Two of the three instances of this bug were found by running the app, which made the whole class feel like it needed hardware. It did not: this one is visible in the type of the thing being compared. Ticket 05's second criterion — "the exe→app table twin ... ported as seed data" — was already checked off, which is how it stayed hidden; the macOS `.helper` rule is platform-neutral code, so sharing the *function* read as having built the *table*. A twin of a table is a table. Notably, browsers were spared only by accident: `known_browsers` lists executables beside bundle ids, so two different mechanisms now cover the two kinds of row, and the new table deliberately omits browser executables so they cannot disagree. The four executable names are the only identifiers in this project asserted from memory rather than read off a running machine — the exact mistake Q20 is about — so they are marked unverified in the source, and `windows-check.md` asks for the real ones. A wrong name matches nothing, which is precisely the behaviour being replaced, so the table cannot regress the platform while it waits.
**Outcome:** applied
**Ref:** (pending)

## Q22 — m2-auto-record/09-m2-closeout — finding

**Question:** The Operator declined Arc and Edge on 2026-08-28. After Teams (Q20) and the Windows rows (Q21), was that decline worth putting back to them, or is re-asking a settled question just noise?
**Options considered:** respect the decline and leave the row open and honestly labelled / put the new evidence to them once and let them re-decide
**Chosen:** Asked once, with the evidence. They reversed it. Installed both, drove both, uninstalled both. Edge was fine — `com.microsoft.edgemac.helper`, triggered and auto-stopped. **Arc could never have matched:** it ships as `company.thebrowser.Browser` and its helpers as `company.thebrowser.browser.*`, so stripping at `.helper` produced an id differing from the Watchlist row in one letter. Fixed by making identity comparison case-insensitive.
**Decided-by:** human
**Justification:** Re-asking was not noise, because the evidence had changed: the decline was made when the row's assumption looked like a formality, and by the time it was reversed three instances of that exact assumption had failed. This was the fourth. It also breaks the pattern the first three had suggested — Safari and Teams both needed the app *running* to expose, so the class felt like it needed hardware and driving; Arc needed only its `Info.plist`, and could have been caught the day the row was written. Two details worth keeping. First, Chrome and Edge are why nobody saw it: their vendors lowercase nothing, so `com.google.Chrome.helper` is exactly the app plus a suffix, and both had been *watched live* under those ids, which made the rule look confirmed rather than lucky. Second, the tests asserted `company.thebrowser.Browser.helper.Renderer` — a string Arc does not ship — which is the same invented-id habit as Q20, in the same file, in a test written to prove the opposite. The fix is the comparison and not an alias row for Arc, because bundle ids are case-insensitive to LaunchServices and an alias would have left the next vendor to lowercase a helper undiscovered. Residual, recorded and not closed: whether Arc's audio comes from a `.helper` process at all is still unobserved, since Arc requires an account to open a window — and that is precisely what Teams turned out to fail.
**Outcome:** applied
**Ref:** (pending)

## Q23 — m2-auto-record/05-windows-detection-vertical — finding

**Question:** Q21 shipped four Windows executable names written from memory and said so. The Operator has deferred the Windows run. Leave them labelled unverified, or find a way to check them without the machine?
**Options considered:** wait for the Operator's run, which is the only real confirmation / check them against the exe→bundle table in Granola's shipped bundle, which the absorption catalog names for exactly this
**Chosen:** Checked them. Three survived. **VooV did not:** written as `wemeetapp.exe → com.tencent.meeting`, it is `voovmeetingapp.exe → com.tencent.tencentmeeting` — wrong in both halves. Corrected. Also carried across a second id for Comet (`ai.perplexity.comet` beside our `com.perplexity.comet`), since no machine here has Comet to settle which is current and an extra browser id can only over-match a browser.
**Decided-by:** agent
**Justification:** One in four wrong is the argument for having looked. The corrected id is also the one VooV was observed under live on macOS, so the two independent sources agree, which is worth more than either alone. On provenance: Granola is not one of PORTS.md's three licensed upstreams, so what was taken is deliberately narrow — the identifiers for rows this product already ships, which are facts about Zoom's and Tencent's software rather than Granola's expression, and no code, structure, or entry for an app we do not watch. Whether Granola belongs in the ledger at all is the Operator's call, not one to make silently by adding a row; the absorption catalog directs porting from it while PORTS.md does not license it, and that tension should be resolved deliberately. What this does **not** do is close ticket 05: it establishes what the names are, not that the Windows detector reports them, and those are different claims — the same distinction Teams collapsed, where the bundle id was right and the recording process was something else entirely. The Chinese 腾讯会议 executable is still unknown, and is deliberately not guessed at.
**Outcome:** applied
**Ref:** (pending)

## Q24 — brand-identity/raster-app-icon — deviation

**Question:** The Operator supplied a finished raster icon (a white monoline seahorse on a coral gradient tile) to replace the desktop app icon. Copy it over the generated files, or make it a master the pipeline consumes?
**Options considered:** copy the PNG over `resources/icon.png` and hand-build the ico/icns once / add it as a raster master that `render.mjs` builds the desktop icons from / redraw it as an SVG master like the e
**Chosen:** A raster master: the source was cleaned once (opaque paper background knocked out; the tile, which the generator drew 656×714, squared; re-composed on Apple's 1024 grid) into `brand/src/appicon-1024.png` (+ `appicon-glyph.png` for the Icon Composer layer), and `render.mjs` now builds the `.icns`, `.icon` package, `.ico` and Electron copies from it — resizing by progressive halving, adding the standard macOS shadow at ≥64 px.
**Decided-by:** agent (the replacement itself was the Operator's instruction; this entry is the how)
**Justification:** A plain copy would be reverted by the next `pnpm -C brand render`, silently — the invariant that the script produces every committed output is what makes the assets trustworthy, so the new art has to enter as a master, not as an output. Redrawing as SVG was not asked for and would change the art. Fixed en route: the icns writer's 16/32 @1x entries (`icp4`/`icp5`) round-tripped to noise under Apple's own tooling — Apple ships those sizes as raw ARGB or not at all — so the container now carries exactly the PNG types Apple's icons use and lets macOS derive the 1x smalls. The vector e remains the mark everywhere not asked about: tray templates, favicon/web, iOS, Android, lockups — a split state, flagged to the Operator.
**Outcome:** applied
**Ref:** (pending)

## Q25 — brand-identity/raster-rebrand — deviation

**Question:** With the seahorse raster as the desktop icon, the tray, web set, iOS, Android and lockups still carried the teal e — a split identity. Propagate the seahorse everywhere, or hold the split?
**Options considered:** propagate the raster identity to every surface / keep the split and let each future milestone decide / redraw the seahorse as a vector master first and then propagate
**Chosen:** Propagate: every consumer in `render.mjs` gained a raster branch — the tray templates are the glyph tinted black with the same badge system, iOS light is the tile full-bleed over its own gradient (dark/tinted are the glyph alone), Android's adaptive foreground is the glyph in the safe circle over a coral colour resource with clipped-tile legacy icons, the web set and manifest theme follow, and the lockups set the glyph beside the outlined wordmark. The vector e remains intact as the fallback the pipeline returns to if the two `src/appicon-*.png` masters are deleted.
**Decided-by:** human (the rebrand — "update other logos accordingly"); agent (the per-surface mechanics)
**Justification:** The split was flagged when the desktop icon changed, and the Operator's instruction resolved it. Vectorizing first was rejected as silent scope: tracing the AI art into paths changes it, and every surface here consumes rasters anyway. Fixed en route: an opacity passed through the progressive-halving resizer compounded per step (0.45 five times ≈ invisible), caught because the busy tray state vanished; tint and opacity now land exactly once, in the final render. Verified live: the seahorse template with the Attention and Recording badges photographed in the real menu bar.
**Outcome:** applied
**Supersedes:** Q15 — the letterform e chosen there is no longer the shipped mark; it remains the vector fallback, and everything else Q15 rested on (the review process, the tile discipline) carries forward.
**Ref:** (pending)

## Q26 — brand-identity/vectorize-the-seahorse — deviation

**Question:** The seahorse existed only as AI-generated raster masters, which meant raster-resize machinery in the pipeline, soft small sizes, and an unusable-for-strokes Icon Composer layer. Keep the raster masters, or redraw the mark as vector?
**Options considered:** keep the raster masters and their resize/tint machinery / hand-trace the seahorse into SVG strokes and return the pipeline to vector-first / keep both paths behind a raster-override switch
**Chosen:** Hand-traced. `src/mark.svg` is the seahorse as two monoline stroked paths plus two filled dots, drawn against a measured ink map of the raster and iterated under an overlay diff until the residue was sub-stroke-width; `src/mark-small.svg` is the simplified heavy-stroke variant for 32 px and under. Masters now declare their ink box (`data-ink`), placement is portrait-aware, and the raster masters, the progressive-halving resizer, the tint filter and every raster branch were deleted — `render.mjs` is vector-first again with the coral palette (including new dark-tile tokens) and no fallback switch.
**Decided-by:** human (option 4 of the offered set); agent (the tracing and mechanics)
**Justification:** The Operator chose redrawing over keeping the raster or deleting the fallback. The trace was verified by overlaying the vector on the raster at 1:1 (differences below one stroke width), and it strictly improved the outputs the raster struggled with: the 16 px favicon and tray render from curves instead of five halvings, the Icon Composer layer is a true outline again, and the lockup's mark finally matches the wordmark's weight. The e is gone from the masters — the fallback story ended where the traced vector made it unnecessary — and survives in `explorations/` and history.
**Outcome:** applied
**Supersedes:** Q24 — the raster-master mechanism it introduced is retired; the art it carried is what the trace preserves.
**Ref:** (pending)

## Q27 — m2-auto-record/05-windows-detection-vertical — finding

**Question:** The Windows run finally happened. Q20–Q23 had all predicted the same defect shape waiting there — a meeting app recording under a name its Watchlist row does not hold — and the checklist was written to hunt for that name. Report against that prediction, or against what the machine actually did?

**Options considered:** run the checklist as written and report the exe names it asks for / add an instrument that shows the raw capture-session owner before the Watchlist filters it, because the checklist cannot see the case it is hunting

**Chosen:** Added the instrument first, and it immediately showed something the checklist could not have: **Windows detection had never worked at all, for any app.** `executable_name` asked PSAPI's `GetModuleBaseNameW` for a name over a handle opened with `PROCESS_QUERY_LIMITED_INFORMATION`. That call walks the target's module list and is documented against `PROCESS_QUERY_INFORMATION | PROCESS_VM_READ`; it returned `ERROR_ACCESS_DENIED` for every process on the machine, and a zero return is indistinguishable in that function from "no such process". So `microphone_holders` answered "nobody" while Edge plainly held the microphone. Replaced with `QueryFullProcessImageNameW`, which is documented against the right the detector asks for, taking the leaf of the path it returns. Observed working afterwards: Edge starts and stops a Meeting as `msedge.exe`, two Cores with different runtime dirs bind different pipes and answer independently, the calendar declines access without crashing. The instrument stays as `examples/mic-holders.rs`.

**Decided-by:** agent (the instrument and the fix); human (the Operator ran the session on their machine, chose to drive Teams by hand rather than have a meeting created on their account, and had the test Meeting deleted)

**Justification:** The checklist could not have found this, and neither could `Get-Process`, which says a process exists rather than that it owns the capture session. The Core logs an app only *after* a Watchlist row matches, so "nothing was named" and "nothing was watched" produce identical silence — the failure mode the whole run was built to investigate was the one the run could not see. That is why the instrument came before the procedure, and it is the transferable lesson: an observation tool that reports the input to a decision is worth more than one that reports the outcome.

On the prediction: Q21–Q23 were right that a fifth defect was waiting on Windows and wrong about its shape, and the way they were wrong matters. Four rounds of this milestone taught that the bug lives in *which name* a table holds, so a fifth round of scrutiny went into the names. Meanwhile the code that produces the name at all had no test — because nothing on that path ran anywhere. `WINDOWS_EXECUTABLES` was never reached, so its correctness was never what stood between this platform and working; the effort spent auditing it against a competitor's bundle in Q23 was, in the event, spent on the wrong end of the pipeline. Typechecking is what hid it: the call was correct Rust against a real API and simply lacked a right, and cross-compiling with `cargo-xwin` proves exactly that much. CI stayed green for the same reason it always had — `windows-latest` compiled and linked this function and never once called it.

The new test asserts `executable_name(std::process::id())` against the name read from `current_exe` at runtime, rather than a string written down. That is deliberate: two of the four defects in Q20–Q23 shipped with tests asserting identifiers nobody had read off a machine, and a test that invents its expectation cannot fail the way this one does.

What this does **not** establish: no meeting app has been observed holding the microphone on Windows. That machine has Edge and Teams and nothing else, so `WINDOWS_EXECUTABLES` is still unobserved and the ticket-09 browser matrix has exactly one row. Teams is the standing risk and is left named as unknown — it runs WebView2-hosted there, one `ms-teams.exe` beside 24 `msedgewebview2.exe` children, which is the same helper shape as `com.microsoft.teams2.modulehost`. Adding `msedgewebview2.exe` on that reasoning would be the Q20 mistake with a new spelling, and it would match every WebView2 app besides.

**Outcome:** applied

**Ref:** (pending)

## Q28 — m2-auto-record/05-windows-detection-vertical — gate-resolution

**Question:** The Windows run fixed the platform but reached only Edge — no Zoom, VooV, 腾讯会议 or the other browsers, and Teams was never driven into a call. Keep the meeting-app row open until someone runs them, or close live Windows testing?
**Options considered:** hold it open until the matrix is actually complete / close it as the Operator's decision, with the unreached cases named as standing risk
**Chosen:** Closed on the Operator's instruction (2026-08-31). The residual is recorded specifically rather than generally — Teams on Windows is WebView2-hosted, one `ms-teams.exe` beside 24 `msedgewebview2.exe` children, so its capture session may belong to a name the row does not hold; 腾讯会议's executable is unknown; Zoom, VooV and five browsers are unobserved there.
**Decided-by:** human
**Justification:** The run did the thing that mattered: it found that Windows detection had never worked at all — `GetModuleBaseNameW` denied on every process, so `microphone_holders` always answered "nobody" — and fixed it. Holding the ticket open for a fuller matrix would confuse two different states: a platform that does not work, and a platform that works and has been exercised on one app. Only the first is a milestone blocker. The Operator has the machine and has ended this line of work, and more of their time is theirs to offer. The reason for naming Teams so precisely is that it is the one case with a *predicted* failure shape rather than a general unknown, and the prediction must not become the fix: adding `msedgewebview2.exe` on the strength of the reasoning would match every WebView2 app and would be Q20's mistake with a new spelling. `examples/mic-holders.rs`, which the run added, settles it in one command whenever a signed-in Teams call exists.
**Outcome:** applied
**Ref:** (pending)
