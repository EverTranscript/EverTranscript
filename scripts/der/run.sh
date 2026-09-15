#!/usr/bin/env bash
# usage: scripts/der/run.sh <test|dev> <hyp_dir> [--dump]
# Diarizes every meeting of the split with the shipped pipeline (the `rttm`
# example, built in release), one RTTM per meeting under $DER_DIR/<hyp_dir>.
# --dump also writes each meeting's observations (before clustering) as JSON
# lines, for recluster.py. Meetings already done are skipped.
set -euo pipefail
split=$1; out=$2; dump=${3:-}
DER_DIR=${DER_DIR:-$HOME/.cache/evertranscript-der}
cd "$(dirname "$0")/../.."
cargo build -q --release -p evertranscript-core --example rttm
bin=target/release/examples/rttm
mkdir -p "$DER_DIR/$out"
export EVERTRANSCRIPT_DIARIZE_MODELS="$DER_DIR/models"
for m in $(cat "$DER_DIR/setup/lists/$split.meetings.txt"); do
  [ -s "$DER_DIR/$out/$m.rttm" ] && continue
  if [ "$dump" = "--dump" ]; then
    export EVERTRANSCRIPT_RTTM_DUMP="$DER_DIR/$out/$m.jsonl"
  fi
  "$bin" "$DER_DIR/audio/$m.Mix-Headset.wav" "$m" > "$DER_DIR/$out/$m.rttm.tmp" 2> "$DER_DIR/$out/$m.log"
  mv "$DER_DIR/$out/$m.rttm.tmp" "$DER_DIR/$out/$m.rttm"
  tail -1 "$DER_DIR/$out/$m.log"
done
echo "done: $split -> $DER_DIR/$out"
