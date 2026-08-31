# 01: The Diarizer seam and its turn fixtures

**What to build:** One trait through which the Core turns a finished Meeting's audio into speaker turns, with a fixture implementation that replays scripted timelines — the M3 twin of AudioSource and DetectionSource. Every attribution, persistence and naming decision in this milestone is tested through this seam; the ONNX pipeline (03, 04) implements it against real audio.

**Blocked by:** nothing.

Status: not started

- [ ] `Diarizer` trait taking a finished Meeting's two channels and producing **turns** — `(channel, start_ms, end_ms, cluster)` — plus one embedding per cluster. No Speaker identity at this level: the seam produces clusters, and turning a cluster into a persistent Speaker is policy above it (04)
- [ ] Turn times are on the **absolute capture clock** ADR-0029 already mandates, the same one ASR words carry, because reconciliation (06) is interval overlap on one clock and any second clock makes it unprovable
- [ ] `FixtureDiarizer` replays a scripted turn timeline with the same completion signal `FixtureSource` uses, so tests never sleep-and-hope
- [ ] **The fixture can produce ugly timelines, not only tidy ones.** M1's chunker was correct against whole-file fixtures and discarded every sample from a live microphone; M2's detection fixtures hid nothing only because the policy was driven with fragments too. Required shapes: turns shorter than the embedding window, overlapped speech, a channel with exactly one speaker, and a meeting where one speaker never speaks twice in a row
- [ ] The scripted timelines this milestone needs, as reusable constants: a clean two-speaker conversation; a solo meeting; a shared room (two distinct voices on the **mic** channel); a returning speaker from a previous Meeting; overlapped speech; a meeting containing one turn under 0.5 s
- [ ] Diarization is **cancellable** and reports progress, because it runs post-meeting over a whole recording and a Client attaching mid-job must be able to see and stop it
- [ ] The seam is exercised on both platforms in CI even though the ONNX implementation lands later, so the contract cannot drift per-platform (ADR-0025 as amended)
