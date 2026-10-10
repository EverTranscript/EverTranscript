#!/usr/bin/env bash
# The update strip, rendered by the real Client against an isolated debug Core.
#
# An unpackaged Client never checks for updates, so each phase is pushed from
# the main process over its inspector, on the same channel the updater uses.
# What is asserted is what the window shows, and that a link opens only in the
# browser and only for this project (DECISIONS Q331). Isolated the same way as
# e2e-registry.sh: own dirs, auto-record off, no model fetch.
set -euo pipefail
cd "$(dirname "$0")/.."

SESSION=etstrip
PORT="${EVERTRANSCRIPT_E2E_CDP_PORT:-9343}"
INSPECT="${EVERTRANSCRIPT_E2E_INSPECT_PORT:-9344}"
RUNTIME=/tmp/etstrip-run
ROOT=/tmp/etstrip
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
for port in "$PORT" "$INSPECT"; do
  if curl -sf "http://127.0.0.1:$port/json/version" >/dev/null; then
    echo "REFUSING: something else is already debugging on $port"
    exit 1
  fi
done
rm -rf "$ROOT" "$RUNTIME"
mkdir -p "$RUNTIME" "$EVERTRANSCRIPT_HISTORY_DIR" "$EVERTRANSCRIPT_APP_SUPPORT_DIR"
echo '{"briefingAcknowledged":true,"launchAtLogin":false,"autoRecord":false,"checkForUpdates":false}' \
  > "$EVERTRANSCRIPT_APP_SUPPORT_DIR/settings.json"
"$CORE" daemon > "$ROOT/daemon.log" 2>&1 &
DAEMON_PID=$!
until "$CORE" status >/dev/null 2>&1; do sleep 0.3; done
"$CORE" settings | grep "auto-record            off" >/dev/null \
  || { echo "REFUSING: auto-record is on in the test instance"; exit 1; }

(cd clients/electron && npx electron . --remote-debugging-port=$PORT --inspect=$INSPECT \
  > "$ROOT/electron.log" 2>&1 &)
until curl -sf http://127.0.0.1:$PORT/json/version >/dev/null; do sleep 0.3; done
until curl -sf http://127.0.0.1:$INSPECT/json/list >/dev/null; do sleep 0.3; done
playwright-cli -s="$SESSION" attach --cdp=http://127.0.0.1:$PORT >/dev/null
until playwright-cli -s="$SESSION" --raw eval "() => !!document.querySelector('main')" | grep -q true; do
  sleep 0.3
done

main() { node scripts/e2e-main-eval.mjs "$INSPECT" "$1"; }

# Pushes one state, then reads the strip back as "text || buttons || links".
show() {
  main "(() => { for (const w of require('electron').BrowserWindow.getAllWindows()) w.webContents.send('updates:state', $1); return 1 })()" >/dev/null
  sleep 0.5
  playwright-cli -s="$SESSION" --raw eval "() => {
    const s = document.querySelector('main [role=status]');
    if (!s) return 'none';
    const text = s.innerText.replace(/\\n/g, ' ');
    const buttons = [...s.querySelectorAll('button')].map((b) => b.textContent).join(',');
    const links = [...s.querySelectorAll('a')].map((a) => a.href).join(',');
    return text + ' || ' + buttons + ' || ' + links;
  }"
}

# expect <state> <fixed string>…: every string must appear in what is shown.
expect() {
  local state=$1 shown
  shift
  shown=$(show "$state")
  echo "$state => $shown"
  for want in "$@"; do
    grep -qiF -- "$want" <<<"$shown" || { echo "FAIL: \"$want\" is not shown for $state"; exit 1; }
  done
}

NOTES=https://github.com/EverTranscript/EverTranscript/releases/tag/v1.2.0

echo "== every phase =="
expect '{phase:"available",version:"1.2.0"}' \
  "A new version is available: 1.2.0" "|| Download,Later ||" "$NOTES"
expect '{phase:"downloading",version:"1.2.0",percent:42}' "Downloading the update 1.2.0 — 42% ||  ||"
expect '{phase:"downloaded",version:"1.2.0"}' "Ready to install: 1.2.0" "|| Restart to update ||" "$NOTES"
# Waiting has nothing to press: the install runs on its own once the Core grants the hold (Q330).
expect '{phase:"waiting",version:"1.2.0",busy:"recording"}' "It installs when the recording ends." "||  ||"
expect '{phase:"waiting",version:"1.2.0",busy:"diarizing"}' "It installs when Diarization finishes." "||  ||"
expect '{phase:"waiting",version:"1.2.0",busy:"unreadable"}' "It installs once the Core answers." "||  ||"
expect '{phase:"installing",version:"1.2.0"}' "Installing the update… ||  ||"
expect '{phase:"failed",version:"1.2.0",retry:"download",message:"net::ERR_INTERNET_DISCONNECTED"}' \
  "The download failed: net::ERR_INTERNET_DISCONNECTED" "|| Try again ||"
expect '{phase:"failed",version:"1.2.0",retry:"install",message:"did not pass validation"}' \
  "The update did not install: did not pass validation" "|| Try again ||"
expect '{phase:"none"}' "none"

echo "== links =="
# Recorded instead of opened, so the run never touches the real browser.
main "(() => { globalThis.opened = []; require('electron').shell.openExternal = async (u) => { globalThis.opened.push(u) }; return 1 })()" >/dev/null
show '{phase:"available",version:"1.2.0"}' >/dev/null
playwright-cli -s="$SESSION" click "main [role=status] a" >/dev/null
denied=$(playwright-cli -s="$SESSION" --raw eval "() => window.open('https://example.com/') === null")
sleep 0.5
result=$(main "({ opened: globalThis.opened, windows: require('electron').BrowserWindow.getAllWindows().length })")
echo "window.open elsewhere denied: $denied; $result"
[ "$denied" = true ] || { echo "FAIL: a link elsewhere opened a window"; exit 1; }
grep -qF "{\"opened\":[\"$NOTES\"],\"windows\":1}" <<<"$result" \
  || { echo "FAIL: Release notes did not go to the browser alone"; exit 1; }

echo "PASS"
