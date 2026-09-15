#!/usr/bin/env bash
# usage: scripts/der/score.sh <test|dev> <hyp_dir> [meeting ...]
set -euo pipefail
cd "$(dirname "$0")"
exec uv run -q --python 3.12 --with pyannote.metrics python score.py "$@"
