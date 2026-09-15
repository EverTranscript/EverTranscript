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
- [ ] The registry entry carries URL, size, sha256, licence and source, and the download is checksum-verified like every other Provisioned Model
- [ ] Inference runs on both platforms in CI, skipping loudly rather than quietly without the model
- [ ] Upgrading a History built by the old model leaves it migrated, with the record intact and recognition restarting cleanly
- [ ] The zero-network guarantee passes with the new model present and executing
- [ ] Clustering time per meeting does not regress against 02
