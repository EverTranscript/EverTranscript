# 01: The Backend seam and its fake

**What to build:** One trait through which the Core asks for generated text, with a fake that answers scripted responses — the M4 twin of AudioSource, DetectionSource and Diarizer. Every Knob, fallback, prompt-armor and map-reduce decision is tested through this seam; the sidecar (04) and the cloud client (05) implement it against the real things.

**Blocked by:** nothing.

Status: not started

- [ ] `Backend` trait: given a system prompt and a user prompt, produce text. Cancellable, because generation is minutes long and runs while the Operator is doing something else
- [ ] Backends describe themselves — enough for the active-Backend indicator (story 38) to say *which* one is running, not merely local-or-cloud
- [ ] Errors are typed by **shape**, not by message: unreachable, refused (auth), timed out, malformed response, and cancelled. The fallback policy (07) switches on these, and a policy that matched on strings would break the first time a provider reworded an error
- [ ] `FakeBackend` answers scripted responses, and can fail in each of those shapes on demand. **The fallback tests must be able to drive real failure shapes** — a fallback that only handles the failure its author imagined is the one that will not fire
- [ ] The fake records the prompts it was given, so prompt-armor tests (03) can assert on what was actually sent rather than on what the code intended to send
- [ ] The fake can be slow and can be cancelled mid-generation, so the cancellation path is exercised without waiting on a model
- [ ] Exercised on both platforms in CI even though the real implementations land later (ADR-0025 as amended)
