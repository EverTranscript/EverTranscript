/**
 * The Core search, exercised.
 *
 * The Client had no test runner before this file. `node --test` is built in,
 * so this adds a place for main-process tests without adding a dependency —
 * and the first thing it tests is the search DECISIONS Q44 changed, which
 * shipped on reasoning alone.
 */

import assert from "node:assert/strict";
import { delimiter, join } from "node:path";
import { test } from "node:test";

import { coreName, locateCore } from "./core-location.js";

/** A lookup where nothing exists unless `present` lists it. */
function lookup(
  present: string[],
  overrides: Partial<Parameters<typeof locateCore>[0]> = {},
) {
  return {
    platform: "darwin",
    resourcesPath: "/Applications/EverTranscript.app/Contents/Resources",
    searchPath: ["/usr/bin", "/opt/homebrew/bin"].join(delimiter),
    repoRoot: "/checkout",
    exists: (candidate: string) => present.includes(candidate),
    ...overrides,
  };
}

const BUNDLED = join(
  "/Applications/EverTranscript.app/Contents/Resources",
  "evertranscript",
);
const ON_PATH = join("/opt/homebrew/bin", "evertranscript");

test("an explicit EVERTRANSCRIPT_BIN wins over everything", () => {
  const found = locateCore(
    lookup([BUNDLED, ON_PATH], { explicit: "/somewhere/else/evertranscript" }),
  );
  assert.equal(found, "/somewhere/else/evertranscript");
});

test("an explicit path is returned without checking that it exists", () => {
  // So a wrong value fails loudly at spawn, naming the path the Operator
  // set, instead of being silently ignored in favour of a different Core.
  const found = locateCore(lookup([], { explicit: "/typo/evertranscript" }));
  assert.equal(found, "/typo/evertranscript");
});

test("the bundle's own Core beats one on PATH", () => {
  // **The Q44 property.** A Client that preferred PATH would keep launching
  // a hand-installed Core after every update, which is ADR-0028's protocol
  // skew reached by accident.
  const found = locateCore(lookup([BUNDLED, ON_PATH]));
  assert.equal(found, BUNDLED);
});

test("PATH is used when the bundle has no Core", () => {
  // The case every install had before Q44: nothing beside the app, so the
  // only hope is a Core someone installed separately.
  const found = locateCore(lookup([ON_PATH]));
  assert.equal(found, ON_PATH);
});

test("PATH is searched in order", () => {
  const first = join("/usr/bin", "evertranscript");
  const found = locateCore(lookup([first, ON_PATH]));
  assert.equal(found, first);
});

test("empty PATH segments are skipped", () => {
  // A trailing delimiter yields "", which would join to a bare relative name
  // and resolve against the working directory.
  const found = locateCore(
    lookup([join("/usr/bin", "evertranscript")], {
      searchPath: `${delimiter}/usr/bin${delimiter}`,
      exists: (candidate: string) => candidate === "evertranscript" || candidate === join("/usr/bin", "evertranscript"),
    }),
  );
  assert.equal(found, join("/usr/bin", "evertranscript"));
});

test("a checkout's build is the last resort, release before debug", () => {
  const release = join("/checkout", "target", "release", "evertranscript");
  const debug = join("/checkout", "target", "debug", "evertranscript");
  assert.equal(locateCore(lookup([release, debug])), release);
  assert.equal(locateCore(lookup([debug])), debug);
});

test("nothing found is null rather than a guess", () => {
  // The caller turns this into a message naming the failure. A fabricated
  // path would surface as a spawn error about a file nobody chose.
  assert.equal(locateCore(lookup([])), null);
});

test("the filename carries .exe only on Windows", () => {
  assert.equal(coreName("win32"), "evertranscript.exe");
  assert.equal(coreName("darwin"), "evertranscript");
  assert.equal(coreName("linux"), "evertranscript");
});

test("the Windows bundle is found under its own name", () => {
  const windowsBundle = join("C:\\Program Files\\EverTranscript\\resources", "evertranscript.exe");
  const found = locateCore(
    lookup([windowsBundle], {
      platform: "win32",
      resourcesPath: "C:\\Program Files\\EverTranscript\\resources",
    }),
  );
  assert.equal(found, windowsBundle);
});
