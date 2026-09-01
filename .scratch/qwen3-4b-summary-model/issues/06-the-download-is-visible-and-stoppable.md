# 06: The download is visible and stoppable

**What to build:** An Operator can watch the model download and stop it. Today progress
exists only as something the Core logs, and the cancellation token cannot be reached from
a Client. That was tolerable when a fetch only happened because someone pressed a button
and waited on that screen; once it starts by itself, an unobservable multi-gigabyte
transfer is indistinguishable from the product misbehaving.

**Blocked by:** 05.

**Status:** ready-for-agent

- [ ] Download progress reaches Clients as it happens, the way Diarization's already does
- [ ] An Operator can cancel a download in progress, and a cancelled one resumes rather than restarting
- [ ] Both additions are additive to the protocol; a Client that predates them is unaffected, and regenerated bindings and schemas are committed
- [ ] The Client shows progress and offers the stop
- [ ] The local gate is green
