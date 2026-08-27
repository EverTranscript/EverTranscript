# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

**Layout: single-context.** One `CONTEXT.md` at the repo root and one `docs/adr/` directory. There is no `CONTEXT-MAP.md` and there are no per-context ADR directories.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root: the product and system glossary.
- **`docs/adr/`**: read ADRs that touch the area you're about to work in.

If any of these files don't exist, **proceed silently**. Don't flag their absence; don't suggest creating them upfront. The `/domain-modeling` skill (reached via `/grill-with-docs` and `/improve-codebase-architecture`) creates them lazily when terms or decisions actually get resolved.

## File structure

```
/
├── CONTEXT.md
├── docs/adr/
│   ├── 0001-history-never-reaches-cloud.md
│   └── 0026-rust-core-daemon-clients-over-protocol.md
└── ...
```

If this repo ever splits into multiple bounded contexts, the multi-context layout is a root `CONTEXT-MAP.md` pointing at one `CONTEXT.md` per context, with context-scoped decisions under `src/<context>/docs/adr/`. Re-run `/setup-matt-pocock-skills` and pick multi-context at that point.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal: either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0022 (notetaker-only, no live-assist), but worth reopening because…_
