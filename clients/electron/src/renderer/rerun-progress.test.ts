import assert from "node:assert/strict";
import { test } from "node:test";

import type { DiarizeRerun } from "@protocol/DiarizeRerun";

import { rerunLines, rerunMoved } from "./rerun-progress.js";

const backlog = (over: Partial<DiarizeRerun>): DiarizeRerun => ({
  model: "wespeaker",
  modelVersion: "2",
  total: 40,
  done: 12,
  remaining: 28,
  abandoned: 0,
  cancelled: false,
  pausedForRecording: false,
  ...over,
});

test("a re-run standing aside for a recording says it is waiting, not stopped", () => {
  const { state, through } = rerunLines(backlog({ pausedForRecording: true }));
  assert.equal(state, "Waiting while a meeting records. It carries on when the recording ends.");
  assert.equal(through, 12);
});

test("given up is counted apart from gone through", () => {
  const { counts } = rerunLines(backlog({ done: 10, abandoned: 2, remaining: 28 }));
  assert.equal(counts, "10 gone through · 2 given up · 28 still to go");
});

// The bar fills as the line empties, so a stopped re-run's bar is near full.
// Reading that as "finished" is exactly the claim this must not make.
test("a stopped re-run says stopped, not finished", () => {
  const { state } = rerunLines(backlog({ cancelled: true, done: 12, abandoned: 28, remaining: 0 }));
  assert.equal(state, "Update stopped.");
});

// A Meeting promoted out of the backlog is somebody's own request; the bulk
// stop deliberately leaves it in line. Saying "still to go" would make Stop
// look like it had failed to stop it.
test("work left after a stop is named as the separate request it is", () => {
  const { state, counts } = rerunLines(
    backlog({ cancelled: true, done: 12, abandoned: 27, remaining: 1 }),
  );
  assert.equal(state, "Update stopped.");
  assert.equal(
    counts,
    "12 gone through · 27 given up · 1 meeting still queued, because it was asked for on its own. Stopping the update does not cancel that.",
  );
});

test("an emptied line that nobody stopped is finished", () => {
  const { state, counts } = rerunLines(backlog({ done: 40, remaining: 0 }));
  assert.equal(state, "Update finished.");
  assert.equal(counts, "40 gone through");
});

// The common case by a wide margin: no model has ever changed here, so every
// poll for as long as the Registry is open answers the same nothing.
test("an installation with no re-run at all is not moving", () => {
  assert.equal(rerunMoved(null, null), false);
});

test("a re-run appearing, and the same re-run finishing and going, are all moves", () => {
  const running = backlog({});
  assert.equal(rerunMoved(null, running), true);
  assert.equal(rerunMoved(running, backlog({ done: 13, remaining: 27 })), true);
  assert.equal(rerunMoved(running, null), true);
});

// The rows underneath are the reason to refetch, and a pause changes none of
// them: the words above say "waiting" from the answer already in hand.
test("standing aside for a recording is not a move", () => {
  assert.equal(rerunMoved(backlog({}), backlog({ pausedForRecording: true })), false);
});
