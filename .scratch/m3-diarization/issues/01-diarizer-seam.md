# 01: The Diarizer seam and its turn fixtures

**What to build:** One trait through which the Core turns a finished Meeting's audio into speaker turns, with a fixture implementation that replays scripted timelines — the M3 twin of AudioSource and DetectionSource. Every attribution, persistence and naming decision in this milestone is tested through this seam; the ONNX pipeline (03, 04) implements it against real audio.

**Blocked by:** nothing.

Status: done

- [x] `Diarizer` trait taking a finished Meeting's two channels and producing **turns** — `(channel, start_ms, end_ms, cluster)` — plus one embedding per cluster. No Speaker identity at this level: the seam produces clusters, and turning a cluster into a persistent Speaker is policy above it (04)
- [x] Turn times are on the **absolute capture clock** ADR-0029 already mandates, the same one ASR words carry, because reconciliation (06) is interval overlap on one clock and any second clock makes it unprovable
- [x] `FixtureDiarizer` replays a scripted turn timeline. **Deviation, and it makes the criterion stronger rather than weaker:** `FixtureSource` needs a oneshot completion channel because capture produces events forever and a test has to know when the script ran out. Diarization has an answer and then stops, so `diarize` returns the `Diarization` and there is nothing to wait for — the no-sleep property this asked for is structural here instead of arranged
- [x] **The fixture can produce ugly timelines, not only tidy ones.** M1's chunker was correct against whole-file fixtures and discarded every sample from a live microphone; M2's detection fixtures hid nothing only because the policy was driven with fragments too. Required shapes: turns shorter than the embedding window, overlapped speech, a channel with exactly one speaker, and a meeting where one speaker never speaks twice in a row
- [x] The scripted timelines this milestone needs, as reusable constants: a clean two-speaker conversation; a solo meeting; a shared room (two distinct voices on the **mic** channel); a returning speaker from a previous Meeting; overlapped speech; a meeting containing one turn under 0.5 s
- [x] Diarization is **cancellable** and reports progress, because it runs post-meeting over a whole recording and a Client attaching mid-job must be able to see and stop it
- [x] The seam is exercised on both platforms in CI even though the ONNX implementation lands later, so the contract cannot drift per-platform (ADR-0025 as amended). Nothing in `diarize` is `cfg`-gated, so the 14 tests run on `macos-14` and `windows-latest` alike
