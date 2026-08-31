# 01: The Backend seam and its fake

**What to build:** One trait through which the Core asks for generated text, with a fake that answers scripted responses — the M4 twin of AudioSource, DetectionSource and Diarizer. Every Knob, fallback, prompt-armor and map-reduce decision is tested through this seam; the sidecar (04) and the cloud client (05) implement it against the real things.

**Blocked by:** nothing.

Status: done

- [x] `Backend` trait: given a system prompt and a user prompt, produce text. Cancellable, because generation is minutes long and runs while the Operator is doing something else
- [x] `BackendIdentity` distinguishes three cases rather than two: the bundled sidecar, someone else's **local** runtime (Ollama, LM Studio), and cloud. "Local" is a claim about where the data went, and an Operator running Ollama is in a different situation from one running the bundled model even though neither leaves the machine. `leaves_the_machine()` is one method rather than a `match` per call site — one place to get the milestone's central question wrong instead of several
- [x] Errors are typed by **shape**, not by message: unreachable, refused (auth), timed out, malformed response, and cancelled. The fallback policy (07) switches on these, and a policy that matched on strings would break the first time a provider reworded an error
- [x] `FakeBackend` answers scripted responses, and can fail in each of those shapes on demand. **The fallback tests must be able to drive real failure shapes** — a fallback that only handles the failure its author imagined is the one that will not fire
- [x] The fake records the prompts it was given, so prompt-armor tests (03) can assert on what was actually sent rather than on what the code intended to send
- [x] The fake can be slow and can be cancelled mid-generation, so the cancellation path is exercised without waiting on a model
- [x] Exercised on both platforms in CI even though the real implementations land later (ADR-0025 as amended)
