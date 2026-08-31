# 08: The surfaces — Notes pane, Backend picker, prompt editor

**What to build:** Everything M4 adds that an Operator touches, in both the Client and the CLI.

**Blocked by:** 02, 03, 07.

Status: done

- [x] A Notes pane below the transcript, **saving on a debounce rather than behind a Save button** — Notes are written *during* a meeting, and a button is a thing to forget while listening to somebody. The field adopts the stored value once per Meeting and then leaves itself alone, because re-syncing on every refresh would overwrite what someone is mid-sentence in
- [x] The **Summary** is displayed, with its action items, and says which Backend produced it and when
- [x] The **Backend picker**: Local (Recommended) and Cloud, no preselection, the one-time warning on choosing Cloud, preset labels shown with their verification date, and an open custom base-URL field (ADR-0010, ADR-0013)
- [x] **Strict Mode** is a visible switch with its consequence stated, not a hidden preference
- [x] The **active-Backend indicator** is visible where Summary is, not buried in Settings
- [x] The **system prompt is editable with reset-to-default** (story 42), and reset is one act
- [x] Key entry writes to the store; the field is a password input and the screen can only say *whether* a key exists. The CLI reads a key from **stdin** rather than an argument, so it never reaches shell history or `ps`
- [x] The CLI carries all of it — notes, summary, backend selection, key management, prompt editing — because the Operator's record stays scriptable (standing story 16)
- [x] Strings in English and Simplified Chinese, as every UI string since M1
