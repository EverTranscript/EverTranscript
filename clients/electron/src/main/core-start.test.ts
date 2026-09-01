import assert from "node:assert/strict";
import { test } from "node:test";

import { classifyCoreExit, STARTUP_WINDOW_MS } from "./core-start.js";

const base = { code: null, signal: null, msSinceSpawn: 100, platform: "darwin" };

test("a Core killed by a signal on macOS names quarantine", () => {
  // The measured case: an unsigned Core inside a still-quarantined download
  // is SIGKILLed by Gatekeeper, exit 137, no output.
  const verdict = classifyCoreExit({ ...base, signal: "SIGKILL" });
  assert.equal(verdict.key, "core.start.killedQuarantine");
  assert.equal(verdict.retry, true);
});

test("a Core killed on other platforms does not blame Gatekeeper", () => {
  const verdict = classifyCoreExit({
    ...base,
    signal: "SIGKILL",
    platform: "win32",
  });
  assert.equal(verdict.key, "core.start.killed");
});

test("a clean immediate exit says nothing", () => {
  const verdict = classifyCoreExit({ ...base, code: 0 });
  assert.equal(verdict.key, null);
  assert.equal(verdict.retry, false);
});

test("a non-zero exit is the Core declining to start", () => {
  const verdict = classifyCoreExit({ ...base, code: 2 });
  assert.equal(verdict.key, "core.start.exited");
  assert.equal(verdict.retry, true);
});

test("opening the Client twice exits 1, and that is not shown as an error", () => {
  // Measured, after this file first claimed the Core exits 0 here: a second
  // daemon exits **1** with `another EverTranscript Core is already
  // listening`. The verdict carries a key, and the caller never reads it
  // because the connection to the first Core succeeds. The exit code is not
  // the authority — the socket is.
  const verdict = classifyCoreExit({ ...base, code: 1 });
  assert.equal(verdict.key, "core.start.exited");
  assert.equal(verdict.retry, true);
});

test("an exit long after startup is not a start failure", () => {
  // A Core that ran for an hour and stopped is a different event, but the
  // way must still be clear for a restart.
  const verdict = classifyCoreExit({
    ...base,
    code: 1,
    msSinceSpawn: STARTUP_WINDOW_MS + 1,
  });
  assert.equal(verdict.key, null);
  assert.equal(verdict.retry, true);
});

test("a signal long after startup is also not a start failure", () => {
  const verdict = classifyCoreExit({
    ...base,
    signal: "SIGTERM",
    msSinceSpawn: STARTUP_WINDOW_MS + 1,
  });
  assert.equal(verdict.key, null);
  assert.equal(verdict.retry, true);
});
