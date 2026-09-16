#!/usr/bin/env bash
#
# Fetches an AMI split and BUT's references into a cache directory, for the
# DER and EER harness (`tests/diarization_accuracy.rs`, ticket 01).
#
# Deliberately a script a person runs, not something a test does. The whole
# product promises to work offline (ADR-0002), and a test binary that reached
# the network to measure that promise would be the worst possible way to keep
# it. Nothing in `cargo test` touches this; the harness reads the directory
# or skips.
#
#   scripts/fetch-ami.sh ~/ami             # the test split, the default
#   scripts/fetch-ami.sh ~/ami-dev dev     # the dev split, for choosing thresholds
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
split="${2:-test}"
if [[ -z "$root" ]]; then
  echo "usage: $0 <cache-dir> [test|dev]" >&2
  exit 2
fi
if [[ "$split" != test && "$split" != dev ]]; then
  echo "$0: split must be test or dev, not '$split'" >&2
  exit 2
fi

mkdir -p "$root/audio" "$root/rttm" "$root/.download"

for tool in curl ffmpeg; do
  command -v "$tool" >/dev/null || {
    echo "$0 needs $tool on PATH" >&2
    exit 1
  }
done

# BUT's splits, vendored here as lists rather than fetched, so the set being
# measured is visible in the diff when it changes.
#
# `test` is the same sixteen meetings pyannote publishes its 18.8% against.
# `dev` is where a threshold is chosen, so that the number reported on test is
# a measurement rather than a fit — choosing on test and reporting on test
# reports how well the constant was tuned, not how well the model works.
#
# Each split goes in its own cache directory: pass a different `<cache-dir>`
# for each, and point EVERTRANSCRIPT_AMI_DIR at whichever is being measured.
# That keeps one layout rather than teaching the harness about splits.
case "$split" in
  test)
    meetings=(
      EN2002a EN2002b EN2002c EN2002d
      ES2004a ES2004b ES2004c ES2004d
      IS1009a IS1009b IS1009c IS1009d
      TS3003a TS3003b TS3003c TS3003d
    )
    ;;
  dev)
    meetings=(
      ES2011a ES2011b ES2011c ES2011d
      IB4001 IB4002 IB4003 IB4004 IB4010 IB4011
      IS1008a IS1008b IS1008c IS1008d
      TS3004a TS3004b TS3004c TS3004d
    )
    ;;
esac

rttm_base="https://raw.githubusercontent.com/BUTSpeechFIT/AMI-diarization-setup/main/only_words/rttms/$split"
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

Done. ${#meetings[@]} $split meetings in $root

  EVERTRANSCRIPT_MEASURE_DER=1 EVERTRANSCRIPT_AMI_DIR="$root" \\
    cargo test -p evertranscript-core --test diarization_accuracy -- --nocapture

The downloaded originals are in $root/.download and are only needed to
re-convert; deleting them frees most of the space.
EOF
