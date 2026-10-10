import assert from "node:assert/strict";
import { test } from "node:test";

import { mkdtempSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { CoreClient, nobodyListening, pipeNameFor } from "./core-client.js";

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

async function connectError(address: string): Promise<unknown> {
  try {
    (await CoreClient.connect(address)).close();
  } catch (error) {
    return error;
  }
  assert.fail(`something answered at ${address}`);
}

test("nothing at the address proves no Core is listening", async () => {
  const address =
    process.platform === "win32"
      ? "\\\\.\\pipe\\evertranscript-test-nobody-here"
      : join(mkdtempSync(join(tmpdir(), "et-")), "evertranscript.sock");
  assert.equal(nobodyListening(await connectError(address)), true);
});

test("a socket a dead Core left behind proves it too", { skip: process.platform === "win32" }, async () => {
  const address = join(mkdtempSync(join(tmpdir(), "et-")), "evertranscript.sock");
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(address, resolve));
  await new Promise<void>((resolve) => server.close(() => resolve()));
  assert.equal(nobodyListening(await connectError(address)), true);
});

test("a Core that fails to answer is not proof that none is running", () => {
  // An update must wait on these, never install (Q330).
  const refused = Object.assign(new Error("permission denied"), { code: "EACCES" });
  assert.equal(nobodyListening(new Error("connect failed", { cause: refused })), false);
  const busy = Object.assign(new Error("all pipe instances are busy"), { code: "EBUSY" });
  assert.equal(nobodyListening(new Error("connect failed", { cause: busy })), false);
  assert.equal(nobodyListening(new Error("the Core did not answer")), false);
  assert.equal(nobodyListening(new Error("the connection to the Core is closed")), false);
  assert.equal(nobodyListening("unknown method: update/hold"), false);
});

test("a Core going away settles `closed`, which is how a restart knows it went", async () => {
  // The catch-up waits on this before starting the new Core (Q12): starting
  // it while the old one still holds the address would reconnect to the old.
  const address =
    process.platform === "win32"
      ? `\\\\.\\pipe\\evertranscript-test-closing-${process.pid}`
      : join(mkdtempSync(join(tmpdir(), "et-")), "evertranscript.sock");
  const server = createServer((socket) => socket.destroy());
  await new Promise<void>((resolve) => server.listen(address, resolve));
  const core = await CoreClient.connect(address);
  await core.closed;
  assert.equal(core.open, false);
  await assert.rejects(core.request("status"), /closed/);
  await new Promise<void>((resolve) => server.close(() => resolve()));
});
