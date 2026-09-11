#!/usr/bin/env bash
# Everything CI gates, in the order CI runs it, plus the e2e, which CI cannot.
# Run before committing: `cargo fmt` alone has twice let a clippy failure
# through to a commit.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== fmt =="
cargo fmt --all --check

echo "== clippy =="
cargo clippy --workspace --all-targets -- -D warnings

echo "== tests =="
cargo test --workspace

# The Windows half of the parity gate — off, and not for want of a toolchain.
#
# ADR-0025 as amended makes Windows a gate rather than a follow-up, and a
# failure discovered in CI is a failure discovered after the commit, so
# `cargo-xwin` plus LLVM used to cross-compile the real workspace here — it
# found two unused imports that exist only on Windows, exactly the class of
# thing a macOS-only loop cannot see. It no longer gets that far.
# `mp3lame-sys` picks its build path on `cfg(windows)` — the *host*, not the
# target — so a Unix host always takes its autotools route whatever it is
# building for, and libtool cannot drive `clang-cl`: it mangles the flags
# until every compile is "clang-cl: error: no input files". The walls before
# that one do have answers (`MP3LAME_SYS_OVERRIDE_HOST` for the missing
# `--host`, `CPP`/`CPPFLAGS` for the preprocessor); getting past them is what
# exposes it. Its own `cfg(windows)` branch is a plain `cc::Build` that would
# cross fine, so the fix is upstream gating on `target_env = "msvc"` instead.
#
# Hence a variable rather than `command -v cargo-xwin`: installing the
# toolchain turned a skip into a hard failure that stopped this script before
# the Client checks and the e2e ever ran, which is red for a reason that has
# nothing to do with this code. CI still builds Windows natively, where none
# of this applies. Set the variable to try again the day mp3lame-sys is
# fixed — the toolchain is still installed on this machine.
if [ -n "${EVERTRANSCRIPT_WINDOWS_CROSS:-}" ]; then
  echo "== windows (cross) =="
  PATH="/opt/homebrew/opt/llvm/bin:$PATH" \
    cargo xwin clippy --workspace --all-targets \
      --target x86_64-pc-windows-msvc -- -D warnings
else
  echo "== windows (cross) == skipped: mp3lame-sys cannot cross-compile from a"
  echo "                      Unix host (see the comment); CI gates Windows natively"
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
