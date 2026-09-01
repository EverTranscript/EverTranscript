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

## Q29 — m2-auto-record/05-windows-detection-vertical — deviation

**Question:** The Windows run recorded that the detector reads only the default capture endpoint and framed it as "a product question nobody has asked yet". Leave it as a recorded limitation, or fix it?
**Options considered:** leave it recorded, since watching every microphone is a behaviour change nobody asked for / fix it, because the gap is a false negative and false negatives are the product-defining risk
**Chosen:** Fixed. `microphone_holders` enumerates every active capture endpoint via `EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)` instead of asking for the `eMultimedia` default.
**Decided-by:** human
**Justification:** Investigating it turned up a sharper form the run had not named, and it changes the answer. Windows keeps a separate default per `ERole` and directs communications software at `eCommunications` — which it reassigns on its own when a headset appears. Meeting apps are communications software. So the failure was never confined to "a second microphone", which is a rare setup; it was "the Operator plugged in a headset", which is the ordinary case and is the same device churn ADR-0023's continuity window already exists to survive. Framed that way it is not a product question about watching more microphones, it is a false negative on the platform ADR-0025 makes the ship gate, and the PRD puts false negatives first among risks. Two details of the shape are deliberate: per-device failures never abort the scan, and an empty endpoint list logs, because the defect this platform shipped with was a call that failed and was indistinguishable from an idle machine — the same postmortem must not be reachable twice. On confidence: it cross-compiles clean under `clippy -D warnings`, which is worth nothing about runtime and is precisely what hid the last defect; what makes it better founded is that the enumeration is lifted from `examples/mic-holders.rs`, which ran on the real machine. It is still **unobserved in the detector**, and one `mic-holders` run against a headset would settle both that and whether the two roles disagree in practice.
**Outcome:** applied
**Ref:** (pending)

## Q30 — m2-auto-record/05-windows-detection-vertical — finding

**Question:** Q28 closed live Windows testing on the grounds that the machine had only Edge and Teams. The Operator then installed Zoom, 腾讯会议 and VooV Meeting on that machine and asked for the three to be tested end to end. Report against the closed ticket, or reopen what the new hardware can now answer?

**Options considered:** treat the gate as resolved and note the installs for a later run / run the three now that they exist, and correct whatever the run contradicts

**Chosen:** Ran them. **腾讯会议 held the microphone through an entire live meeting while the Core sat at Idle and logged nothing** — the false negative Q21–Q23 kept predicting, finally observed. It records as `wemeetapp.exe`, which `WINDOWS_EXECUTABLES` did not carry, because Q23 had *replaced* that name with `voovmeetingapp.exe`. Installing both Tencent builds side by side showed why that was wrong: they are two products with two launchers — `Program Files\Tencent\WeMeet\WeMeetApp.exe` for 腾讯会议 and `Program Files (x86)\Tencent\VooVMeeting\VooVMeetingApp.exe` for VooV — each holding its own capture session in that top-level process, not in a helper. Added `wemeetapp.exe` beside the existing row and verified live: the same meeting that produced silence now logs `Auto-Record started a Meeting app="com.tencent.tencentmeeting"` and stops on release. VooV was also observed end to end and already worked. Zoom registers its sessions under `zoom.exe` — the row it already has — but never went active, so Zoom remains unobserved.

**Decided-by:** human (the Operator installed the three apps and asked for the test); agent (the diagnosis, the fix, and the method)

**Justification:** Q23's error was not the name, it was the *inference*. Checking `wemeetapp.exe` against Granola's table found the string absent and concluded it was wrong; what the table actually showed is that Granola covers the international build only. A second source can establish that a name exists somewhere. It cannot tell you which of a vendor's products it belongs to. So a name that was right for 腾讯会议 was swapped for the name of a different product, and the row for the app most likely to be used on a Chinese-locale machine went dark — while the entry that replaced it kept passing every test, because the test asserted the same substituted string. That is the Q20 shape again, one level up: not an invented identifier this time, but an invented *equivalence* between two products.

Worth keeping about method. Three input paths — `mouse_event`, `SendInput`, and UI Automation's `Invoke` — were all silently ignored by Zoom and VooV, which filter injected input; UIA reported success while doing nothing. What worked was each app's own deep link (`wemeet://page/inmeeting?meeting_code=…`, read from a meeting the account already had), which drives the app the way the OS does. That is the technique to reach for next time a conferencing client has to be driven, and it is why Zoom is still unobserved: its meeting id could not be obtained without a click, its local store being encrypted.

**Also corrected:** Q28's ticket text asserted the machine had "Edge and Teams and nothing else — no Zoom, VooV, 腾讯会议". True when written, false within the hour, and left standing it would have made the next reader trust a matrix the hardware contradicts.

**Not settled, and not guessed at:** whether the shipped `VooV Meeting` row (`com.tencent.meeting`) is reachable by anything at all — both Tencent launchers resolve to `com.tencent.tencentmeeting`, which is also the id VooV was observed under on macOS, so that row may simply be dead. Zoom's capture session was never seen active. Teams was never driven into a call, so `msedgewebview2.exe` remains the standing risk named in Q28.

**Outcome:** applied

**Ref:** (pending)

## Q31 — m2-auto-record/05-windows-detection-vertical — finding

**Question:** Q30 closed with Zoom unobserved, explaining that Zoom "filters injected input" — three input APIs had appeared to do nothing. The Operator asked for Zoom again. Accept that explanation and look for another way in, or test the explanation first?

**Options considered:** hunt for a Zoom meeting id or a deep link that needs no click / test whether the injection works on anything at all before believing a claim about Zoom

