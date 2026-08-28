/**
 * Copies the ts-rs-generated protocol bindings into the Client.
 *
 * The Rust crate is the single source of truth for wire types; `cargo test`
 * regenerates them and CI fails on drift. Copying rather than path-aliasing
 * keeps every source file the Client compiles under its own root, and makes
 * it impossible to ship a Client built against stale types: this runs before
 * every typecheck, build, and dev server.
 *
 * The destination is generated output and is gitignored.
 */

import { cp, mkdir, rm, readdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const source = join(here, "..", "..", "..", "crates", "evertranscript-protocol", "bindings");
const destination = join(here, "..", "src", "protocol");

try {
  const entries = await readdir(source);
  if (entries.length === 0) throw new Error("no bindings found");
} catch (error) {
  console.error(
    `\nProtocol bindings are missing at ${source}.\n` +
      `Generate them first:  cargo test -p evertranscript-protocol\n`,
  );
  process.exit(1);
}

await rm(destination, { recursive: true, force: true });
await mkdir(destination, { recursive: true });
await cp(source, destination, { recursive: true });
