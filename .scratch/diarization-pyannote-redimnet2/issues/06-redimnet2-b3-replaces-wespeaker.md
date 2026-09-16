# 06: ReDimNet2-B3 replaces WeSpeaker, and the DER bar is met

**Parent:** `docs/prd.md` (Speakers & diarization, stories 33b-33i) and ADR-0037.

**What to build:** The swap, and with it the first of the two bars ADR-0037 sets.

One embedding does both jobs — clustering and Voiceprints — where Granola splits them across
a masked WeSpeaker export and ReDimNet2-B3. The masked half needs an ONNX carrying a
speaker-mask input that nobody publishes, so either design ships an export of our own, and
this one ships one instead of two.

The model is already exported and published: MIT, 192-d, `waveform [batch, samples]` float32
at 16 kHz in and `embedding [batch, 192]` out, with the mel frontend inside the graph, so the
Rust fbank path is dead for this model. It has been run through `ort` on macOS arm64 and
Windows x86_64 at cosine 1.000000 against PyTorch, agreeing to six decimals, and the export
script is committed and pinned. What remains is the registry entry with its URL and checksum,
the waveform contract in place of the mel contract, and the measurement.

The migration from 05 fires on upgrade because the registry's model identity changed. Merge
threshold starts at the bake-off's measured 0.60 for this model; the recognition thresholds
are the next ticket's subject.

**Blocked by:** 03, 05.

**Status:** ready-for-agent

- [ ] **DER on AMI test is at or under 18.8%**, reported with its missed, false-alarm and confusion split — the merge does not land otherwise
- [x] The registry entry carries URL, size, sha256, licence and source, and the download is checksum-verified like every other Provisioned Model
- [x] Inference runs on both platforms in CI, skipping loudly rather than quietly without the model
- [x] Upgrading a History built by the old model leaves it migrated, with the record intact and recognition restarting cleanly
- [x] The zero-network guarantee passes with the new model present and executing
- [x] Clustering time per meeting does not regress against 02

## What changed

The registry entry is the swap: `redimnet2-b3-vox2-lm`, 18,045,013 bytes, sha256
`dcecdce7…`, MIT, 192-d. `DIARIZE_EMBEDDING_DIM` sits beside it so the width travels with
the checksum rather than as a literal at the assertion site — the old literal 256 is what
caught this model change, and a width mismatch is this pipeline's quietest failure, since
vectors of two widths simply never match and nothing reports an error.

`embed` takes raw waveform now. `diarize/fbank.rs` is gone — 373 lines of hand-written FFT
and mel filterbank — because the mel frontend is inside the graph. That module was the one
place where our implementation of a feature convention and the model's had to agree exactly,
and where a disagreement produced plausible vectors that quietly stopped matching. Its one
surviving item, `SAMPLE_RATE`, moved to `diarize/mod.rs` (DECISIONS Q130).

CI fetches the new URL, verifies the new checksum, and carries a new cache key.
`MERGE_THRESHOLD` stays 0.60 with its doc rewritten to say that is this model's measured
value and that matching the old one is a coincidence — an unchanged constant across a model
swap is exactly what reads as a carry-over somebody forgot (Q131).

## Measured

The zero-network guarantee **runs** rather than skipping: all ten pass with every required
model staged, including the 2.5 GB Summary model, because a Core provisioning in the
background is a Core with sockets open and that is what the test would otherwise catch and
blame on Diarization.

`recognition_restarts_on_the_new_model_after_the_upgrade` is the upgrade end to end over a
file-backed History: an old-model Voiceprint is wiped, offered to neither model as a seed,
Alice keeps her name and her confirmation, the first Meeting after the upgrade mints rather
than recognising, and the second recognises the first. Ticket 05 proved the record survives;
this proves the product still works afterwards, which is the other half of the same claim.

Clustering is unregressed: `a_meetings_worth_of_windows_clusters_in_seconds_rather_than_minutes`
holds, and the corpus run's cost is inference, not clustering — about 12,000 ONNX forwards
per meeting at the one-second step.