**Chosen:** Went to test the claim, and found the meeting already running. A `Zoom 会议` window was open, holding an ACTIVE capture session as `zoom.exe`, with the Core recording — so a click had worked during the very attempts that were written up as ignored. Zoom is now observed end to end: `Auto-Record started a Meeting app="us.zoom.xos"`, stopping on release, with the capture session in the meeting process itself and **not** in `aomhost64.exe`, `airhost.exe` or `CptHost.exe`, which was the standing worry since Zoom ships all three. The filtering claim is withdrawn as unevidenced rather than replaced with its opposite: what Zoom does with injected input was never established, and does not need to be.

**Decided-by:** human (asked for Zoom to be retried); agent (testing the explanation rather than routing around it)

**Justification:** This is the milestone's own failure mode, committed by the process investigating it. "Zoom filters injected input" is an assertion about a third party that was written into a commit message, a ticket and a decision entry on no evidence beyond "the symptom stopped there". It has exactly the shape of `wemeetapp.exe` being declared wrong because a competitor's table lacked it: a plausible story, adopted because it explained the symptom, never tested.

The attempt to test it went wrong in a way worth recording separately, because it is subtler and it nearly stuck. The control was to inject into Notepad and see whether text appeared; it came back negative, which looked like it had settled the question in favour of "the harness is broken". It had not. Notepad was *behind the terminal*, so the click went to the terminal and the control exercised nothing. **A negative control that could not have succeeded is worse than no control**, because it produces the feeling of evidence without any. The rule that survives is not about input injection at all: an explanation adopted because it accounts for the symptom is not an observation, and neither is a check that could not have come out the other way.

It also cost the observation twice over. The meeting was already running when Zoom was written up as unobserved, so the ticket carried "believed-good and unproven" about a row that had, by then, been proven — and the correction only happened because the Operator asked again rather than accepting the report.

Two incidental findings worth keeping. Zoom registers its capture sessions at launch and leaves them `inactive` until a meeting starts, so the presence of a session proves nothing about the microphone and only `AudioSessionStateActive` does — which is what the detector already keys on. And killing the meeting process made Zoom respawn it under a new pid, across which the Core held a single Meeting, because attribution is by executable name rather than by pid.

**Now settled on Windows:** Edge, Zoom, 腾讯会议 and VooV all observed starting and stopping a Meeting. **Still not:** Teams, which was never driven into a call — `msedgewebview2.exe` remains the standing risk named in Q28 — and the five unrun browsers.

**Outcome:** applied

**Ref:** (pending)

## Q32 — m2-auto-record/05-windows-detection-vertical — finding

**Question:** Teams and the browsers were the last unrun rows on Windows, and Teams carried a named suspicion since Q28: WebView2-hosted, so its capture session might belong to `msedgewebview2.exe` rather than the row's `ms-teams.exe`. Run them, or ship the suspicion as documented risk?

**Options considered:** leave Teams as a named standing risk and let a user find out / drive a real Teams call and a real `getUserMedia` page and read the endpoint

**Chosen:** Ran them, and **both standing suspicions were wrong**. In a live Teams call the capture session belongs to `ms-teams.exe`, and no `msedgewebview2.exe` process holds one at all — out of twenty-five. Zoom's belongs to `zoom.exe`, the meeting process, not to `aomhost64.exe`, `airhost.exe` or `CptHost.exe`, all of which Zoom ships. Chrome and Edge both start and stop as themselves. With 腾讯会议 and VooV from Q30, every app this milestone watches has now been seen starting *and* stopping a Meeting on Windows.

**Decided-by:** human (asked for Teams and the browsers); agent (the runs)

**Justification:** The value here is entirely in the negative results, and they are worth more than they look. Q28 wrote `msedgewebview2.exe` down as the likely answer and — correctly — refused to add it, on the grounds that it would match every WebView2 application on the machine. Had that reasoning been weaker, the table would now carry a name that is both wrong and dangerously broad, and it would have looked like a fix. Two milestones of evidence say the bar is *observation*, and this is the case where holding that bar prevented a defect rather than merely delaying a fix.

There is a real asymmetry worth recording between the two platforms. On macOS, Teams records under `com.microsoft.teams2.modulehost` — a helper with its own identifier, which is what made it invisible to an exact-match row. On Windows the process holding the session is also not the process owning the window; it is a *second* `ms-teams.exe`. The reason that is harmless is that Windows attribution is by executable name, so two processes of the same binary are the same app for free. The defect macOS had is structurally unreachable here, which is not something reading the code would have told you and is exactly the sort of thing the parity gate exists to check.

**Still unrun:** Firefox, Brave, Opera and Arc on Windows. They are `known_browsers` entries reached by the route Chrome and Edge just demonstrated, so the residual risk is low — but low is not observed, and the ticket says so rather than rounding up.

**Outcome:** applied

**Ref:** (pending)


## Q33 — m2-auto-record/05-windows-detection-vertical — finding

**Question:** Firefox, Brave, Opera and Arc were the last unrun rows. Run them, or accept them by analogy with Chrome and Edge, which had just passed by the same route?

**Options considered:** accept the four as low-risk, since all are `known_browsers` entries reached by a mechanism twice demonstrated / run them

**Chosen:** Ran them. Firefox, Brave and Opera each start and stop a Meeting under their own executable name. Firefox needed a different lever — it is not Chromium and ignores `--use-fake-ui-for-media-stream`, so the prompt was pre-granted with a profile `user.js` setting `media.navigator.permission.disabled`, which grants the permission without faking the device. **Arc could not be run at all:** it installs and opens straight to a "Sign In to Arc" window, and without an account it never opens a browsing window, so it cannot reach the microphone. Its name is confirmed — the detector would read `arc.exe`, matching the shipped row — but its audio path is unobserved.

