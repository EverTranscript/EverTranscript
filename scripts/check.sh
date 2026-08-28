#!/usr/bin/env bash
# Everything CI gates, in the order CI runs it. Run before committing —
# `cargo fmt` alone has twice let a clippy failure through to a commit.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== fmt =="
cargo fmt --all --check

echo "== clippy =="
cargo clippy --workspace --all-targets -- -D warnings

echo "== tests =="
cargo test --workspace

echo "== protocol bindings and schemas are committed =="
git diff --exit-code -- \
  crates/evertranscript-protocol/bindings \
  crates/evertranscript-protocol/schema

echo "== client =="
pnpm -C clients/electron typecheck

echo
echo "all checks passed"
