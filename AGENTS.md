# EverTranscript

## Reuse before writing

During implementation, always take a look at the two MIT-licensed sibling notetakers before writing new code, and reuse as much of their code as possible — avoid re-inventing wheels:

- **anarlog**: `~/github.com/fastrepl/anarlog` (clone from `https://github.com/fastrepl/anarlog` if absent). **Exclude `enterprise/`** — that directory is commercially licensed, not MIT; everything else is fair game.
- **Meetily**: `~/github.com/Zackriya-Solutions/meetily` (clone from `https://github.com/Zackriya-Solutions/meetily` if absent).

Reuse means port-with-attribution, same discipline as the codex ports (ADR-0028): copied/adapted files keep the upstream copyright notice, and every port is logged in `PORTS.md` with its upstream path and rev. MIT→Apache-2.0 is license-compatible; the notice is the obligation. The codex reference (`~/github.com/openai/codex`, Apache-2.0) stays read-and-port-never-link per ADR-0028's amendment. Distilled competitor findings live in `docs/competitive-facts-2026-08-27.md`.

## Agent skills

### Issue tracker

Issues and specs live as markdown files under `.scratch/<feature-slug>/` in this repo. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, each label string equal to its name, recorded as a `Status:` line in the issue file. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` plus `docs/adr/` at the repo root. See `docs/agents/domain.md`.