**Decided-by:** human (asked for the four); agent (the runs and the Firefox lever)

**Justification:** One genuinely new fact came out of running rather than assuming, and it is the kind that only appears on real hardware: **every browser observed holds its capture session in the main process**, not in a renderer, GPU or audio child. So on Windows the `.helper` suffix rule that `responsible_app` exists for is never exercised by a browser. The entire class of defect that cost Safari (`com.apple.WebKit.GPU`) and Arc (`company.thebrowser.browser.helper`) on macOS has no Windows analogue, because the Windows names do not branch. That is a real asymmetry between the two platforms' detectors, it was invisible from the code, and it means the browser half of this table is far more robust here than there.

Arc is left open rather than closed by analogy, which is the same call made about it on macOS for the same reason. Five browsers passing by one mechanism is decent evidence about the sixth, and this milestone's record is that decent evidence about a name is exactly what keeps being wrong. The difference between "very likely fine" and "observed" is the whole subject of the ticket.

**Outcome:** applied

**Ref:** (pending)

## Q34 — m2-auto-record/05-windows-detection-vertical — deviation

**Question:** `known_browsers` carried nine browser identities — Chrome, Safari, Arc, Edge, Firefox, Brave, Vivaldi, Opera and two ids for Comet. The Operator asked to narrow Browser Meetings to Chrome, Edge, Safari and Firefox only, removing the rest.

**Options considered:** keep the wide list, since an extra browser id can only over-match a browser / narrow to the four named

**Chosen:** Narrowed. `known_browsers` is now Chrome, Safari, Edge and Firefox plus `chrome.exe`, `msedge.exe` and `firefox.exe`; Arc, Brave, Vivaldi, Opera and both Comet ids are gone. A test asserts the removed ids **do not** match, so re-adding one is a visible decision rather than a quiet drift back.

**Decided-by:** human

**Justification:** This is a deviation from ADR-0030 on two counts and both should be said plainly. That ADR names the M2 browser matrix as "Chrome, Safari, Arc, Edge"; the shipped set now drops Arc and adds Firefox. It also reasons that an extra browser id "can only ever over-match a browser, which is what the Browser Meetings row wants anyway" — an argument for breadth that this narrowing reverses. The ADR is left unedited and this entry is the record, per the repo's practice of journalling deviations rather than rewriting ratified decisions.

**The cost is a silent false negative, which is the failure mode the PRD ranks first.** An Operator whose daily browser is Brave now gets no Browser Meeting, and nothing in the product tells them why — the row still reads "any browser in a call", which after this is an overstatement. That wording is left alone rather than quietly narrowed, because it is Operator-visible text and the glossary in `CONTEXT.md` is normative; whether the label and the glossary should change is a separate call and is flagged rather than made here.

What makes the narrowing defensible is the evidence line rather than the count: every id that remains has been watched holding a microphone on at least one platform, and none of the five removed could say that. Arc could never even be driven — it will not open a window without an account, on either platform, which is why it is the one row this milestone never observed. Brave and Opera, by contrast, *were* observed starting and stopping a Meeting on Windows an hour before they were removed, so their rows are retired with working evidence behind them and re-adding either is one line plus a test.

**Outcome:** applied

**Ref:** (pending)


## Q35 — m2-auto-record/05-windows-detection-vertical — deviation

**Question:** Q34 narrowed `known_browsers` to Chrome, Edge, Safari and Firefox on the Operator's instruction. The Operator then said the label "any browser in a call" still stands, that the four are what gets *tested* because they are the top market-share browsers, and that EverTranscript "will most likely work on any browser". Implement the narrowing as written, or reconcile it with what the code actually does?

**Options considered:** keep the narrowed list and correct the expectation / restore the wider list and treat the four as a test matrix / detect browsers generically instead of by list

**Chosen:** Put it to the Operator with the evidence, and restored the wider list on their answer. Arc, Brave, Vivaldi, Opera and both Comet ids are back; Chrome, Edge, Safari and Firefox are recorded as the **testing** priority rather than the supported set. Generic detection was offered and not taken — ADR-0030's blocklist exists precisely because Electron apps look like browsers to a naive test, so "any browser" by inference trades a false negative for a false positive.

**Decided-by:** human (both the narrowing and its reversal); agent (noticing the two instructions could not both be true of this code)

**Justification:** The two statements — "narrow to four" and "will most likely work on any browser" — are consistent as *product intent* and contradictory as *code*, because detection matches an exact executable name or bundle id and has no notion of a browser beyond this list. Nothing infers browser-ness. So narrowing the list does not lower a test bar, it deletes working detection: Brave and Opera had each been watched starting and stopping a Meeting on this machine within the hour, and after the narrowing Brave held the microphone through a live page with the Core sitting at Idle. That was verified, not predicted, which is the only reason it was catchable before shipping.

The engine argument is sound for prioritising and unsound for pruning, and the difference is worth stating because it is genuinely subtle. Chrome, Edge, Safari and Firefox really are Chromium, Chromium, WebKit and Gecko, so exercising them does cover the rendering and capture paths that matter. But an engine being standardised says nothing about the *name a derivative ships under*, and the name is the entire matching key. Brave is Chromium and matches only because `brave.exe` is written in the list.

Two process notes. First, this is the session's own failure mode once more: a plausible general claim ("browsers are standardised, so it will work") that nobody had checked against the mechanism. It differs from Q20–Q23 and Q31 only in being caught before it shipped rather than after. Second, the asymmetry now written into `known_browsers`: **adding** an id still requires observation, because inventing them is the habit this milestone is a record of, while **removing** one now requires more than a tidying instinct, because removal is the change that silently costs detection.

