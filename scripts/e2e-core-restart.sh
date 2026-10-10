#!/usr/bin/env bash
# The catch-up's restart, against a real isolated debug Core (DECISIONS Q333).
#
# `update/restart` must be refused to a connection without the hold, and
# under the hold it must stop the daemon cleanly — exit 0, address let go —
# so the new Client can start its own Core in the old one's place. Run with
# WITH_TRAY=1 to stop it through the macOS menu bar's run loop, which is how
# the installed Core runs. Isolated as e2e-update-strip.sh is: own dirs,
# auto-record off, no model fetch.
set -euo pipefail
cd "$(dirname "$0")/.."

RUNTIME=/tmp/etrestart-run
ROOT=/tmp/etrestart
CORE="$PWD/target/debug/evertranscript"

export EVERTRANSCRIPT_RUNTIME_DIR="$RUNTIME"
export EVERTRANSCRIPT_HISTORY_DIR="$ROOT/history"
export EVERTRANSCRIPT_APP_SUPPORT_DIR="$ROOT/support"
export EVERTRANSCRIPT_NO_MODEL_FETCH=1
export EVERTRANSCRIPT_NO_LOGIN_ITEM=1
[ "${WITH_TRAY:-0}" = 1 ] || export EVERTRANSCRIPT_NO_TRAY=1

cleanup() { [ -n "${DAEMON_PID:-}" ] && kill "$DAEMON_PID" >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "== build =="
cargo build --bin evertranscript
(cd clients/electron && npx tsc -p tsconfig.test.json)

start_core() {
  "$CORE" daemon >> "$ROOT/daemon.log" 2>&1 &
  DAEMON_PID=$!
  until "$CORE" status >/dev/null 2>&1; do sleep 0.3; done
  "$CORE" settings | grep "auto-record            off" >/dev/null \
    || { echo "REFUSING: auto-record is on in the test instance"; exit 1; }
}

echo "== isolated instance (tray: ${WITH_TRAY:-0}) =="
rm -rf "$ROOT" "$RUNTIME"
mkdir -p "$RUNTIME" "$EVERTRANSCRIPT_HISTORY_DIR" "$EVERTRANSCRIPT_APP_SUPPORT_DIR"
echo '{"briefingAcknowledged":true,"launchAtLogin":false,"autoRecord":false,"checkForUpdates":false}' \
  > "$EVERTRANSCRIPT_APP_SUPPORT_DIR/settings.json"
start_core
FIRST=$DAEMON_PID

node --input-type=module -e '
import assert from "node:assert/strict";
import { CoreClient, nobodyListening } from "./clients/electron/dist-test/main/core-client.js";

const open = async () => {
  const core = await CoreClient.connect();
  await core.initialize("e2e-core-restart", "0.0.0");
  return core;
};

const stranger = await open();
await assert.rejects(stranger.request("update/restart", {}), /update\/hold/);
assert.equal(stranger.open, true, "a refused restart leaves the Core serving");
stranger.close();

const held = await open();
assert.deepEqual(await held.request("update/hold", {}), { held: true });
await held.request("update/restart", {}).catch(() => {});
await held.closed;

for (let attempt = 0; ; attempt += 1) {
  try {
    (await CoreClient.connect()).close();
  } catch (error) {
    if (nobodyListening(error)) break;
  }
  assert.ok(attempt < 50, "the old Core still holds its address after 10 s");
  await new Promise((resolve) => setTimeout(resolve, 200));
}
console.log("restart: refused without the hold; under it, the Core went and let go of its address");
'

set +e
wait "$FIRST"
CODE=$?
set -e
[ "$CODE" = 0 ] || { echo "FAIL: the Core exited $CODE"; tail -20 "$ROOT/daemon.log"; exit 1; }
grep -q "stopping for an update" "$ROOT/daemon.log" \
  || { echo "FAIL: the Core did not say why it stopped"; exit 1; }

start_core
echo "a new Core started at the same address"
echo PASS
