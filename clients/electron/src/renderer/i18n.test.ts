import assert from "node:assert/strict";
import { test } from "node:test";

import { plural } from "./i18n.js";

// "1 meetings" shipped, because the catalog held one string and the call site
// pasted it after a number. The bug is invisible in Chinese, invisible on any
// count but one, and the sort of thing nobody re-reads — so it is pinned here
// rather than left to a Registry screenshot.
test("a counted noun agrees with its count", () => {
  assert.equal(plural("registry.meetings", 1), "meeting");
  assert.equal(plural("registry.meetings", 2), "meetings");
  assert.equal(plural("registry.meetings", 0), "meetings");
});
