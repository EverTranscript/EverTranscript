"""DER of a directory of hypothesis RTTMs against the BUT AMI references.

usage: scripts/der/score.sh <split: test|dev> <hyp_dir under $DER_DIR> [meeting ...]
Protocol: only_words reference, evaluation restricted to the split's UEM, collar 0, overlap scored
(pyannote's published protocol, DECISIONS Q111).
"""
import os, sys, pathlib
from pyannote.core import Annotation, Segment, Timeline
from pyannote.metrics.diarization import DiarizationErrorRate

DER_DIR = pathlib.Path(os.environ.get("DER_DIR", pathlib.Path.home() / ".cache/evertranscript-der"))
split, hyp_dir = sys.argv[1], DER_DIR / sys.argv[2]
setup = DER_DIR / "setup"
meetings = sys.argv[3:] or setup.joinpath(f"lists/{split}.meetings.txt").read_text().split()

def load_rttm(path):
    ann = Annotation()
    for line in pathlib.Path(path).read_text().splitlines():
        f = line.split()
        if len(f) < 8 or f[0] != "SPEAKER":
            continue
        start, dur = float(f[3]), float(f[4])
        ann[Segment(start, start + dur), f"{f[1]}-{f[7]}-{start}"] = f[7]
    return ann

def load_uem(path):
    tl = Timeline()
    for line in pathlib.Path(path).read_text().splitlines():
        f = line.split()
        tl.add(Segment(float(f[2]), float(f[3])))
    return tl

metric = DiarizationErrorRate(collar=0.0, skip_overlap=False)
rows = []
for m in meetings:
    hyp_path = hyp_dir / f"{m}.rttm"
    if not hyp_path.exists():
        print(f"{m:8} missing"); continue
    ref = load_rttm(setup / f"only_words/rttms/{split}/{m}.rttm")
    hyp = load_rttm(hyp_path)
    uem = load_uem(setup / f"uems/{split}/{m}.uem")
    d = metric(ref, hyp, uem=uem, detailed=True)
    total = d["total"]
    rows.append((m, d))
    print(f"{m:8} DER {100*d['diarization error rate']:5.1f}  miss {100*d['missed detection']/total:5.1f}  fa {100*d['false alarm']/total:5.1f}  conf {100*d['confusion']/total:5.1f}  ref {total/60:5.1f} min  hyp spk {len(hyp.labels())}")
if rows:
    agg = abs(metric)
    comp = metric[:]
    total = comp["total"]
    print(f"{'ALL':8} DER {100*agg:5.1f}  miss {100*comp['missed detection']/total:5.1f}  fa {100*comp['false alarm']/total:5.1f}  conf {100*comp['confusion']/total:5.1f}  ref {total/3600:5.1f} h  ({len(rows)} meetings)")
