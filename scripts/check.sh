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

# The Windows half of the parity gate, when this machine can reach it.
#
# ADR-0025 as amended makes Windows a gate rather than a follow-up, and CI
# builds it — but a failure discovered in CI is a failure discovered after
# the commit. `cargo-xwin` plus LLVM cross-compiles the real workspace here,
# and it immediately found two unused imports that only exist on Windows,
# which is exactly the class of thing a macOS-only loop cannot see.
#
# Optional on purpose: not every machine has the toolchain, and a check that
# refuses to run without a 2 GB dependency is a check people delete.
if command -v cargo-xwin >/dev/null 2>&1; then
  echo "== windows (cross) =="
  PATH="/opt/homebrew/opt/llvm/bin:$PATH" \
    cargo xwin clippy --workspace --all-targets \
      --target x86_64-pc-windows-msvc -- -D warnings
else
  echo "== windows (cross) == skipped: cargo-xwin not installed"
fi

echo "== protocol bindings and schemas are committed =="
git diff --exit-code -- \
  crates/evertranscript-protocol/bindings \
  crates/evertranscript-protocol/schema

echo "== client =="
pnpm -C clients/electron typecheck

echo
echo "all checks passed"
