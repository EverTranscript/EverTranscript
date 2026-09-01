# 06: The download is visible and stoppable

**What to build:** An Operator can watch the model download and stop it. Today progress
exists only as something the Core logs, and the cancellation token cannot be reached from
a Client. That was tolerable when a fetch only happened because someone pressed a button
and waited on that screen; once it starts by itself, an unobservable multi-gigabyte
transfer is indistinguishable from the product misbehaving.

**Blocked by:** 05.

**Status:** done

- [x] `models/progress`, mirroring `diarize/progress` — sent on the Core's notification channel rather than only logged, because an Operator cannot read the Core's log
- [x] `models/cancel`. The token is held by the Core rather than made per-call, because the Client that wants to stop a fetch is not the one that started it — on a fresh install nobody did, the binary did. Partial files stay, so asking again resumes
- [x] Additive: 112 lines added across schema and bindings, none removed. Regenerated and committed
- [x] Shown in onboarding's Models step, silent when nothing is downloading. A stopped fetch clears rather than freezing — a bar that stops moving reads as a stall, which is the one thing it is not
- [x] The local gate is green
