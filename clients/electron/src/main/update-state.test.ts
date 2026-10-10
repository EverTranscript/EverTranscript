import assert from "node:assert/strict";
import { test } from "node:test";

import { nextUpdateState, type UpdateEvent, type UpdateState } from "./update-state.js";

function run(events: UpdateEvent[], from: UpdateState = { phase: "none" }): UpdateState {
  return events.reduce(nextUpdateState, from);
}

test("an offer goes found, download, progress, downloaded, install", () => {
  assert.deepEqual(
    run([
      { type: "found", version: "1.2.0" },
      { type: "downloadStarted" },
      { type: "progress", percent: 41.7 },
    ]),
    { phase: "downloading", version: "1.2.0", percent: 41 },
  );
  assert.deepEqual(
    run([
      { type: "found", version: "1.2.0" },
      { type: "downloadStarted" },
      { type: "downloaded" },
      { type: "installStarted" },
    ]),
    { phase: "installing", version: "1.2.0" },
  );
});

test("a later check does not take a downloaded update back a step", () => {
  const downloaded: UpdateState = { phase: "downloaded", version: "1.2.0" };
  assert.equal(nextUpdateState(downloaded, { type: "found", version: "1.2.0" }), downloaded);
});

test("Later hides the offer until a check finds it again", () => {
  const hidden = run([{ type: "found", version: "1.2.0" }, { type: "dismissed" }]);
  assert.deepEqual(hidden, { phase: "none" });
  assert.deepEqual(nextUpdateState(hidden, { type: "found", version: "1.2.0" }), {
    phase: "available",
    version: "1.2.0",
  });
});

test("a failed download offers Try again, and Try again downloads", () => {
  const failed = run([
    { type: "found", version: "1.2.0" },
    { type: "downloadStarted" },
    { type: "downloadFailed", message: "offline" },
  ]);
  assert.deepEqual(failed, {
    phase: "failed",
    version: "1.2.0",
    retry: "download",
    message: "offline",
  });
  assert.equal(nextUpdateState(failed, { type: "downloadStarted" }).phase, "downloading");
});

test("a failed install keeps the download and retries the install", () => {
  const failed = run(
    [{ type: "installStarted" }, { type: "installFailed", message: "signature" }],
    { phase: "downloaded", version: "1.2.0" },
  );
  assert.equal(failed.phase, "failed");
  assert.equal(nextUpdateState(failed, { type: "found", version: "1.2.0" }), failed);
  assert.equal(nextUpdateState(failed, { type: "downloadStarted" }), failed);
  assert.equal(nextUpdateState(failed, { type: "installStarted" }).phase, "installing");
});

test("events that do not apply leave the state itself, so nothing is pushed", () => {
  const none: UpdateState = { phase: "none" };
  for (const event of [
    { type: "downloadStarted" },
    { type: "progress", percent: 10 },
    { type: "downloaded" },
    { type: "installStarted" },
    { type: "installFailed", message: "x" },
  ] as UpdateEvent[]) {
    assert.equal(nextUpdateState(none, event), none);
  }
  const downloading: UpdateState = { phase: "downloading", version: "1.2.0", percent: 41 };
  assert.equal(nextUpdateState(downloading, { type: "progress", percent: 41.9 }), downloading);
});

test("Restart to update waits while the Core is busy, then installs", () => {
  const downloaded: UpdateState = { phase: "downloaded", version: "1.2.0" };
  const waiting = nextUpdateState(downloaded, { type: "installWaiting", busy: "recording" });
  assert.deepEqual(waiting, { phase: "waiting", version: "1.2.0", busy: "recording" });
  // The same answer again changes nothing, so nothing is pushed.
  assert.equal(nextUpdateState(waiting, { type: "installWaiting", busy: "recording" }), waiting);
  assert.deepEqual(nextUpdateState(waiting, { type: "installWaiting", busy: "diarizing" }), {
    phase: "waiting",
    version: "1.2.0",
    busy: "diarizing",
  });
  assert.deepEqual(nextUpdateState(waiting, { type: "installStarted" }), {
    phase: "installing",
    version: "1.2.0",
  });
  // A check finding the same version does not take a waiting install back.
  assert.equal(nextUpdateState(waiting, { type: "found", version: "1.2.0" }), waiting);
  assert.equal(nextUpdateState({ phase: "none" }, { type: "installWaiting", busy: "recording" }).phase, "none");
});
