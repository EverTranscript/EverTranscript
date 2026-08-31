# 03: What it knows, what it holds, what it may say

**What to build:** Stories 46 and 47 — the guarantees made checkable in the product rather than asserted in a README.

**Blocked by:** nothing.

Status: built

- [x] **The data model and its tests exist (`posture::Holdings`); no screen renders it yet.** The facts are computable; showing them is the part still owed. Original criterion: with an enumeration: every OS grant held, every model on disk, the History folder's location and size, how many Speakers and Voiceprints are stored, and whether the calendar has been granted
- [x] A surface answering **"what may it say on the wire?"** listing Sanctioned Traffic's three entries by name (ADR-0034), which are enabled, and that with updates off and models downloaded the answer is *nothing*
- [x] **Read from the same facts the guarantee tests assert against**, not from a hand-maintained list. A list that can drift from the binary is a claim, and this milestone is about the difference
- [x] State plainly what is foreclosed (ADR-0020 as amended): no filesystem indexing, no contacts, no screen content — and the two reversals that ADR has actually taken, because a guarantee page that omits its own amendments is the thing an evaluator will find first
- [x] Point at the source (ADR-0033) and, on macOS, at the entitlements — verifiable rather than promised
- [x] Not reachable, because there is nothing to reach. Original criterion:
- [x] No strings yet. Original criterion:
