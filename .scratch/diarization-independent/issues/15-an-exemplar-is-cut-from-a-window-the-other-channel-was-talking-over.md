# 15: An exemplar is cut from a window the other channel was talking over

Raised 2026-09-17 out of ticket 13's second failed gate (Q256). Tickets 13 and
14 both read the mint: 13 asks whether the exemplars a Speaker holds agree, 14
asks which of them the cap spends itself on. This ticket is upstream of both —
it asks whether an exemplar should have been written at all.

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b–33i) and
ADR-0037.

**Blocked by:** nothing. **No threshold and no corpus.** The rule is
categorical — any overlap drops the window — and the evidence it needs is
already in the record, because the product transcribes both channels.

**Status:** **built, mic-only, and measured on a rebuilt copy of the real
History** — Q257 specified it, Q258 escalated the direction and the advisor,
for the user, ruled mic-only, `ea13e2b` built it, Q260 corrected a premise, Q261 measured the
rebuild. All six acceptance criteria are met. **Adopted, and exercised on the real
History 2026-09-18 (Q288).** The sign-and-swap put `6579ed0` on
`macbook-pro-nickel` and the bulk re-run walked all 12 Meetings; every stop
condition passed and nothing was restored. The rule held on the real record —
reseed-shaped mic rows overlapping the far end went **27 of 27 → 0 of 133** —
and **265 segments lost an owner, every one of them overlapped, none clean**.
The exchange-rate framing above is superseded: the rate was assumed at 4:1 by
Q231 and the DER bar closed by Q232 (see Q287).

## Why this is the ticket, and why it comes from 13's failure

Ticket 13 tried three measures of "these exemplars disagree" and each read a
symptom at the mint. The third — the two-way partition test — passed AMI's
**strict** pools 16 of 16 and refused **10 of 16** permissive ones (Q256).
That split is the finding: the partition test is sound, and what it detects is
not a defect of the mint but of what the mint was given. Strict pools are built
from windows where the filed speaker held the audio; permissive pools include
windows where somebody else was talking. One passes, the other does not.

`reseed::plan` builds one range per `transcript_segments` row — the whole
segment, one channel, no clean-runs filter and no overlap exclusion
(`reseed.rs:189`). So production writes permissive pools. Q246 had already
localised it: of the Operator's contaminated group, 5 of 5 windows overlapped
far-end speech against 1 of 4 in the clean group.

**The purity signal AMI-strict approximates is available here without a
corpus.** AMI needs an RTTM to know who held a window; the product has both
channels' transcript segments, so "was the other channel speaking across this
window" is a query, not an inference.

## What to build

Drop an exemplar window when the other channel carries voiced speech
intersecting it. Any intersection at all — no threshold, no proportion.

**The two write paths are not symmetric, and only one of them selects windows
this way.**

- **The reseed path** (`reseed.rs:389`) writes one exemplar per attributed
  range, and the vector *is* the embedding of that one window. The rule applies
  directly and the record already holds what it needs:
  `speaker_exemplars.sample_channel` / `sample_start_ms` / `sample_end_ms`
  against `transcript_segments` on the opposite channel of the same Meeting.

- **The machine path** (`cluster.rs:1123`) does **not** select windows the same
  way, so the rule cannot be applied to its exemplar. Its vector is the
  cluster's centroid over every observation of that voice in the run, and its
  `sample` column is a **playback pointer** — `live.rs:858` sets it from the
  middle of the longest *clean* run, chosen for listening rather than for
  embedding. Dropping that exemplar because its playback pointer overlaps would
  discard a whole run's evidence on the strength of a window that did not
  produce the vector. For this path the filter belongs one level up, on the
  observations before `cluster::centroid` consumes them (`live.rs:865`), where
  each `Observation` carries its own `channel` and `runs` on the capture clock.
  Note that this path already computes same-channel exclusivity as
  `clean_runs` — "the stretches it was the only voice" — but feeds the
  unfiltered `observation.vector` to the centroid regardless, and `clean_runs`
  is same-channel only, so it says nothing about the far end.

**Correction, Q260 — that claim was wrong, and it was this ticket's own
warning.** This section first read: *"no exemplar in the real History is
machine-shaped today (every usable row carries a sample window, and the counts
match the reseed writes exactly), so the machine path is unmeasured here."* The
census says otherwise. Partitioning by the writer's own tell — a reseed row's
vector **is** its window, so `voiced_ms == sample_end_ms - sample_start_ms`,
while a machine row's vector is the cluster centroid against a 10s pointer, so
the two disagree — the 1472 rows are:

| shape | named / Operator | pseudonym |
|---|---|---|
| reseed (vector is the window) | 1407 | 1 |
| **machine (centroid + pointer)** | **14**, over 8 Meetings | 50 |