Windows was not visited for this ticket. Nothing here is platform-specific, the criterion
names CI, and the export was already checked through `ort` on Windows x86_64 at cosine
1.000000 against PyTorch (DECISIONS Q121).

### DER on AMI test: 20.00%, against a bar of 18.8%

The corpus was fetched and all sixteen test meetings measured.

```
16 meetings, 30714.0s of reference speech
DER          20.00%   missed 11.42  false alarm 3.86  confusion 4.72
oracle floor 19.19%
```

| | EN2002a | EN2002b | EN2002c | EN2002d | ES2004a | ES2004b | ES2004c | ES2004d |
|---|---|---|---|---|---|---|---|---|
| DER | 26.50 | 26.61 | 22.82 | 28.80 | 21.96 | 13.60 | 13.98 | 22.14 |
| oracle | 26.60 | 24.70 | 22.92 | 29.31 | 20.37 | 13.20 | 14.00 | 21.08 |

| | IS1009a | IS1009b | IS1009c | IS1009d | TS3003a | TS3003b | TS3003c | TS3003d |
|---|---|---|---|---|---|---|---|---|
| DER | 22.38 | 14.40 | 11.77 | 19.32 | 18.77 | 14.40 | 14.31 | 24.43 |
| oracle | 19.73 | 13.03 | 10.23 | 17.73 | 15.89 | 11.82 | 13.83 | 22.34 |

**The bar is not met, and it is not the embedding's to meet.** The oracle floor —
what this pipeline would score if clustering were perfect — is 19.19%, already above 18.8%
on its own. Clustering costs 0.81 points of the 20.00%. A better embedding can win at most
those 0.81 points, so no model swap reaches 18.8% while turn placement stays as it is.

Where the remaining error is: 11.42 points of the 20.00% is missed speech, reference speech
this pipeline hands to nobody. 27.0% of AMI's reference speaker-time is overlapped (two or
more people at once, counted separately, which is the protocol pyannote publishes under), so
the pipeline is recovering roughly half of the overlap and losing the rest. The two duration
floors are not the cause and were checked: `MIN_SPAN_MS` gates only which stretch is clipped
for playback, not whether a turn is emitted, and `MIN_SPEAKER_MS` gates minting a Speaker in
the record, which the harness does not go through. Reference speech in turns shorter than
`MIN_EMBED_MS` is 0.3% of the corpus, so that floor is not it either.

For scale, M3 measured 49.7% with a 32.6% oracle floor. This branch is at 20.00% with a
19.19% floor — the pipeline rebuild plus this model took two thirds of the error out. The
last 1.2 points to 18.8% are turn placement, which is ticket 03's subject and not this one's.

**Escalated rather than decided** (DECISIONS Q135): the ticket says the merge does not land
without the number, and the number says the bar cannot be reached here. Whether to land the
swap anyway and reopen turn placement is not a call this ticket can make.

## Still owed

- Cross-meeting EER is not measured. It needs every meeting's voiceprints in one process, and
  the corpus was measured one meeting per process after the sixteen-meeting run was killed
  three times by the machine's low-memory guard. It is ticket 07's bar, and ticket 07 needs a
  single-process run or a harness that writes embeddings to disk for a second pass.
- The pooled figure above is pooled by hand from the per-meeting lines, weighted by each
  meeting's reference speech, which is how the harness pools and how pyannote's 18.8% is
  pooled. Fourteen meetings contribute two-decimal rates and EN2002a only one, so the total
  carries about 0.01 of rounding. The reference total, 30714.0s, matches an independent sum
  over the RTTMs exactly.
- Wall clock is 0.13x-0.21x of real time across the corpus, roughly 5x-7x faster than real
  time. Ticket 03 projected "roughly six minutes" for an hour-long meeting from a 17.6s clip;
  the corpus says nearer ten. Clustering is not the cost — it is about 12,000 ONNX forwards
  per meeting at the one-second step.