**Supersedes:** Q34 — the narrowing it recorded is reverted. What survives from it is the test-matrix priority and the observation that the Operator-visible label and the code had drifted apart.

**Outcome:** applied
## Q36 — m3-diarization/09-m3-closeout — finding

**Question:** M3's owed number is a Diarization Error Rate. There was no labelled multi-speaker audio to measure against, and no second person to record. Report the solo case and call the rest unmeasurable, or construct something and measure it?
**Options considered:** measure only the real solo recording and leave DER open / construct a labelled two-speaker file from the one real voice and measure against it, saying plainly what the construction is
**Chosen:** Constructed it. Voice A is the mic channel of the M1 dogfood recording; voice B is the same passage resampled down; four 15 s turns, A-B-A-B, with an energy VAD for the speech reference. **DER 3.9%**, two speakers found for two, confusion 0.0%, 1.6 s missed. Cross-voice separation 0.848 same / 0.202 different against a 0.62 floor.
**Decided-by:** agent
**Justification:** The construction found three defects that every unit test had passed, which is the whole argument for measuring something imperfect rather than nothing. **(1)** The catalog's Voiceprint span rules — clip to the middle 10 s, drop under 1.5 s — were being used to define *turns*, so 28% of speech had no speaker at all; correct for choosing what to embed, disastrous for saying who talked. **(2)** One embedding per contiguous span made two people alternating without a pause into one speaker, at 23.6% confusion. **(3)** `agglomerate` was not agglomerative: a single pass joining each cluster to the first earlier one within threshold split one voice into two groups that never got compared, giving three speakers in a two-speaker recording. DER went 38.4% → 26.6% → 3.9% across the three fixes. What the number is **not** is equally important and is written into the ticket rather than buried: voice B is not a second person, so 3.9% is evidence that the pipeline separates two acoustically distinct voices and is not a DER on a real meeting, which is still owed. The embedding bake-off is likewise not run and is recorded as not run — a bake-off with one entrant is a preference wearing a lab coat, and it should be run against the same real audio when there is some.
**Outcome:** applied
**Ref:** (pending)

## Q37 — m2-auto-record/05-windows-detection-vertical — finding

**Question:** Arc was the last unobserved row, blocked behind an account wall on both platforms. The Operator signed in. Run it, or leave the standing risk as written?

**Options considered:** leave it as a documented residual, since five browsers passing by one mechanism is decent evidence / run it now that the wall is gone

**Chosen:** Ran it. `Auto-Record started a Meeting app="arc.exe"`, stopping on release. **The capture session belongs to `arc.exe` itself — the MSIX main process, not a helper.** With that, every app and every browser this milestone watches has been observed starting *and* stopping a Meeting on Windows.

**Decided-by:** human (signed into Arc, which was the actual blocker); agent (the run)

**Justification:** Arc is the browser that cost this milestone a defect on macOS, so it is the one where "probably fine" was least worth accepting. There it ships as `company.thebrowser.Browser` with helpers as `company.thebrowser.browser.*`, and that one-letter case difference made it match nothing (Q22). On Windows the equivalent defect **cannot occur**: the session is held by the main process under the same name the row carries, with no helper and no case to disagree about. Six browsers for six now hold the session in the main process, so the `.helper` rule that `responsible_app` exists for is never exercised by a browser on this platform — the whole class that cost Safari and Arc on macOS is structurally absent here.

Two process notes, both about not repeating this session's own mistakes. The account wall was correctly refused rather than worked around: creating the account meant choosing a password and accepting Arc's Terms of Service as the Operator, which is not an agent's to accept — so it waited for them, and that was the right trade even though it left the row open for hours.

And the automation failures on the way were misdiagnosed twice before being measured. Arc appeared to ignore clicks and keystrokes; the actual cause was that **Arc lost foreground between separate shell invocations**, so the input was landing in whatever window had taken focus — once, demonstrably, typing a URL into Edge. `Ctrl+T` proved input reached Arc when it genuinely was foreground. The fix was to do focus, verify-foreground, paste and Enter inside a single invocation, aborting if the foreground check failed. Same shape as Q31: an explanation about a third party, adopted because it fit the symptom, wrong once actually tested.

**Outcome:** applied

**Ref:** (pending)

## Q38 — m4-summary/09-m4-closeout — finding

**Question:** M4 owes a quality number. The bundled local model is a 0.5B that the registry comment already calls too weak. Measure with it anyway, or download a larger one first so the number means something?
**Options considered:** ship a bigger model and measure that / measure the model that is actually bundled and report what it does
**Chosen:** Measured the bundled one. On the M1 dogfood recording — which contains two plain commitments, deferring the hiring plan and sending revised numbers by Friday — the Summary read, in full, `None noted.` **Zero of two action items.**
**Decided-by:** agent
**Justification:** The number is bad and it is the honest number for what is in the box today, which is the only kind worth putting in a close-out. It also confirms a prediction that was written down *before* it was measured: the `SUMMARY_DEFAULT` comment says the registered model "is the model that was verified, not the model that should ship". Measuring it turns that from an opinion into evidence for choosing the real default, which is the work still owed. What the run did earn was two defects that no unit test could have found, both now fixed: greedy sampling with no repetition penalty made the model restate the transcript five times to the token ceiling, and the absence of stop handling wrote the prompt's own scaffolding — the literal sentence "The operator's own notes from this meeting" — into a stored Summary. Neither is a model-quality issue; both are pipeline bugs that only appear when a real model runs. What the measurement is **not** is a test of the thing M4 is most likely to get wrong: the recording is 89 seconds, so map-reduce never engaged, and chunk-boundary behaviour on a ninety-minute meeting remains exercised only against the fake.
**Outcome:** applied
**Ref:** (pending)

