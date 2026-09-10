#!/usr/bin/env bash
# Everything CI gates, in the order CI runs it, and then the two checks CI
# cannot run — the Windows cross-compile and the e2e. Run before committing:
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

echo "== client tests =="
# The main process has logic worth running, not just typechecking: the Core
# search decides whether a fresh install works at all (DECISIONS Q44).
pnpm -C clients/electron test

# The one check that runs the whole product rather than a layer of it —
# SQLite, Core, socket, Electron main, preload, React — and asserts against
# what the window renders. Everything above it can pass while the app shows
# the wrong thing, which is how "the Registry knew and did not say" survived
# a green suite.
#
# **The exception to this file's own rule**, and deliberately so: CI does not
# gate this one. The Client job is an Ubuntu runner with no Rust toolchain and
# no display, and giving it both plus a Playwright install to drive an Electron
# window is a different job, not a step. Optional here for the same reason the
# Windows cross-check is: a check that refuses to run without a dependency the
# machine lacks is a check people delete.
if command -v playwright-cli >/dev/null 2>&1; then
  echo "== e2e (Voice Registry) =="
  ./scripts/e2e-registry.sh
else
  echo "== e2e (Voice Registry) == skipped: playwright-cli not installed"
fi

echo
echo "all checks passed"
