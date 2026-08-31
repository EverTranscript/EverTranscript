# 08: The surfaces — Notes pane, Backend picker, prompt editor

**What to build:** Everything M4 adds that an Operator touches, in both the Client and the CLI.

**Blocked by:** 02, 03, 07.

Status: not started

- [ ] A **Notes pane** per Meeting, editable during and after the call (ADR-0018)
- [ ] The **Summary** is displayed, with its action items, and says which Backend produced it and when
- [ ] The **Backend picker**: Local (Recommended) and Cloud, no preselection, the one-time warning on choosing Cloud, preset labels shown with their verification date, and an open custom base-URL field (ADR-0010, ADR-0013)
- [ ] **Strict Mode** is a visible switch with its consequence stated, not a hidden preference
- [ ] The **active-Backend indicator** is visible where Summary is, not buried in Settings
- [ ] The **system prompt is editable with reset-to-default** (story 42), and reset is one act
- [ ] Key entry writes to the credential store and never displays what is stored; clearing a key is available and obvious
- [ ] The CLI carries all of it — notes, summary, backend selection, key management, prompt editing — because the Operator's record stays scriptable (standing story 16)
- [ ] Strings in English and Simplified Chinese, as every UI string since M1