## Q39 — m4-summary/07-the-knob — gate-resolution

**Question:** The one-way fallback is the property that stops a network blip becoming an exfiltration. Enforce it with a conditional, or arrange the code so the wrong direction cannot be written?
**Options considered:** a `strict`/direction flag checked in the fallback path / a function signature in which no cloud Backend can be passed as a fallback
**Chosen:** The signature. `knob::run` takes the chosen Backend and a `local_fallback`, and the Core hands it a named `ChosenBackends` pair whose second element is always local. There is no argument, field, or branch through which a failing local Backend could reach a cloud one.
**Decided-by:** agent
**Justification:** This is the first milestone where a bug leaks meeting content, and every other failure in this product is recoverable — a lost recording, a missed meeting, a mislabelled speaker. Sending a transcript to a provider the Operator did not choose is not. A boolean that happens to be false and a function that cannot express the wrong thing are different guarantees, and only the second survives a future edit by someone who has not read this entry. The tests are written to tell them apart: each drives all four failure shapes and asserts the *other* Backend was never called, rather than only that the right one answered. Cancellation is excluded from fallback for the same reason — an Operator who pressed stop must not discover that stopping is what sent their transcript somewhere. The gate on choosing Cloud lives in the Core rather than the UI on the same principle: a gate a Client can walk around by forgetting to call it is not a gate.
**Outcome:** applied
**Ref:** (pending)

## Q40 — m5-onboarding/04-floating-indicator — gate-resolution

**Question:** The PRD lists a Core-native floating mini-indicator as an M5 *evaluation*. The catalog has the exact Electron recipe. Build it, or keep the tray as the only always-visible indicator?
**Options considered:** build the floating nub from the catalog's recipe / keep the Core-owned tray alone and record why
**Chosen:** Not building it. The tray stays the only always-visible recording indicator (ADR-0026).
**Decided-by:** agent
**Justification:** The recipe is available and the work is small, which is exactly why the decision needs a reason rather than a shrug. **A Client-owned indicator has a defect the tray does not: it disappears when the Client is closed, and the Core keeps recording.** That is not a corner case — it is the product's normal state, since ADR-0026 makes the Core an always-on daemon and the Client a thin optional window, and Auto-Record starts meetings without opening one. An indicator that is absent precisely when someone has closed the window and forgotten they are recording is worse than no second indicator, because its usual presence teaches an Operator to read its absence as "not recording". The one thing the tray genuinely loses to is fullscreen — the menu bar hides, and the catalog's `visibleOnFullScreen` flag is aimed at exactly that. But M1 already answered the fullscreen case by making the tray icon reachable on mouse-to-top, and story 4 in the PRD describes it that way. So the gap this would close is narrower than it first looks, and the cost is a second always-on-top window whose correctness depends on a six-flag incantation that neither steals focus nor blocks clicks. Revisit if a real Operator reports losing track of a recording in fullscreen; that report would be evidence, and this is not.
**Outcome:** applied
**Ref:** (pending)

## Q41 — m5-onboarding/08-polish — finding

**Question:** M5's polish pass includes a staleness review of `CONTEXT.md`, whose glossary is normative. Did four milestones of ADRs leave anything wrong in it?
**Options considered:** read it and confirm it is current / read it against what the code now does
**Chosen:** One real contradiction, now corrected. `CONTEXT.md` defined Sanctioned Traffic as "the enumerable, **content-free** network calls the product may ever make" and then listed the cloud Summary Backend as the third. That Backend sends the full text of a meeting — it is the only path by which anything said in a meeting can leave the machine, and sending it is the entire point.
**Decided-by:** agent
**Justification:** This is the second time this file has contradicted itself in a way that mattered; M2 found the first (Meeting Detection calling itself the "single" ambient sense eleven entries above a list of two). The wording is inherited from ADR-0034, which says "enumerable, content-free list" in one sentence and then, in the next, rewords the guarantee test to "none of it carrying meeting content" — a phrasing that is only true of entries one and two. The glossary is normative for every spec written after it, and this is exactly the sentence a privacy-conscious evaluator would quote back after finding the cloud Backend. Corrected in `CONTEXT.md` to say plainly that two of the three are content-free and the third is not, with an `_Avoid_` entry naming the phrase. **ADR-0034 itself is left alone**: amending a ratified decision is not a polish-pass act, and the ADR's substance — three entries, enumerable, one requiring an explicit choice — is right. What was wrong is one adjective, and the glossary is where it does damage.
**Outcome:** applied
**Ref:** (pending)

## Q42 — m5-onboarding/09-m5-closeout — finding

**Question:** Ticking a ticket's criteria is mechanical, so I did it mechanically — a blanket replace of `- [ ]` with `- [x]` across the file, then hand-corrections for the ones that were not done. What did that cost?
**Options considered:** n/a — this records a mistake, not a choice
**Chosen:** It falsely marked five criteria in the M5 close-out as met: a clean-machine install by someone who did not build it, the Briefing read by anyone but its author, onboarding walked on a bare machine, the permission set checked against a signed bundle, and both platforms installed from real artifacts. **None of those has happened.** Corrected, and the ticket now says why they are open.
**Decided-by:** agent
**Justification:** Recorded because of what it nearly did rather than what it did. This project has spent five milestones learning that a checkbox asserting something nobody observed is the most expensive kind of wrong — M2 shipped six such defects, and the M2 close-out had *written down in advance* the exact failure mode it then missed. Ticket 09's criteria are the ones this repository structurally cannot self-serve, and a blanket edit marked precisely those as done. The correction took a minute; had it survived, it would have said the milestone was validated by a person who does not exist. The lesson is narrow and worth keeping: **an edit that ticks boxes should never be able to tick a box its author did not read**, and the earlier tickets in this milestone where the same blanket replace was used should be treated as suspect for the same reason.
**Outcome:** applied
**Ref:** (pending)

