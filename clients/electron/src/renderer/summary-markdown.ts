/**
 * The Markdown a Summary is written in, turned into blocks.
 *
 * Split out of `App.tsx` for the reason `core-location.ts` was split out of
 * `index.ts`: so it can be *run* by a test rather than reasoned about. The
 * rendering that used to hold this logic could not be tested at all — the
 * Client's runner only globs `dist-test/main`, and JSX does not compile under
 * the main process's config — so a parser living inside a component was a
 * parser nothing could check.
 *
 * Only what Summaries actually contain. Anything outside the subset falls
 * through as a paragraph, which is how this behaved before it rendered
 * anything, so unknown syntax can never be a regression.
 */

export type Block =
  | { kind: "heading"; text: string }
  | { kind: "paragraph"; text: string }
  | { kind: "bullets"; items: string[] }
  | { kind: "table"; header: string[] | null; rows: string[][] };

/** `**bold**` and `` `code` ``, which is all Summaries use inline. */
export type Span =
  | { kind: "text"; text: string }
  | { kind: "strong"; text: string }
  | { kind: "code"; text: string };

const TABLE_ROW = /^\s*\|.*\|\s*$/;
const HEADING = /^(#{1,4})\s+(.*)$/;
const BULLET = /^\s*[-*]\s+/;

function cells(line: string): string[] {
  return line
    .trim()
    .replace(/^\||\|$/g, "")
    .split("|")
    .map((cell) => cell.trim());
}

/** `|---|:--:|` — the row that says "the one above was a header". */
function isDivider(line: string): boolean {
  return TABLE_ROW.test(line) && cells(line).every((cell) => /^:?-{2,}:?$/.test(cell));
}

export function parseSummary(text: string): Block[] {
  const lines = text.split("\n");
  const blocks: Block[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index] ?? "";

    if (line.trim() === "") {
      index += 1;
      continue;
    }

    if (TABLE_ROW.test(line)) {
      const rows: string[][] = [];
      let header: string[] | null = null;
      while (index < lines.length && TABLE_ROW.test(lines[index] ?? "")) {
        const current = lines[index] ?? "";
        if (isDivider(current)) {
          header = rows.pop() ?? null;
        } else {
          rows.push(cells(current));
        }
        index += 1;
      }
      blocks.push({ kind: "table", header, rows });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      blocks.push({ kind: "heading", text: heading[2] ?? "" });
      index += 1;
      continue;
    }

    if (BULLET.test(line)) {
      const items: string[] = [];
      while (index < lines.length && BULLET.test(lines[index] ?? "")) {
        items.push((lines[index] ?? "").replace(BULLET, ""));
        index += 1;
      }
      blocks.push({ kind: "bullets", items });
      continue;
    }

    const paragraph: string[] = [];
    while (
      index < lines.length &&
      (lines[index] ?? "").trim() !== "" &&
      !TABLE_ROW.test(lines[index] ?? "") &&
      !HEADING.test(lines[index] ?? "") &&
      !BULLET.test(lines[index] ?? "")
    ) {
      paragraph.push(lines[index] ?? "");
      index += 1;
    }
    blocks.push({ kind: "paragraph", text: paragraph.join(" ") });
  }

  return blocks;
}

export function parseSpans(text: string): Span[] {
  return text
    .split(/(\*\*[^*]+\*\*|`[^`]+`)/g)
    .filter((part) => part !== "")
    .map((part): Span => {
      if (part.startsWith("**") && part.endsWith("**") && part.length > 4) {
        return { kind: "strong", text: part.slice(2, -2) };
      }
      if (part.startsWith("`") && part.endsWith("`") && part.length > 2) {
        return { kind: "code", text: part.slice(1, -1) };
      }
      return { kind: "text", text: part };
    });
}
