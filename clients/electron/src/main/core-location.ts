/**
 * Where the Core binary is.
 *
 * Split out of `index.ts` so it can be *run* by a test rather than reasoned
 * about. DECISIONS Q44 changed this search — it had never consulted the app's
 * own bundle, so a packaged Client could not find the Core it shipped with —
 * and that change went out on reasoning alone, because the Client had no test
 * runner at all. CI's packaging guard proves the binary is *in* the artifact;
 * it says nothing about whether this function would find it. Those are
 * different claims, and confusing them is exactly what Q44 was about.
 *
 * The filesystem is injected, so the order of preference can be tested
 * without staging real binaries in real directories.
 */

import { delimiter, join } from "node:path";

/** Everything the search depends on, passed in rather than read. */
export interface Lookup {
  /** `EVERTRANSCRIPT_BIN`, when the Operator set one. */
  explicit?: string | undefined;
  /** `process.platform` — decides the filename only. */
  platform: string;
  /** `process.resourcesPath`: the bundle's own copy. */
  resourcesPath: string;
  /** The raw `PATH`. */
  searchPath: string;
  /** A checkout's root, for a Client run from source. */
  repoRoot: string;
  /** Whether a candidate exists. Injected for tests. */
  exists(candidate: string): boolean;
}

/** What the Core is called here. */
export function coreName(platform: string): string {
  return platform === "win32" ? "evertranscript.exe" : "evertranscript";
}

/**
 * The first Core this build should use, or `null` if there is none.
 *
 * The order is the decision, and each step earns its place:
 *
 * 1. `EVERTRANSCRIPT_BIN` — an unusual install saying outright, and returned
 *    without an existence check so a wrong value fails loudly at spawn rather
 *    than being silently ignored in favour of some other Core.
 * 2. **The bundle's own copy, ahead of `PATH`.** The Core is replaced
 *    wholesale when the Client updates (ADR-0016), so if `PATH` won, an
 *    Operator who had ever installed a Core by hand would keep launching that
 *    one after every update — the protocol skew ADR-0028 exists to *survive*,
 *    arrived at deliberately instead of by accident. It is also the only
 *    entry a real install populates: neither the macOS zip nor the NSIS
 *    installer puts anything on `PATH`.
 * 3. `PATH`, searched by hand — a GUI app inherits a much smaller `PATH` than
 *    a shell, so a Core in `/opt/homebrew/bin` is invisible to an app
 *    launched from Finder, and searching here is what lets the caller say the
 *    name resolved to nothing.
 * 4. A checkout's `target/`, for a Client run from source before anything is
 *    installed. `release` before `debug`, because a developer who has built
 *    both most recently meant the optimised one.
 */
export function locateCore(lookup: Lookup): string | null {
  if (lookup.explicit) return lookup.explicit;

  const name = coreName(lookup.platform);

  const bundled = join(lookup.resourcesPath, name);
  if (lookup.exists(bundled)) return bundled;

  for (const dir of lookup.searchPath.split(delimiter)) {
    // An empty segment joins to a bare relative name, which would resolve
    // against the working directory — not a place to find a Core.
    if (!dir) continue;
    const candidate = join(dir, name);
    if (lookup.exists(candidate)) return candidate;
  }

  for (const profile of ["release", "debug"]) {
    const candidate = join(lookup.repoRoot, "target", profile, name);
    if (lookup.exists(candidate)) return candidate;
  }

  return null;
}