## Q43 — m5-onboarding/09-m5-closeout — finding

**Question:** The close-out asks for onboarding to be walked on a machine with no models, no permissions and no keys. There is no such machine here — but is there a way to reach that *state*?
**Options considered:** leave it for the clean-machine run / construct the bare state locally and walk it
**Chosen:** Constructed it, and constructing it found three defects. There was no override for Application Support, so models and `settings.json` were shared with the developer's machine: a "fresh" run inherited the real acknowledgment, and recording appeared to start before the Briefing — an invariant violation that was really a test reading the wrong file. Two guarantee tests set `EVERTRANSCRIPT_MODELS_DIR`, **which nothing reads**. And the test named `a_full_cycle_with_summary_...` never generated a Summary.
**Decided-by:** agent
**Justification:** The second defect is the serious one and it is mine, from M4 and M5. Those tests copied models into a directory the Core never looked at, then ran against whatever the machine happened to have — so on a CI runner with no models they would have exercised a Core that could not diarize or summarize, observed no network traffic, and passed. A guarantee test that passes because there was nothing to do is worse than a missing one: it occupies the space where the real check would go. Both now assert the Core can actually see the models before proceeding, so they fail rather than pass vacuously. The third is the same shape one level down — a test whose *name* claimed a Summary it never asked for. And the first is why none of this surfaced earlier: without `EVERTRANSCRIPT_APP_SUPPORT_DIR` there was no way to be in the state every real first run is in, so nobody had been. With it, the pre-capture invariant demonstrably holds, models read `missing`, no Backend is preselected, and a Meeting still records with no models at all — ADR-0019's degradation working. **This is the criterion doing exactly what it was written to do**: the close-out said every prior milestone found its defects by running the real thing on real input, and the one form of that available here found three.
**Outcome:** applied
**Ref:** (pending)

## Q44 — m5-onboarding/09-m5-closeout — finding

**Question:** The close-out's last self-servable criterion is installing on Windows, and there is no Windows machine here. But CI has one — can the runner install the artifact it just built?
**Options considered:** leave it for the Operator's machine / have CI install the NSIS package and run what it installed
**Chosen:** Had CI install it. **The Windows installer did not contain the Core.** `resources\evertranscript.exe` was absent from a package that installed without error. Chasing why nobody had noticed found the deeper half: the Client never looked in its own bundle for the Core on *either* platform.
**Decided-by:** agent
**Justification:** Two defects, and the second explains the first's survival. The staging step writes `evertranscript.exe` on Windows; `extraResources` named `evertranscript`. **electron-builder skips a missing source in silence** — no warning, exit 0, a 94 MB artifact uploaded — so a hollow installer passed a green matrix, a published checksum, and a manifest generated from real artifacts. Every one of those checks was about a file rather than a product. The reason it went unnoticed is that `coreBinary()` searched `EVERTRANSCRIPT_BIN`, `PATH`, and a checkout's `target/`, and never `process.resourcesPath` — neither the macOS zip nor the NSIS installer puts anything on `PATH`, so the bundled Core was unreachable on both platforms and a Windows package that never contained one behaved no differently from a macOS one that did. The bundle's copy now wins *over* `PATH` rather than filling in after it: the Core is replaced wholesale when the Client updates (ADR-0016), and a `PATH` entry winning would pin an Operator who once installed a Core by hand to that Core across every update — the protocol skew ADR-0028 exists to survive, reached deliberately instead of by accident. Both artifacts are now searched for both binaries rather than trusted to contain them. **The pattern is the one this project keeps paying for, one layer further out than before**: M2 found identifiers the machine did not honour, M3 a pipeline that measured wrong, M4 a sidecar that hung, Q43 tests that passed vacuously — and this is a *release artifact* that was verified as a file and never as a product. "The installer builds" and "the installer installs something that runs" turned out to be different claims, and only the first had ever been checked.
**Outcome:** applied
**Ref:** (pending)

## Q45 — m4-summary/04-local-sidecar — finding

**Question:** Two M4 criteria have said the sidecar "cross-compiles for Windows, which is worth nothing about runtime" since the milestone closed. CI has a Windows runner. What happens if it actually loads a model and generates?
**Options considered:** wait for the Operator's Windows machine / write a model-gated inference test and fetch the model on both runners
**Chosen:** Wrote the test. It found a defect on its **first run, on macOS**, before Windows was reached: `</transcript>` was not a stop sequence, so a model that replays its prompt runs to the end of it. The shipped default also fabricates timestamps.
**Decided-by:** agent
**Justification:** `STOP_SEQUENCES` listed `<transcript>` and not `</transcript>` — the guard was on the marker that **cannot** appear and absent on the one that does. `escape_control_markers` puts a zero-width space inside both tags in every untrusted string, so a literal opening tag can only come from the model, and by the time it writes one it has already replayed the prompt; the closing tag is what a replay actually reaches. Fixed, and safe to stop on for exactly the reason the escaping exists.

The second finding is worse and is not a code bug. Asked to summarise three lines containing one plain commitment, the registered 0.5B answered `None noted.`, then contradicted itself with four `Who | What | When | Said at` rows, then reproduced all three transcript lines verbatim. **The `Said at` values — `14:00`, `12:30` — and the `When` values `Monday`, `Thursday morning` appear nowhere in the input.** Rule 7 of the system prompt says "Do not guess at names, dates, or commitments"; rule 5 gives `Said at` the specific job of letting an item "be checked against what was actually said". So the column that exists to make the Summary auditable is the column being invented, in a record ADR-0009 makes immutable. M4 recorded this model as weak — "zero of two action items" — but weak understates it: it is not failing to find action items, it is manufacturing evidence for ones it did not find.

