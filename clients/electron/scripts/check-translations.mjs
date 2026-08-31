/**
 * Every UI string exists in every locale.
 *
 * Four milestones added strings, and a gap is invisible from an
 * English-locale machine — the renderer falls back to English silently, so
 * the only person who finds a missing translation is the Operator it was
 * missing for. Mechanical rather than by reading, because "check the
 * catalogs match" is exactly the task nobody performs.
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, "..", "src", "renderer", "i18n.ts"), "utf8");
const lines = source.split("\n");

const KEY = /^\s*"([a-zA-Z0-9._]+)":/;

/** Product names are the same in every language; translating them is worse. */
const NOT_TRANSLATED = new Set(["app.title"]);

const startEn = lines.findIndex((line) => line.startsWith("const en = {"));
const localeStarts = lines
  .map((line, index) => ({ line, index }))
  .filter(({ line }) => /^\s+"[a-z]{2}(-[A-Z]{2})?":\s*\{/.test(line));

if (startEn < 0 || localeStarts.length === 0) {
  console.error("could not find the catalogs in i18n.ts — has its shape changed?");
  process.exit(1);
}

const english = [];
for (let i = startEn; i < localeStarts[0].index; i += 1) {
  const match = KEY.exec(lines[i]);
  if (match) english.push(match[1]);
}

let failed = false;
for (const [position, { line, index }] of localeStarts.entries()) {
  const name = /"([a-z]{2}(-[A-Z]{2})?)"/.exec(line)[1];
  const end =
    position + 1 < localeStarts.length ? localeStarts[position + 1].index : lines.length;

  const translated = new Set();
  for (let i = index; i < end; i += 1) {
    const match = KEY.exec(lines[i]);
    if (match) translated.add(match[1]);
  }

  const missing = english.filter(
    (key) => !translated.has(key) && !NOT_TRANSLATED.has(key),
  );
  const unknown = [...translated].filter((key) => !english.includes(key));

  if (missing.length) {
    failed = true;
    console.error(`${name} is missing ${missing.length} string(s):`);
    for (const key of missing) console.error(`  ${key}`);
  }
  if (unknown.length) {
    // A key that exists only in a translation is dead weight, and usually
    // the residue of an English string that was renamed.
    failed = true;
    console.error(`${name} has ${unknown.length} string(s) English does not:`);
    for (const key of unknown) console.error(`  ${key}`);
  }
}

if (failed) process.exit(1);
console.log(`translations complete: ${english.length} strings, all locales`);
