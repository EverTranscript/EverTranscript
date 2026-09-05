import assert from "node:assert/strict";
import { test } from "node:test";

import { parseSummary, parseSpans } from "./summary-markdown.js";

/** The Summary this machine actually generated, verbatim. */
const REAL = `# Security Audit and Vendor Decision Meeting

**Discussion:** The security audit was completed on Thursday, allowing the vendor decision to be unblocked.

**Action items**
| Who    | What                     | When       | Said at  |
|-------|--------------------------|------------|----------|
| Priya | Owns the migration runbook | Before Tuesday | 00:01 |`;

test("a real Summary parses into the blocks it looks like", () => {
  const blocks = parseSummary(REAL);
  assert.deepEqual(
    blocks.map((block) => block.kind),
    ["heading", "paragraph", "paragraph", "table"],
  );
  assert.equal(blocks[0]?.kind === "heading" && blocks[0].text,
    "Security Audit and Vendor Decision Meeting");
});

test("a table's divider row names the one above it as the header", () => {
  const [table] = parseSummary("| Who | What |\n|-----|------|\n| Priya | Runbook |");
  assert.equal(table?.kind, "table");
  if (table?.kind !== "table") return;
  assert.deepEqual(table.header, ["Who", "What"]);
  assert.deepEqual(table.rows, [["Priya", "Runbook"]]);
});

test("a table with no divider has rows and no header", () => {
  const [table] = parseSummary("| a | b |\n| c | d |");
  assert.equal(table?.kind, "table");
  if (table?.kind !== "table") return;
  assert.equal(table.header, null);
  assert.equal(table.rows.length, 2);
});

test("wrapped prose becomes one paragraph, not one per line", () => {
  const blocks = parseSummary("The audit finished\non Thursday.");
  assert.deepEqual(blocks, [
    { kind: "paragraph", text: "The audit finished on Thursday." },
  ]);
});

test("bullets group, and a blank line does not split a run", () => {
  const [bullets] = parseSummary("- one\n- two");
  assert.deepEqual(bullets, { kind: "bullets", items: ["one", "two"] });
});

test("syntax outside the subset survives as text rather than vanishing", () => {
  // The old behaviour was to show everything verbatim, so anything this does
  // not understand must still reach the reader.
  const blocks = parseSummary("> a quote\n\n~~struck~~");
  assert.deepEqual(
    blocks.map((block) => block.kind === "paragraph" && block.text),
    ["> a quote", "~~struck~~"],
  );
});

test("bold and code are spans; asterisks that are not pairs are text", () => {
  assert.deepEqual(parseSpans("**Discussion:** the `aec` ran"), [
    { kind: "strong", text: "Discussion:" },
    { kind: "text", text: " the " },
    { kind: "code", text: "aec" },
    { kind: "text", text: " ran" },
  ]);
  assert.deepEqual(parseSpans("2 * 3 * 4"), [{ kind: "text", text: "2 * 3 * 4" }]);
});

test("a model can emit anything, and none of it becomes markup", () => {
  // Summary text is written by a language model, so it is untrusted input.
  // The parser must hand back data, never anything a renderer would treat as
  // structure it did not ask for.
  const hostile = '<img src=x onerror="alert(1)">';
  assert.deepEqual(parseSummary(hostile), [{ kind: "paragraph", text: hostile }]);
  assert.deepEqual(parseSpans(hostile), [{ kind: "text", text: hostile }]);
});