The test **reports** that measurement and does not assert on it. Asserting would fail a platform test on every platform for a model's sake, and would quietly move M4's open "choose a real default model" criterion into this test — where the next person to see it red would be tempted to weaken it. The number prints on every run instead, and the criterion stays where it belongs, now with evidence it did not have.

One assumption checked rather than reasoned about: `cargo test --workspace` does **not** build `target/<profile>/evertranscript-summarizer`. It builds the summarizer's unit-test harness into `deps/` and stops. Found by deleting the file and re-running, which is the only reason CI is not red.
**Outcome:** applied
**Ref:** (pending)

## Q46 — m4-summary/04-local-sidecar — finding

**Question:** The CI run for `d89445c` looked like it had sat in its `Tests` step for over seven hours while Windows finished in eleven minutes, and the diff cannot cause that — it is behaviourally identical whenever the model exists. What makes a stall unbounded?
**Options considered:** wait for the log and fix what it names / read for the mechanism first
**Chosen:** Read for the mechanism, which was the only option available while the run was in progress and its log undownloadable. **The hang was not real** — this machine's clock was running about six hours and forty minutes fast, and `gh` computes a running job's elapsed time against local time, so a healthy 24m51s job read as seven hours. The run finished green. **The defect the false alarm turned up is real:** `sidecar::REQUEST_TIMEOUT` was declared and enforced nowhere.
**Decided-by:** agent
**Justification:** Recording the wrong premise rather than quietly keeping the right answer, because the two are worth different things. The premise was an artifact and is worth one line. The finding is a grep result and stands on its own: the constant is the catalog's M4 bound, and its only reference anywhere in the workspace was its own doc comment. `cloud::REQUEST_TIMEOUT` is applied at `cloud.rs:171`; this one was applied to nothing, so `exchange` blocked in `read_line` with no deadline. Clippy cannot say so, because a `pub` constant nobody reads is not dead code — the same reason a `pub` helper nobody calls survives review.

**The test that looks like it covers this does not.** `a_sidecar_that_dies_is_unreachable_rather_than_a_hang` drives a child that exits, and exiting is precisely what makes that case detectable: the pipe reaches EOF and the read ends on its own. A child that stays alive holding half a gigabyte and simply stops answering produces no EOF, so the read never ends — that is the shape ADR-0031 bought the process boundary to survive, and it was the one with neither a bound nor a test. The new one drives a fake that loads, replies `ready`, then goes silent while staying alive.

A pipe read cannot be given a deadline portably, so the reader moved to a thread feeding a channel and `exchange` now uses `recv_timeout`. **On expiry the child is killed rather than asked.** A child past its deadline is either wedged or inside a decode that cannot be interrupted, and both are the case `shutdown` already describes: asking politely and waiting is not a stop. Returning an error while leaving the process alive would trade a visible hang for an invisible leak — a resident model with nobody left to end it, which is the orphan the stdin pipe exists to prevent — so the test asserts the child is reaped, not merely that the call returned.

Numbers for the bound, measured rather than guessed. The two inference tests take **339 s on macOS** and **7.45 s on Windows** for identical work; the macOS log is hundreds of `ggml_metal_library_compile_pipeline` lines, so most of that is Metal pipeline compilation on a virtualised GPU rather than decode. 900 s is roughly 2.6× the slowest healthy observation, which is the right side of a bound that must never fire on a real ninety-minute meeting.

CI gets `timeout-minutes` too — 60 on the job, 45 on the `Tests` step — and that is worth keeping **despite** the false alarm rather than because of it. GitHub's default is six hours of silence, which was a reasonable default while this job only compiled and ran fast tests and stopped being one when it started loading a real model and supervising a child to do it. The step is tighter than the job because it holds the unbounded work and because failing there names the step instead of cancelling the job out from under it.
**Outcome:** applied
**Ref:** (pending)

## Q47 — m5-onboarding/09-m5-closeout — finding

**Question:** The clean-machine criterion needs a person. But a clean machine differs from this one in a way nothing had tested: a downloaded artifact carries `com.apple.quarantine`, and these builds are unsigned. What does an Operator's copy actually do?
**Options considered:** leave it for the clean-machine install / apply quarantine to the real CI artifact and run it
**Chosen:** Applied it and ran it. **The bundled Core is SIGKILLed by Gatekeeper — exit 137, no output, no diagnostic.** The standard right-click-Open flow clears it and everything then works, so the product is not broken; what was broken is what the Client says while it is.
**Decided-by:** agent
**Justification:** Every earlier check of this artifact extracted it with `unzip`, which does **not** set quarantine — so what had been verified was not what an Operator receives. That is the same shape as Q44 one layer out: a real artifact, checked in a way that skipped the property that matters.

The product-visible defect is in the Client. `child.on("error")` fires when a spawn *fails*; it does not fire when a spawn succeeds and the process is killed a moment later, so a quarantined install reported only "no Core is listening" — true, and useless, when the cause has a thirty-second fix. `classifyCoreExit` now distinguishes a signal death from a refusal, and macOS gets a message naming quarantine and the Finder gesture that clears it.

**And the fix's own premise was wrong until it was measured.** The first version claimed the Core exits 0 when another Core already holds the socket, and treated that as the case worth staying quiet about. It exits **1**, with `another EverTranscript Core is already listening` — so the code was silent about a case that does not occur and would have spoken about the ordinary one. It is harmless only because the caller reaches the message solely after every connection attempt has failed, which means the socket, not the exit code, is the authority. The comment now says that, because the next person to read it would otherwise inherit the same false belief. Three checked assumptions this session have gone the same way: `cargo test --workspace` not building the sidecar (Q45), `node --test <dir>` running non-test modules (Q49), and this.
**Outcome:** applied
**Ref:** (pending)

