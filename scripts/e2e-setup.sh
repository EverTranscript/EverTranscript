#!/usr/bin/env bash
# Setup on a first install, rendered by the real Client against an isolated debug Core.
#
# Starts from an unacknowledged Briefing, walks every step, and asserts the main
# view appears after Done. Every other e2e seeds `briefingAcknowledged`, so none
# of them ever saw setup end (.scratch/zx8-upgrade-e2e/issues/02). Isolated the
# same way as e2e-update-strip.sh: own dirs, auto-record off, no model fetch.
set -euo pipefail
cd "$(dirname "$0")/.."

SESSION=etsetup
PORT="${EVERTRANSCRIPT_E2E_CDP_PORT:-9345}"
RUNTIME=/tmp/etsetup-run
ROOT=/tmp/etsetup
CORE="$PWD/target/debug/evertranscript"

export EVERTRANSCRIPT_RUNTIME_DIR="$RUNTIME"
export EVERTRANSCRIPT_HISTORY_DIR="$ROOT/history"
export EVERTRANSCRIPT_APP_SUPPORT_DIR="$ROOT/support"
export EVERTRANSCRIPT_NO_MODEL_FETCH=1
export EVERTRANSCRIPT_NO_TRAY=1
export EVERTRANSCRIPT_NO_LOGIN_ITEM=1
export EVERTRANSCRIPT_BIN="$CORE"

cleanup() {
  playwright-cli -s="$SESSION" detach >/dev/null 2>&1 || true
  pkill -f "electron .* --remote-debugging-port=$PORT" >/dev/null 2>&1 || true
  [ -n "${DAEMON_PID:-}" ] && kill "$DAEMON_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "== build =="
cargo build --bin evertranscript
pnpm -C clients/electron build >/dev/null

echo "== isolated instance =="
cleanup
if curl -sf "http://127.0.0.1:$PORT/json/version" >/dev/null; then
  echo "REFUSING: something else is already debugging on $PORT"
  exit 1
fi
rm -rf "$ROOT" "$RUNTIME"
mkdir -p "$RUNTIME" "$EVERTRANSCRIPT_HISTORY_DIR" "$EVERTRANSCRIPT_APP_SUPPORT_DIR"
# Local Summaries already chosen, so the Backend step lets Continue through.
echo '{"briefingAcknowledged":false,"launchAtLogin":false,"autoRecord":false,"checkForUpdates":false,"summaryBackend":"local"}' \
  > "$EVERTRANSCRIPT_APP_SUPPORT_DIR/settings.json"
"$CORE" daemon > "$ROOT/daemon.log" 2>&1 &
DAEMON_PID=$!
until "$CORE" status >/dev/null 2>&1; do sleep 0.3; done
"$CORE" settings | grep "auto-record            off" >/dev/null \
  || { echo "REFUSING: auto-record is on in the test instance"; exit 1; }

(cd clients/electron && npx electron . --remote-debugging-port=$PORT \
  > "$ROOT/electron.log" 2>&1 &)
until curl -sf http://127.0.0.1:$PORT/json/version >/dev/null; do sleep 0.3; done
playwright-cli -s="$SESSION" attach --cdp=http://127.0.0.1:$PORT >/dev/null

page() { playwright-cli -s="$SESSION" --raw eval "$1"; }
# Presses the enabled button whose label is one of the given words, ignoring case
# (a Mac capitalizes labels Windows does not). Answers its label, or "none".
press() {
  page "() => {
    const want = [$1].map((w) => w.toLowerCase());
    const b = [...document.querySelectorAll('button')].find(
      (b) => !b.disabled && want.includes(b.textContent.trim().toLowerCase()));
    if (!b) return 'none';
    b.click();
    return b.textContent.trim();
  }" | tr -d '"'
}
until [ "$(press "'I have read this'")" != none ]; do sleep 0.3; done
echo "acknowledged the Briefing"

echo "== every step =="
for step in 1 2 3 4 5 6 7; do
  sleep 0.5
  pressed=$(press "'Continue','Done'")
  echo "step $step: $pressed"
  [ "$pressed" = none ] && { echo "FAIL: nothing to press on step $step"; exit 1; }
  [ "$pressed" = Done ] && break
done

echo "== the main view =="
for _ in $(seq 1 20); do
  shown=$(page "() => !!document.querySelector('aside') && !document.querySelector('footer button.push-default')")
  [ "$shown" = true ] && break
  sleep 0.2
done
echo "main view shown after Done: $shown"
[ "$shown" = true ] || { echo "FAIL: setup is still showing after Done"; exit 1; }

echo "PASS"
