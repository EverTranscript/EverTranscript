#!/usr/bin/env bash
# The Voice Registry, driven end to end through the real stack: SQLite → Core
# → JSON-RPC over the unix socket → Electron main → preload → React. Nothing
# is stubbed, and the assertions are made against what the window actually
# renders.
#
# Three things about this harness are load-bearing, and all three were learned
# by getting them wrong:
#
#   1. **`autoRecord: false` is written before the Core ever starts.** Auto-
#      Record lives in the Core and ships On (ADR-0023), so a test instance
#      that comes up armed starts capturing whatever real meeting is live on
#      this machine — which is not hypothetical, it recorded one. There is no
#      CLI switch for it (`settings` only changes `--chinese-script`, because
#      the toggle is consent rather than preference), so the settings file is
#      the only place to close it from outside the UI.
#   2. **The runtime dir is short.** `sockaddr_un.sun_path` is 104 bytes; a
#      long path kills the Core with "path must be shorter than SUN_LEN",
#      which the Client then misreports as "no Core is listening".
#   3. **`EVERTRANSCRIPT_NO_MODEL_FETCH`.** An empty app-support dir otherwise
#      makes the Core fetch ~3.4 GB of models before it will do anything.
set -euo pipefail
cd "$(dirname "$0")/.."

SESSION=ete2e
RUNTIME=/tmp/ete2e-run
ROOT=/tmp/ete2e
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
  pkill -f "electron .* --remote-debugging-port=9222" >/dev/null 2>&1 || true
  [ -n "${DAEMON_PID:-}" ] && kill "$DAEMON_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "== build =="
cargo build --bin evertranscript
pnpm -C clients/electron build >/dev/null

echo "== isolated instance =="
cleanup
rm -rf "$ROOT" "$RUNTIME"
mkdir -p "$RUNTIME" "$EVERTRANSCRIPT_HISTORY_DIR" "$EVERTRANSCRIPT_APP_SUPPORT_DIR"
cat > "$EVERTRANSCRIPT_APP_SUPPORT_DIR/settings.json" <<'JSON'
{
  "briefingAcknowledged": true,
  "launchAtLogin": false,
  "autoRecord": false,
  "checkForUpdates": false,
  "chineseScript": "simplified"
}
JSON

"$CORE" daemon > "$ROOT/daemon.log" 2>&1 &
DAEMON_PID=$!
until "$CORE" status >/dev/null 2>&1; do sleep 0.3; done
"$CORE" settings | grep -q "auto-record            off" \
  || { echo "REFUSING: auto-record is on in the test instance"; exit 1; }

echo "== seed =="
# Two Meetings and three voices. The first Meeting is untitled and only
# app-detected (the common case); the second is titled. Alice is in both, so
# a row naming the *later* one is visibly wrong; the Operator is in neither,
# so a row that invents a capture for her is visibly wrong too.
sqlite3 "$EVERTRANSCRIPT_HISTORY_DIR/.data/EverTranscript.db" < scripts/e2e-registry.sql

echo "== drive =="
(cd clients/electron && npx electron . --remote-debugging-port=9222 \
  > "$ROOT/electron.log" 2>&1 &)
until curl -sf http://127.0.0.1:9222/json/version >/dev/null; do sleep 0.3; done
playwright-cli -s="$SESSION" attach --cdp=http://127.0.0.1:9222 >/dev/null
playwright-cli -s="$SESSION" click "getByRole('button', { name: 'Voices' })" >/dev/null

rendered=$(playwright-cli -s="$SESSION" --raw eval \
  "el => [...el.querySelectorAll('[data-testid=registry-first-seen]')].map(n => n.textContent).join('\n')" \
  "main")

echo "$rendered"
grep -q "First heard Mar 4, 2026, 9:30 AM · Microsoft Teams, 2026-03-04" <<<"$rendered" \
  || { echo "FAIL: Alice's row does not name the Meeting she was first heard in"; exit 1; }
grep -q "First heard May 19, 2026, 2:00 PM · Design review" <<<"$rendered" \
  || { echo "FAIL: the unnamed voice's row does not name its titled Meeting"; exit 1; }