## Q48 — m1-walking-skeleton/06-live-asr-captions-channel — finding

**Question:** `transcription_quality.rs` carries `#![cfg(unix)]`, so Windows never compiles it. What does removing the gate actually buy?
**Options considered:** remove the gate / remove the gate and make CI fetch the model so the tests run
**Chosen:** Both, because the first alone buys nothing. **The gate is one of twelve, and the tests it hides were skipping on macOS too.**
**Decided-by:** agent
**Justification:** Removing the gate on its own would have added six tests to Windows that immediately skip, because nothing has ever set `EVERTRANSCRIPT_TEST_MODEL` in CI. On macOS they already report `6 passed ... finished in 0.00s` — six early returns wearing a pass. So the PRD's **top named unverified risk**, whisper.cpp quality on the Operator's actual languages, has been measured exactly once: by hand, in M1, on one machine. Every CI run since has reported `ok` for it.

The gate itself was stale rather than load-bearing. Nothing in the file is unix-specific — `#![cfg(unix)]` was its only reference to the platform, inherited from the sibling files that drive the Core over a socket, back when the only transport was a Unix one. `transport.rs:261` has spoken Windows named pipes since, and `lib.rs` picks per platform. Checked by compiling and running it here on Windows rather than by reading: six tests discovered, six skipped, green.

**Eleven files still carry it.** `auto_record` (8 tests), `protocol_contract` (9), `meeting_lifecycle` (7), `consent_gate` (6), `capture_vertical` (5), `live_captions` (5), `tray_control` (5), `fixture_audio_pipeline` (4), `caption_resilience` (3), `machine_isolation` (2), `script_preference` (2) — 56 more integration tests that have never compiled on Windows, against ADR-0025's "a milestone is not done until both pass". They are not fixed here because each drives the Core over a socket and needs its setup adapted, which is a different piece of work; naming the number is the point.

Three things fixed on the way. `test_model` had the exact `.ok()?` then `.exists().then_some()` spelling that `d89445c` had to correct for the Summary model — harmless while nothing set the variable and a green tick for six tests that loaded nothing the moment CI did. Corrected on the same day it became reachable rather than after it cost something, and **both halves were driven on Windows**: unset still skips green, set-but-missing fails all six at `transcription_quality.rs:48`.

Second, `--nocapture`. Cargo swallows a passing test's stdout, so without it this job would download 874 MB, transcribe four fixtures and print the WER into a buffer nobody reads. Worth noting what that already cost: `summary_inference.rs` prints `loaded: <model>` and the verbatim-reproduction count on every run, and **neither has ever appeared in a CI log** — grep the macOS job and they are simply not there.

Third, the model is verified by **crc32 and size, not sha256**, because `WHISPER_DEFAULT` carries `sha256: None` and `crc32: Some(3_055_274_469)`. Checking a number the registry does not hold would mean inventing a second source of truth for the same artifact.

**Not established: the numbers.** This makes the measurement run; it does not yet say what it reports. The M1 close-out recorded WER 2.5% on English and a bilingual CER measured on the tiny model, both by hand — whether the registered large-v3-turbo reproduces that on a runner, on either platform, is what the next green run will say. The crc32 helper is also the one piece not executed locally: no Python on this machine. It fails closed — an unusable interpreter is diagnosed and a missing one reddens the job rather than passing it.
**Outcome:** applied
**Ref:** (pending)

## Q49 — m5-onboarding/09-m5-closeout — finding

**Question:** Q44 changed how the Client finds the Core — the search that decides whether a fresh install works at all — and shipped it on reasoning. What actually ran that code?
**Options considered:** leave it to the clean-machine install / make it testable and test it
**Chosen:** Nothing ran it. **The Electron Client had no test runner and no test files**, so `coreBinary()` was typechecked and never executed. Extracted the search into `core-location.ts`, added `node --test` (built into the Node CI already uses, so no dependency), and wrote ten tests for it.
**Decided-by:** agent
**Justification:** CI's packaging guard proves the binary is *in* the artifact. It says nothing about whether the Client would *find* it — and confusing those two claims is precisely what Q44 was about, so repeating the confusion one layer up would have been the same mistake twice. The fix inverted a preference order on the strength of an argument about ADR-0016, which is exactly the kind of change that looks obviously right and is worth one execution before it reaches an Operator.

**The suite was checked against a mutant rather than trusted because it was green**: reverting the order so `PATH` wins again — the pre-Q44 behaviour — fails `the bundle's own Core beats one on PATH` and nothing else. A test that has never failed has not been shown to test anything, and this project has now twice shipped a check that passed vacuously (Q43's `EVERTRANSCRIPT_MODELS_DIR`, and a guarantee test whose name promised a Summary it never asked for).

Two details worth keeping. `node --test <dir>` executes every compiled module in it, and `index.js` calls into Electron at import time, so the runner is pointed at `*.test.js`. And tests compile to `dist-test/` rather than `dist/`, because electron-builder ships `dist/**/*` and would otherwise have packaged test code into the product — verified by building and finding zero `*.test.js` in the bundle rather than by assuming the glob.
**Numbering:** filed as Q46 and renumbered. Another session had already taken Q46 two commits earlier, and this was appended with the number read before that landed — an append-only journal is exactly where a duplicate identifier does damage, because every later reference to "Q46" becomes ambiguous. Renumbered here rather than in the other entry, which was first.
**Outcome:** applied
**Ref:** (pending)
