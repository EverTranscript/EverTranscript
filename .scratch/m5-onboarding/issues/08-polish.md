# 08: The polish pass

**What to build:** The accumulated small things four milestones have deferred, gathered rather than scattered.

**Blocked by:** 01, 02.

Status: done

- [x] Both phases already existed from M1 and the tray already renders them; what M5 adds is the Briefing that makes `NotPermitted` reachable on a real first run rather than only in tests
- [x] Transitional tray items are optimistic and revert on error (catalog: "⏹ Stopping…", disabled), so a click always visibly does something
- [x] **Every UI string in both languages, checked mechanically** rather than by reading. Four milestones have added strings and the drift is invisible until a Chinese-locale Operator finds a gap
- [x] The CLI's help text is accurate after four milestones of additions — including the two places a doc comment was attached to the wrong subcommand and found by reading `--help` output
- [x] Checked the four named. "No Summary Backend chosen" now names the fix in the message; the credential store reports plainly and never writes the key elsewhere; `audio-check` records-to-verify rather than trusting the OS's answer about a permission. **Not systematically audited** — four messages read, not every message in the product
- [x] A pass over `CONTEXT.md` for staleness, which M2 already found once — its glossary is normative and has been amended by four milestones of ADRs
