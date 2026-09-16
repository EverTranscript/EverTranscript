#!/usr/bin/env bash
#
# Fetches the AMI test set and BUT's references into a cache directory, for
# the DER and EER harness (`tests/diarization_accuracy.rs`, ticket 01).
#
# Deliberately a script a person runs, not something a test does. The whole
# product promises to work offline (ADR-0002), and a test binary that reached
# the network to measure that promise would be the worst possible way to keep
# it. Nothing in `cargo test` touches this; the harness reads the directory
# or skips.
#
#   scripts/fetch-ami.sh ~/ami
#   EVERTRANSCRIPT_MEASURE_DER=1 EVERTRANSCRIPT_AMI_DIR=~/ami \
#     cargo test -p evertranscript-core --test diarization_accuracy -- --nocapture
#
# Licences, both of which permit this use and neither of which permits
# redistribution inside this repository:
#   AMI Meeting Corpus audio — CC BY 4.0
#   BUT AMI-diarization-setup references — Apache-2.0
#
# The harness itself is plain Rust and runs on both platforms (ADR-0025).
# This fetcher is bash, which on Windows means Git Bash or WSL; it only
# downloads and converts files, so a corpus fetched on either machine works
# on the other over a share.
#
# About 5 GB and a slow hour on a home connection. Resumable: every step
# skips what it already has, so re-running after an interruption costs only
# what is missing.

set -euo pipefail

root="${1:-}"
if [[ -z "$root" ]]; then
  echo "usage: $0 <cache-dir>" >&2
  exit 2
fi

mkdir -p "$root/audio" "$root/rttm" "$root/.download"

for tool in curl ffmpeg; do
  command -v "$tool" >/dev/null || {
    echo "$0 needs $tool on PATH" >&2
    exit 1
  }
done

# BUT's split, vendored here as a list rather than fetched, so the set being
# measured is visible in the diff when it changes. This is the `test` half of
# AMI-diarization-setup — the same sixteen meetings pyannote publishes its
# 18.8% against.
meetings=(
  EN2002a EN2002b EN2002c EN2002d
  ES2004a ES2004b ES2004c ES2004d
  IS1009a IS1009b IS1009c IS1009d
  TS3003a TS3003b TS3003c TS3003d
)

rttm_base="https://raw.githubusercontent.com/BUTSpeechFIT/AMI-diarization-setup/main/only_words/rttms/test"
audio_base="https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus"

echo "==> references (BUT only_words, Apache-2.0)"
for meeting in "${meetings[@]}"; do
  target="$root/rttm/$meeting.rttm"
  [[ -s "$target" ]] && continue
  echo "    $meeting"
  curl -fsSL --retry 3 -o "$target.part" "$rttm_base/$meeting.rttm"
  mv "$target.part" "$target"
done

echo "==> audio (AMI Mix-Headset, CC BY 4.0) — resampled to 16 kHz mono"
for meeting in "${meetings[@]}"; do
  target="$root/audio/$meeting.wav"
  [[ -s "$target" ]] && continue
  source="$root/.download/$meeting.Mix-Headset.wav"
  if [[ ! -s "$source" ]]; then
    echo "    downloading $meeting"
    curl -fL --retry 3 --progress-bar -o "$source.part" \
      "$audio_base/$meeting/audio/$meeting.Mix-Headset.wav"
    mv "$source.part" "$source"
  fi
  # The harness asserts 16 kHz mono rather than resampling, so that a corpus
  # prepared wrong fails loudly instead of being silently measured through a
  # resampler the shipped pipeline does not use.
  echo "    converting $meeting"
  ffmpeg -nostdin -loglevel error -y -i "$source" -ac 1 -ar 16000 "$target.part.wav"
  mv "$target.part.wav" "$target"
done

cat <<EOF

Done. ${#meetings[@]} meetings in $root

  EVERTRANSCRIPT_MEASURE_DER=1 EVERTRANSCRIPT_AMI_DIR="$root" \\
    cargo test -p evertranscript-core --test diarization_accuracy -- --nocapture

The downloaded originals are in $root/.download and are only needed to
re-convert; deleting them frees most of the space.
EOF
