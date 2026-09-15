"""Cross-meeting recognition, to check MATCH_FLOOR and MATCH_MARGIN against real colleagues.

usage: uv run --python 3.12 --with onnxruntime --with numpy scripts/der/recognition.py <test|dev> [meeting,to,exclude]

A voiceprint per (meeting, reference speaker) is the mean of that speaker's clean 3 s windows (reference
turns >= 3 s whose middle 3 s nobody else overlaps), L2-normalised, embedded through a NumPy copy of
`fbank.rs` (the golden test in fbank.rs pins the Rust one to torchaudio; this copy is checked against it
by the parity numbers in DECISIONS Q115). The product's protocol: each such print is a probe, and the
gallery holds ONE print per person of the same series, the centroid of that person's prints from the
OTHER meetings. Reports correct and impostor score distributions, the margin between the right person
and the nearest wrong one, and for a grid of floors how many returning people are recognized and how
many impostor scores clear the floor.
"""
import os, sys, pathlib, wave, numpy as np, onnxruntime as ort
DER_DIR = pathlib.Path(os.environ.get("DER_DIR", pathlib.Path.home() / ".cache/evertranscript-der"))
split = sys.argv[1] if len(sys.argv) > 1 else "test"
EXCLUDE = set(sys.argv[2].split(",")) if len(sys.argv) > 2 else set()
setup = DER_DIR / "setup"; meetings = setup.joinpath(f"lists/{split}.meetings.txt").read_text().split()
sess = ort.InferenceSession(str(DER_DIR / "models/embedding.onnx"), providers=["CPUExecutionProvider"])

def fbank(x):
    n_fft, flen, hop, nmel = 512, 400, 160, 80
    win = 0.54 - 0.46 * np.cos(2 * np.pi * np.arange(flen) / (flen - 1))
    mel = lambda hz: 1127.0 * np.log(1.0 + hz / 700.0)
    medges = np.linspace(mel(20.0), mel(8000.0), nmel + 2); mbins = mel(np.arange(n_fft // 2 + 1) * (16000 / n_fft))
    fb = np.zeros((nmel, len(mbins)), np.float32)
    for i in range(nmel):
        l, c, r = medges[i], medges[i+1], medges[i+2]
        up = (mbins > l) & (mbins <= c); dn = (mbins > c) & (mbins < r)
        fb[i, up] = (mbins[up] - l) / (c - l); fb[i, dn] = (r - mbins[dn]) / (r - c)
    x = x * 32768.0; nfr = (len(x) - flen) // hop + 1
    fr = np.stack([x[i*hop:i*hop+flen] for i in range(nfr)]); fr = fr - fr.mean(1, keepdims=True)
    fr = np.concatenate([fr[:, :1] * (1 - 0.97), fr[:, 1:] - 0.97 * fr[:, :-1]], 1) * win
    p = np.abs(np.fft.rfft(fr, n=n_fft)) ** 2
    f = np.log(np.maximum(p @ fb.T, 1e-10)).astype(np.float32)
    return f - f.mean(0, keepdims=True)

def embed(x):
    v = sess.run(None, {"input_features": fbank(x)[None]})[0][0]; return v / np.linalg.norm(v)

prints = {}  # (series, meeting, speaker) -> vector
windows_of = {}
meetings = [m for m in meetings if m not in EXCLUDE]
for m in meetings:
    with wave.open(str(DER_DIR / f"audio/{m}.Mix-Headset.wav")) as w:
        audio = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768.0; sr = w.getframerate()
    ref = []
    for line in (setup / f"only_words/rttms/{split}/{m}.rttm").read_text().splitlines():
        f = line.split(); ref.append((float(f[3]), float(f[3]) + float(f[4]), f[7]))
    per = {}
    for s, e, spk in ref:
        if e - s < 3.0: continue
        mid = (s + e) / 2; a, b = mid - 1.5, mid + 1.5
        if any(o != spk and os_ < b and oe > a for os_, oe, o in ref): continue
        per.setdefault(spk, []).append((a, b))
    for spk, wins in per.items():
        rng = np.random.default_rng(0); rng.shuffle(wins); wins = wins[:40]
        vecs = np.stack([embed(audio[int(a*sr):int(b*sr)]) for a, b in wins]); v = vecs.mean(0); v /= np.linalg.norm(v)
        series = m[:-1] if m[-1] in "abcd" else m[:-2]
        prints[(series, m, spk)] = v; windows_of[(m, spk)] = len(wins)
    print(f"{m}: {len(per)} speakers, windows {[len(w) for w in per.values()]}", file=sys.stderr)

keys = list(prints); V = np.stack([prints[k] for k in keys])
# Product protocol: for each probe (one meeting's voiceprint of one person), the gallery holds ONE voiceprint per
# person of the same series, each the centroid of that person's voiceprints from the OTHER meetings. The probe's
# own person is in the gallery only if heard in another meeting. Margin = correct score minus best other score.
by_series = {}
for i, (se, m, p) in enumerate(keys): by_series.setdefault(se, []).append(i)
same, diff, margins, wrong, per_probe = [], [], [], [], []
for se, idx in by_series.items():
    for i in idx:
        _, m, p = keys[i]
        gallery = {}
        for j in idx:
            _, mj, pj = keys[j]
            if mj == m: continue
            gallery.setdefault(pj, []).append(V[j])
        if p not in gallery: continue
        scores = {pj: float(V[i] @ (lambda v: v / np.linalg.norm(v))(np.mean(vs, 0))) for pj, vs in gallery.items()}
        correct = scores.pop(p); others = max(scores.values()) if scores else -1.0
        same.append(correct); diff.extend(scores.values()); margins.append(correct - others)
        if others > correct: wrong.append((m, p, round(correct, 3), max(scores, key=scores.get), round(others, 3)))
        per_probe.append((m, p, correct, others))
same, diff, margins = np.array(same), np.array(diff), np.array(margins)
print(f"{split}: {len(same)} probes, {len(diff)} impostor scores")
print(f"correct  cos mean {same.mean():.3f} min {same.min():.3f} p5 {np.percentile(same,5):.3f} p10 {np.percentile(same,10):.3f}")
print(f"impostor cos mean {diff.mean():.3f} max {diff.max():.3f} p95 {np.percentile(diff,95):.3f} p99 {np.percentile(diff,99):.3f}")
print(f"nearest is right {100*(margins>0).mean():.1f}%;  margin min {margins.min():.3f} p5 {np.percentile(margins,5):.3f} p10 {np.percentile(margins,10):.3f} p25 {np.percentile(margins,25):.3f} median {np.median(margins):.3f}")
print("windows behind the lowest correct scores:", [(m, p, windows_of.get((m, p))) for m, p, c, o in sorted(per_probe, key=lambda r: r[2])[:3]])
print("wrong nearest:", wrong)
print("lowest correct scores:", sorted(per_probe, key=lambda r: r[2])[:6])
print("highest impostor:", sorted(per_probe, key=lambda r: -r[3])[:4])
for floor in (0.45, 0.50, 0.55, 0.60, 0.62):
    for margin in (0.0, 0.05, 0.08, 0.10):
        acc = (same >= floor) & (margins >= margin)
        fa = ((diff >= floor)).mean()
        print(f"floor {floor:.2f} margin {margin:.2f}: recognized {100*acc.mean():5.1f}% of returning people; impostor scores above floor {100*fa:4.1f}%")
