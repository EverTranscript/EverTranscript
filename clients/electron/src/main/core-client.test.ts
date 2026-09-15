import assert from "node:assert/strict";
import { test } from "node:test";

import { pipeNameFor } from "./core-client.js";

test("a named runtime directory finds the pipe the Core actually bound", () => {
  // The same literal `paths::tests` pins on the Rust side. If this fails,
  // the Core's derivation moved and this one has to move with it — not the
  // other way round.
  assert.equal(
    pipeNameFor("/tmp/one", "frank"),
    "\\\\.\\pipe\\evertranscript-frank-1bf83e71b8d8b200",
  );
});

test("no runtime directory is the one global pipe per user", () => {
  assert.equal(pipeNameFor(undefined, "frank"), "\\\\.\\pipe\\evertranscript-frank");
});