The 14 are the Operator's **nine**, all on mic, plus five on system for `Jack
Ahn` ×2, `Hong Li`, `Marc Ammann` and `Ming Chen`. Five were written by the two
Meetings recorded after Q235's install; the rest by live diarization after the
04:58 bulk re-run, which reseeds the relearnable Speakers and leaves the newly
recognised ones to the machine path. The mistake was reading `named` as a
synonym for `reseed-shaped`; the bulk re-run makes that true of most rows, not
all.

## What the measurement found

Read-only, `mode=ro`, on `macbook-pro-nickel`'s real History, 2026-09-17, with
the post-ticket-14 selection in place. Nothing was written and nothing was
built first, as instructed.

**Port check.** The recomputed centroid reproduces the **stored** Voiceprint at
`1.00000` for every Speaker at or under the cap and for both single-Meeting
Speakers over it. The four that differ — Hong Li 0.94280, Ming Chen (230)
0.98900, Marc Ammann 0.99638, Jack Ahn 0.99944 — are exactly the multi-Meeting
over-cap rows whose Voiceprints were minted before ticket 14 shipped, when the
cap took a contiguous tail. That is ticket 14's effect size measured a second
way, not a port error.

### The Operator: the rule mints the right voice rather than refusing

All nine of the Operator's usable exemplars are on the **mic** channel. Six
overlap system-channel speech and are dropped; three survive.

**But all nine are machine-shaped (Q260), so this measured a different rule
than the one specified.** The arithmetic below is sound — those are the nine
stored vectors, and the centroid of the three kept really does reach 0.9811.
What is not sound is calling the selector an overlapping *window*: a machine
row's sample is a playback pointer from the middle of the longest same-channel
clean run, and it did not produce the vector. So the six were dropped for where
their pointer landed, which makes this a coarse per-Meeting selector — drop the
whole mic cluster of a Meeting the far end talked through — and not the
per-window rule the ticket specifies. That it works at all is explainable: a
pointer drawn from a clean run in a Meeting full of far-end speech is itself
likely to sit in far-end speech, so it proxies for "this cluster is
contaminated". A proxy, not the rule.

The shipped machine-path filter is the finer form of the same idea: it drops
the contaminated **observations** before `centroid` consumes them, so a
contaminated Meeting loses its bad windows instead of its whole cluster.
Nothing in the stored record can show that, because the record keeps only the
centroid — which makes the rebuild the first real measurement of it, and the
Operator's after-vs-before the place to read it.

| the Operator's centroid | vs `d859b1`, their own clean voice | vs Ming Chen `89fbf5` | vs Ming Chen `7f584e` |
|---|---|---|---|
| today, all 9 usable | 0.6183 | 0.3148 | 0.2773 |
| **after the drop, 3 kept** | **0.9811** | 0.3283 | 0.3164 |

This is the result ticket 13 could not reach by any measure it tried. Ticket 13
could at best **refuse** a contaminated record; the drop **repairs** it — the
surviving three agree with the Operator's own clean voice at 0.9811, where the
blend of all nine sat at 0.6183, and the far-end voice stays as far away as it
was. The two centroids agree with each other at only 0.6227, so this is a
different vector, not a rounding of the same one.

### The six named controls keep their identity; one is stranded

| Speaker | mic/system | usable | survive | kept | after vs today's Voiceprint |
|---|---|---|---|---|---|
| Hong Li | 0/153 | 153 | 48 | 31% | 0.9443 |
| Jack Ahn | 24/866 | 890 | 160 | 18% | 0.9713 |
| Marc Ammann | 1/58 | 59 | 29 | 49% | 0.9827 |
| **Menggang Xu** | 1/0 | 1 | **0** | **0%** | **stranded** |
| Ming Chen | 0/79 | 79 | 36 | 46% | 0.9701 |
| Ming Chen | 0/230 | 230 | 70 | 30% | 0.8654 |

Five of six keep their voice: 0.8654–0.9827 against the Voiceprint they carry
today, so the rule costs them evidence without moving their identity.

**`Menggang Xu` is stranded and the rule is not softened for them.** They hold
exactly one exemplar, on mic, and it is overlapped — so every usable row goes
and `refresh_voiceprint` falls through to `clear_voiceprint`. Reported rather
than accommodated, as instructed. What bounds the harm: clearing does **not**
set the forgotten mark, so the name and the identity survive and only
recognition is lost until that voice is heard again.

Across every Speaker: **1472 usable exemplars, 1115 dropped (76%), 15 Speakers
left with none** — `Menggang Xu` and fourteen pseudonyms.

### The channel asymmetry, reported and not acted on

The specified rule is symmetric, and the two channels are not. The mic
recording can contain far-end speech, because the speakers leak into the
microphone — that is the path Q246 measured. The system recording is a tap of
the output stream, so the Operator's own voice has no route into it.

Measuring the one-directional variant — drop a **mic** window overlapped by
system speech, keep system windows — gives:

| Speaker | symmetric keeps | one-directional keeps | one-directional vs today's Voiceprint |
|---|---|---|---|
| Frank Dai (Operator) | 3 of 9 | **3 of 9** | 0.9811 vs own clean voice — **identical** |
| Hong Li | 48 | 153 | 0.9428 |
| Jack Ahn | 160 | 866 | 0.9994 |
| Marc Ammann | 29 | 58 | 0.9960 |
| Menggang Xu | 0 | **0** | still stranded |
| Ming Chen | 36 | 79 | 1.0000 |
| Ming Chen | 70 | 230 | 0.9890 |

The Operator's repair is **bit-for-bit the same** under both rules, because
every one of their exemplars is on mic. The controls are 1372 of 1412
system-channel, so the symmetric rule's extra 74 points of drop rate falls
almost entirely on windows with no inbound leak path. The stranding is not
caused by the symmetric direction either — `Menggang Xu`'s one window is on
mic.

**This is reported, not applied.** It is a scoping question about which
capture paths can carry contamination, not a threshold to be tuned, and it is
the user's call in the same way `MATCH_FLOOR` is. Q258 carries it.

## Acceptance criteria

- [x] An exemplar window that intersects opposite-channel voiced speech is not
      enrolled, on the reseed path, with no threshold — asserted through the
      write path rather than on the query.
      `a_mic_range_the_far_end_talked_over_is_not_enrolled`, plus the rebuild:
      reseed-shaped mic rows overlapping the far end go **27 of 27 → 0 of 133**
- [x] The machine path's observations are filtered before the cluster centroid
      consumes them, not its exemplar afterwards — and the distinction is
      pinned by a test, so a later reader does not "simplify" it into an
      exemplar-level check.
      `a_mic_observation_the_far_end_talked_over_does_not_reach_the_centroid`
      asserts the surviving vector, and Q260 is why the distinction matters
- [x] A Speaker left with no usable exemplar falls through to
      `clear_voiceprint` and keeps its name and its unforgotten mark.
      `a_speaker_talked_over_throughout_loses_the_vector_but_not_her_name`, and
      on the rebuild `Menggang Xu` ends with no vector and `forgotten = 0`
- [x] The Operator's real record mints a Voiceprint agreeing with their own
      clean voice rather than a blend — the 0.9811 above, re-measured after the
      build. The rebuild reaches **0.9960** against `d859b1` from **139**
      exemplars where there was **no Voiceprint at all** before, with the
      far-end `Ming Chen` rows at 0.3365 and 0.3110, and attribution rising
      from 483 segments over 6 Meetings to 591 over all 12 (Q261)
- [x] The six named controls still mint, and the one that cannot
      (`Menggang Xu`) is recorded as expected rather than worked around.
      Precisely: **five of the six** mint and keep their identity — Hong Li
      0.9536, Jack Ahn 0.9673, Marc Ammann 0.9953, `Ming Chen` 1.0000,
      `Ming Chen` 0.9740 — and the sixth is `Menggang Xu`, stranded. Named
      Speakers holding a vector is 6 before and 6 after; the membership trades
      `Menggang Xu`'s recognition for the Operator's
- [x] Whether the rule is symmetric or mic-only is settled before the drop rate
      is treated as acceptable. Q258 escalated it; the advisor, for the user, ruled **mic-only**,
      carried by **Q265**, which supersedes Q258 so the choice is recorded in
      one place rather than read as two answers

## What this does not claim

It does not replace ticket 13. A record can hold two voices without either
window overlapping the other channel — two people on the same far-end call,
recorded on one system channel, is the case the drop rule cannot see.

**Its mirror is the in-room second voice, and it bears on Q266.** The rule
reads a mic window as suspect only while the system channel is speaking. A
person sitting in the room with the Operator speaks in the far end's silences,
so their windows survive and land in the Operator's Voiceprint. This is why
ticket 15 narrows Q245's escalation to a one-window confirmation rather than
removing it: the surviving group is the one that spoke when the far end was
silent, which is the Operator *if nobody else was in the room* — and nothing in
the record establishes that.

It does not claim the cost is affordable. The 76% above was the **symmetric**
rule's projection and is superseded by the mic-only ruling; measured on the
rebuild, mic-only does not shrink the corpus at all — exemplars go 1472 →
**1776** — because the reseed path enrols filtered windows for Speakers that
previously had cluster centroids or nothing. What it costs instead is
**pseudonym recognition**, and Q263 measured exactly what that means. **265**
segments lose their owner (the net was 264 because one gained), and every one
of them is **on the mic channel with the system channel talking over it** —
2963 s of double-talk. **No clean segment lost an owner: zero.** All 265
belonged to pseudonyms; **not one belonged to a named Speaker or the
Operator.** Those turns keep their text and lose a numbered label, not a name.

Pseudonyms holding a vector fall 20 → 11, but that is a **net**: **13 lost**
their vector and 4 were newly minted. Eleven of the 13 held a single exemplar
in a single Meeting, and none of the 13 matched any known voice above
`MATCH_FLOOR` — the best was 0.5370. They were fragments that could never have
been recognised, not people. Ten are mic-only and are the rule's doing; three
are system-channel, which mic-only cannot touch, so those are re-run cluster
churn rather than the filter.

A bulk re-run also re-derives attribution, so 265 remains an upper bound on the
filter's share. Whether that buys the Operator's Voiceprint is the exchange
rate, and it is open on purpose — though Q263 narrows it considerably: the
currency being spent is double-talk fragments, not speech.
