#!/usr/bin/env bash
# usage: scripts/der/fetch.sh <test|dev>
# Clones BUT's AMI-diarization-setup, downloads the split's Mix-Headset WAVs
# (four at a time), and links the app's downloaded ONNX models.
set -euo pipefail
split=$1
DER_DIR=${DER_DIR:-$HOME/.cache/evertranscript-der}
mkdir -p "$DER_DIR/audio" "$DER_DIR/models"
[ -d "$DER_DIR/setup" ] || git clone -q --depth 1 https://github.com/BUTSpeechFIT/AMI-diarization-setup "$DER_DIR/setup"

case "$(uname)" in
  Darwin) models="$HOME/Library/Application Support/EverTranscript/models" ;;
  *) models="${XDG_DATA_HOME:-$HOME/.local/share}/EverTranscript/models" ;;
esac
ln -sf "$models/diarize-segmentation.onnx" "$DER_DIR/models/segmentation.onnx"
ln -sf "$models/diarize-embedding.onnx" "$DER_DIR/models/embedding.onnx"

fetch_one() {
  m=$1; f="$DER_DIR/audio/$m.Mix-Headset.wav"
  [ -s "$f" ] && exit 0
  if curl -sS -f --retry 3 -o "$f.part" "https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/$m/audio/$m.Mix-Headset.wav"; then
    mv "$f.part" "$f"; echo "got $m"
  else
    echo "FAILED $m" >&2; exit 1
  fi
}
export -f fetch_one; export DER_DIR
xargs -P 4 -n 1 bash -c 'fetch_one "$0"' < "$DER_DIR/setup/lists/$split.meetings.txt"
echo "fetched $split into $DER_DIR"
