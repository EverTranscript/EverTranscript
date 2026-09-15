# 08: Diarization runs one persistent queue

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The replacement for reject-don't-queue, landing before the thing that needs
it and fixing a defect that exists today.

M3's policy refuses an overlapping job rather than queueing it, which was right when the only
producer was a meeting ending. A model change re-runs all of History, and under that policy
every Meeting that ended during the re-run would be silently dropped.

The current implementation is also wrong in a way worth fixing regardless. The refusal happens
inside the spawned task and is only logged, so the caller is told the run started. Worse, the
job-status entry is written before the spawn and cleared after it returns, so a second,
refused run overwrites the running job's entry and then clears it — the running job becomes
invisible to status and uncancellable.

Afterwards: one persistent queue. A just-ended Meeting or an Operator request enters at the
front, bulk work sits at the back, and only a Meeting already in line is refused. It survives
a restart, and a refusal is something the caller is told.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] A Meeting that ends during a long run is processed rather than dropped, and goes ahead of queued bulk work
- [ ] A Meeting already in line is refused, and the caller is told rather than the refusal being logged
- [ ] Status reports the running job and the queue; the running job stays visible and cancellable when a second request arrives
- [ ] The queue survives a Core restart
- [ ] The existing slot guard keeps one run at a time and still releases on panic
- [ ] `diarize/status` gains no breaking change (ADR-0028 additive-only)
