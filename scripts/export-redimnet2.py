# /// script
# requires-python = ">=3.11,<3.13"
# dependencies = [
#   "torch==2.8.0",
#   "torchaudio==2.8.0",
#   "onnx==1.18.0",
#   "onnxruntime==1.22.1",
#   "onnxscript==0.3.2",
#   "numpy<2.3",
#   "scipy==1.15.3",
# ]
# ///
"""Exports ReDimNet2-B3 (PalabraAI/redimnet2, MIT) to ONNX for EverTranscript.

    uv run scripts/export-redimnet2.py [--out models/redimnet2-b3-vox2-lm.onnx]

The graph takes `waveform [N, T]` float32 at 16 kHz and returns `embedding
[N, 192]`, mel frontend included, so the Rust side feeds raw audio (Granola's
contract, and what the upstream README documents). The export is checked
against PyTorch on a fixed waveform, and the check prints the vector the Rust
spike compares against. Nothing here touches the network except torch.hub's
fetch of the upstream checkpoint, which is pinned by tag.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys

import numpy as np
import torch

UPSTREAM = "PalabraAI/redimnet2:v1.0.0"
MODEL = ("b3", "lm", "vox2")
EMBED_DIM = 192
SAMPLE_RATE = 16_000
OPSET = 18


def fixed_waveform(seconds: float = 3.0) -> torch.Tensor:
    """A deterministic 3 s signal: two tones plus seeded noise, so any platform
    can rebuild it without a file."""
    n = int(seconds * SAMPLE_RATE)
    t = torch.arange(n, dtype=torch.float32) / SAMPLE_RATE
    g = torch.Generator().manual_seed(42)
    noise = torch.randn(n, generator=g) * 0.05
    wave = 0.3 * torch.sin(2 * torch.pi * 220.0 * t) + 0.2 * torch.sin(2 * torch.pi * 1375.0 * t) + noise
    return wave.unsqueeze(0)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="target/models/redimnet2-b3-vox2-lm.onnx")
    args = parser.parse_args()
    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)

    torch.manual_seed(0)
    model = torch.hub.load(UPSTREAM, "redimnet2", model_name=MODEL[0], train_type=MODEL[1], dataset=MODEL[2], pretrained=True)
    model.eval()

    wave = fixed_waveform()
    with torch.no_grad():
        reference = model(wave)
    assert reference.shape == (1, EMBED_DIM), reference.shape

    # The TorchScript exporter, not dynamo: dynamo has no ONNX mapping for
    # the variance primitive the mel frontend's normalisation lowers to
    # (prims.broadcast_in_dim, torch 2.8), and TorchScript emits ReduceMean
    # and STFT (opset 17) for the same code.
    torch.onnx.export(
        model,
        (wave,),
        str(out),
        dynamo=False,
        opset_version=OPSET,
        input_names=["waveform"],
        output_names=["embedding"],
        dynamic_axes={"waveform": {0: "batch", 1: "samples"}, "embedding": {0: "batch"}},
        do_constant_folding=True,
    )

    import onnx
    graph = onnx.load(str(out))
    onnx.checker.check_model(graph)
    ops = sorted({node.op_type for node in graph.graph.node})

    import onnxruntime as ort
    session = ort.InferenceSession(str(out), providers=["CPUExecutionProvider"])
    got = session.run(["embedding"], {"waveform": wave.numpy()})[0]
    ref = reference.numpy()
    cosine = float(np.dot(got[0], ref[0]) / (np.linalg.norm(got[0]) * np.linalg.norm(ref[0])))

    # A second, longer waveform proves the samples axis is really dynamic.
    long_wave = torch.cat([wave, wave], dim=1)
    with torch.no_grad():
        long_ref = model(long_wave).numpy()
    long_got = session.run(["embedding"], {"waveform": long_wave.numpy()})[0]
    long_cosine = float(np.dot(long_got[0], long_ref[0]) / (np.linalg.norm(long_got[0]) * np.linalg.norm(long_ref[0])))

    # Tone-only 3 s vector for the Rust spike, which cannot reproduce torch's
    # seeded noise but can rebuild two sine tones exactly.
    n = int(3.0 * SAMPLE_RATE)
    t = torch.arange(n, dtype=torch.float32) / SAMPLE_RATE
    tones = (0.3 * torch.sin(2 * torch.pi * 220.0 * t) + 0.2 * torch.sin(2 * torch.pi * 1375.0 * t)).unsqueeze(0)
    with torch.no_grad():
        tones_ref = model(tones).numpy()[0]

    sha = hashlib.sha256(out.read_bytes()).hexdigest()
    report = {
        "upstream": UPSTREAM,
        "model": "-".join(MODEL),
        "file": str(out),
        "size_bytes": out.stat().st_size,
        "sha256": sha,
        "opset": OPSET,
        "ops": ops,
        "has_stft": "STFT" in ops,
        "cosine_vs_torch_3s": cosine,
        "cosine_vs_torch_6s": long_cosine,
        "reference_3s_first8": [float(x) for x in ref[0][:8]],
        "torch": torch.__version__,
        "tones_only_3s": [float(x) for x in tones_ref],
    }
    report_path = out.with_suffix(".export.json")
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    np.save(out.with_suffix(".reference.npy"), ref)
    print(json.dumps(report, indent=2))
    ok = cosine > 0.9999 and long_cosine > 0.9999
    print("OK" if ok else "MISMATCH", file=sys.stderr)
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
