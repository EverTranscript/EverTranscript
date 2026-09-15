"""Re-cluster dumped observations and score: an oracle (each observation labelled by the reference
speaker covering most of it) and a merge-threshold sweep with a Python copy of cluster::agglomerate.

usage: uv run --python 3.12 --with pyannote.metrics --with numpy scripts/der/recluster.py <split> <hyp_dir> oracle|<threshold> [...]
Reads $DER_DIR/<hyp_dir>/<meeting>.jsonl (from run.sh --dump), writes RTTMs to $DER_DIR/<hyp_dir>/<mode>/
and prints the split's DER for each mode.
"""
import os, sys, json, pathlib, subprocess
import numpy as np

DER_DIR = pathlib.Path(os.environ.get("DER_DIR", pathlib.Path.home() / ".cache/evertranscript-der"))
split, dump_dir, modes = sys.argv[1], DER_DIR / sys.argv[2], sys.argv[3:]
setup = DER_DIR / "setup"
meetings = setup.joinpath(f"lists/{split}.meetings.txt").read_text().split()
MERGE_GAP = 400

def load(m):
    return [json.loads(l) for l in (dump_dir / f"{m}.jsonl").read_text().splitlines()]

def reference(m):
    ref = {}
    for line in (setup / f"only_words/rttms/{split}/{m}.rttm").read_text().splitlines():
        f = line.split(); ref.setdefault(f[7], []).append((float(f[3]) * 1000, (float(f[3]) + float(f[4])) * 1000))
    return ref

def overlap(runs, segs):
    total = 0.0
    for s, e in runs:
        for a, b in segs:
            total += max(0.0, min(e, b) - max(s, a))
    return total

def agglomerate(vectors, threshold):
    """cluster::agglomerate, line for line: closest pair first, merge into the earlier group,
    recentre by member count, L2-normalize; the lowest member names the group."""
    groups = [([i], np.array(v, dtype=np.float32)) for i, v in enumerate(vectors)]
    while True:
        best = None
        for left in range(len(groups)):
            for right in range(left + 1, len(groups)):
                a, b = groups[left][1], groups[right][1]
                na, nb = np.linalg.norm(a), np.linalg.norm(b)
                score = float(a @ b / (na * nb)) if na > 1e-7 and nb > 1e-7 else 0.0
                if score >= threshold and (best is None or score > best[2]):
                    best = (left, right, score)
        if best is None:
            break
        left, right, _ = best
        members, vector = groups.pop(right)
        weight = len(groups[left][0]); total = weight + len(members)
        merged = (groups[left][1] * weight + vector * len(members)) / total
        norm = np.linalg.norm(merged)
        groups[left] = (groups[left][0] + members, merged / norm if norm > 1e-7 else merged)
    canonical = {}
    for members, _ in groups:
        c = min(members)
        for i in members: canonical[i] = c
    return canonical

def write_rttm(m, obs, labels, path):
    # assemble(): every run is a turn of its voice; consecutive runs of one voice on one channel
    # within MERGE_GAP are one turn.
    turns = {}
    for o, label in zip(obs, labels):
        turns.setdefault((o["channel"], label), []).extend(o["runs"])
    lines = []
    for (channel, label), runs in turns.items():
        runs.sort()
        merged = []
        for s, e in runs:
            if merged and s <= merged[-1][1] + MERGE_GAP:
                merged[-1][1] = max(merged[-1][1], e)
            else:
                merged.append([s, e])
        for s, e in merged:
            lines.append((s, f"SPEAKER {m} 1 {s/1000:.3f} {(e-s)/1000:.3f} <NA> <NA> {channel}-{label} <NA> <NA>"))
    lines.sort()
    path.write_text("".join(l + "\n" for _, l in lines))

for mode in modes:
    out = dump_dir / mode; out.mkdir(exist_ok=True)
    for m in meetings:
        obs = load(m)
        if mode == "oracle":
            ref = reference(m)
            labels = []
            for i, o in enumerate(obs):
                best = max(ref, key=lambda spk: overlap(o["runs"], ref[spk]))
                labels.append(best if overlap(o["runs"], ref[best]) > 0 else f"none{i}")
        else:
            canonical = agglomerate([o["vector"] for o in obs], float(mode))
            labels = [f"c{canonical[i]}" for i in range(len(obs))]
        write_rttm(m, obs, labels, out / f"{m}.rttm")
    result = subprocess.run(["uv", "run", "-q", "--python", "3.12", "--with", "pyannote.metrics", "python", str(pathlib.Path(__file__).with_name("score.py")), split, str(out.relative_to(DER_DIR))], capture_output=True, text=True)
    print(mode, result.stdout.strip().splitlines()[-1])
