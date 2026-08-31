# 06: Reconciliation — attribution meets an already-published Transcript

**What to build:** The join between diarization turns and ASR words, on one clock, arriving after the Transcript already exists.

**Blocked by:** 01, 02.

Status: done

- [x] Each word or segment takes the Speaker of the turn containing its **midpoint** (catalog M3). Midpoint-in-turn is chosen over overlap-majority because it is stable under small boundary error, which is the error diarization actually makes
- [x] ASR words and diarization turns share the absolute capture clock (ADR-0029). If they do not, this ticket is unprovable and the clock is the bug
- [x] `reconcile::apply` writes onto rows that already exist, and `segments_after_update` dirties the Mirror for free. **Notification wiring is deferred to the ticket that starts the jobs** (03): there is no producer of a real Diarization yet, so a notification here would have no sender. The protocol carries `diarize/progress` and `speaker/changed` already (ticket 02). Original criterion: attribution re-maps an already-published Transcript: diarization is post-meeting, so segments exist, Clients have seen them, and Mirrors are written. Re-mapping emits protocol notifications so an attached Client updates rather than showing a stale attribution until reload
- [x] **Boundary flips are counted and kept as a quality metric**, not hidden. The number of words whose attribution changed at a turn boundary is the honest measure of how much the join is guessing
- [x] The join generalizes, and the second source already exists: correction hints are applied by the same read path rather than a parallel one, in `meetings::segments`. Original criterion: generalizes to any attribution source as interval overlap on one clock (catalog M3), so a future source — a correction hint, a calendar attendee, a second diarizer — reuses it rather than growing a parallel path
- [x] A Meeting whose diarization is interrupted leaves a coherent record: partially attributed is acceptable, half-written is not. The crash suite covers kill-mid-diarization and confirms the Meeting recovers with whatever attribution completed
- [x] Re-running diarization over a Meeting replaces the machine's attribution and **preserves every correction hint** (ADR-0009): the hints are the Operator's, not the machine's, and re-diarization is exactly the moment they must survive
