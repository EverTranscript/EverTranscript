# Diarization Error Rate on AMI

The measurement behind `DECISIONS.md` Q111, Q112, Q115 and Q116: the shipped
pipeline scored on the AMI Meeting Corpus under pyannote's published protocol,
so its numbers can be compared with pyannote's own (18.8% with the same two
models). The first harness lived in a scratchpad and was deleted with it;
this one is the repo's.

Everything lands in `$DER_DIR` (default `~/.cache/evertranscript-der`): BUT's
`AMI-diarization-setup` (Apache-2.0; the `only_words` references, UEMs and
split lists), the Mix-Headset WAVs (CC BY 4.0, about 100 MB a meeting), and
symlinks to the two ONNX models the app has already downloaded.

```sh
scripts/der/fetch.sh test            # or dev; 16 and 18 meetings
scripts/der/run.sh test hyp/shipped  # diarize every meeting, RTTM per meeting
scripts/der/score.sh test hyp/shipped
```

`score.sh` prints per-meeting and overall DER, missed speech, false alarm and
confusion, with no collar and overlapped speech scored.

**Separating turn placement from clustering.** `run.sh ... --dump` also
writes every observation the models made (one local speaker of one chunk:
channel, frames, vector) before clustering. `recluster.py` re-clusters those:
`oracle` labels each observation with the reference speaker who covers most
of it, which is perfect clustering on the turns the product would place, and
a number such as `0.55` runs a Python copy of `cluster::agglomerate` at that
merge threshold. A threshold is chosen on dev and reported on test.

```sh
scripts/der/run.sh dev hyp/dev --dump
uv run --python 3.12 --with pyannote.metrics --with numpy \
  scripts/der/recluster.py dev hyp/dev oracle 0.5 0.55 0.6 0.65
```

**Recognition across meetings**, for `MATCH_FLOOR` and `MATCH_MARGIN`:
`recognition.py` builds one voiceprint per person per meeting from clean
reference windows, then for each such probe scores it against one gallery
print per person built from that person's *other* meetings of the same
series. It reports the correct and impostor score distributions, the margin
between the right person and the nearest wrong one, and, for a grid of
floors, how many returning people a floor admits and how many impostor scores
it lets over.

```sh
uv run --python 3.12 --with onnxruntime --with numpy scripts/der/recognition.py test
uv run --python 3.12 --with onnxruntime --with numpy scripts/der/recognition.py dev IB4001,IB4002
```

IB4001 and IB4002's reference labels are swapped relative to the rest of
their series (every one of their speakers' nearest neighbour is a different
label, at scores above 0.8), so they are excluded from the dev recognition
figures; they still count in DER.

What the sample is not: Mix-Headset sums close-talking headsets, so it is
cleaner than a laptop microphone across a room; it is one in-room channel,
where a call splits the Operator from everyone else; and it is not this
product's own capture.