# Occurrences, not lines: `--raw eval` hands back one JSON string with the
# newlines still escaped inside it, so `grep -c` would always answer 1.
[ "$(grep -o "First heard" <<<"$rendered" | wc -l | tr -d ' ')" = 2 ] \
  || { echo "FAIL: a voice with no Meetings was given a capture anyway"; exit 1; }

counts=$(playwright-cli -s="$SESSION" --raw eval \
  "el => [...el.querySelectorAll('[data-testid=registry-meeting-count]')].map(n => n.textContent).join(' | ')" \
  "main")
echo "counts: $counts"
# Both forms in one assertion. A voice heard in no Meeting has no count to
# click, so it is not in this list at all.
grep -qF "2 meetings | 1 meeting" <<<"$counts" \
  || { echo "FAIL: the counts do not agree with their numbers"; exit 1; }

last=$(playwright-cli -s="$SESSION" --raw eval \
  "el => [...el.querySelectorAll('[data-testid=registry-last-heard]')].map(n => n.textContent).join('\n')" \
  "main")
echo "$last"
# Alice alone gets this line: she is the only voice in two Meetings, and for a
# voice heard once the last time is the first time already on screen.
[ "$(grep -o "Last heard" <<<"$last" | wc -l | tr -d ' ')" = 1 ] \
  || { echo "FAIL: Last heard is shown for a voice heard in one Meeting"; exit 1; }
grep -q "Last heard May 19, 2026, 2:00 PM" <<<"$last" \
  || { echo "FAIL: Last heard does not name the most recent Meeting"; exit 1; }

# Click through. Alice's capture is the *older* Meeting, and the Client
# defaults to the newest, so landing on it is only possible if the row
# actually carried its id — a no-op click would leave "Design review" up.
playwright-cli -s="$SESSION" click "[data-testid=registry-first-seen] >> nth=0" >/dev/null
opened=$(playwright-cli -s="$SESSION" --raw eval "el => el.textContent" "main h1")
echo "opened: $opened"
grep -q "Microsoft Teams, 2026-03-04" <<<"$opened" \
  || { echo "FAIL: clicking the capture line did not open that Meeting"; exit 1; }

# And the other end. Ordered second on purpose: the older Meeting is open now,
# so arriving at "Design review" is a real move rather than the default the
# Client would have shown anyway.
playwright-cli -s="$SESSION" click "getByRole('button', { name: 'Voices' })" >/dev/null
playwright-cli -s="$SESSION" click "[data-testid=registry-last-heard]" >/dev/null
opened=$(playwright-cli -s="$SESSION" --raw eval "el => el.textContent" "main h1")
echo "opened: $opened"
grep -q "Design review" <<<"$opened" \
  || { echo "FAIL: clicking the last-heard line did not open that Meeting"; exit 1; }

# The count expands to the Meetings it counts, newest first.
playwright-cli -s="$SESSION" click "getByRole('button', { name: 'Voices' })" >/dev/null
playwright-cli -s="$SESSION" click "[data-testid=registry-meeting-count] >> nth=0" >/dev/null
listed=$(playwright-cli -s="$SESSION" --raw eval \
  "el => [...el.querySelectorAll('[data-testid=registry-meeting-entry]')].map(n => n.textContent).join(' | ')" \
  "main")
echo "listed: $listed"
grep -q "Design review.*Microsoft Teams, 2026-03-04" <<<"$listed" \
  || { echo "FAIL: the count did not expand to both Meetings, newest first"; exit 1; }

# The older entry, with "Design review" still open — so landing on the Teams
# call is the list doing the work rather than the Client's default.
playwright-cli -s="$SESSION" click "[data-testid=registry-meeting-entry] >> nth=1" >/dev/null
opened=$(playwright-cli -s="$SESSION" --raw eval "el => el.textContent" "main h1")
echo "opened: $opened"
grep -q "Microsoft Teams, 2026-03-04" <<<"$opened" \
  || { echo "FAIL: clicking a listed Meeting did not open it"; exit 1; }

echo
echo "e2e passed: the Registry names when each voice was captured, where, and when"
echo "it was last heard; both dates and every Meeting behind the count open it"
