# 08: The polish pass

**What to build:** The accumulated small things four milestones have deferred, gathered rather than scattered.

**Blocked by:** 01, 02.

Status: done

- [ ] The tray's **not-ready gate** during onboarding (catalog): the menu says "Downloading model…" rather than offering an action that will be refused. `TrayPhase::NotReady` and `NotPermitted` already exist for this
- [ ] Transitional tray items are optimistic and revert on error (catalog: "⏹ Stopping…", disabled), so a click always visibly does something
- [ ] **Every UI string in both languages, checked mechanically** rather than by reading. Four milestones have added strings and the drift is invisible until a Chinese-locale Operator finds a gap
- [ ] The CLI's help text is accurate after four milestones of additions — including the two places a doc comment was attached to the wrong subcommand and found by reading `--help` output
- [ ] Error messages an Operator can act on, particularly the ones this milestone makes reachable: no model, no Backend chosen, permission refused, credential store unavailable
- [ ] A pass over `CONTEXT.md` for staleness, which M2 already found once — its glossary is normative and has been amended by four milestones of ADRs
