# 09: The old model is removed on upgrade

**What to build:** The 0.5B that no build can load stops occupying half a gigabyte of the
Operator's disk. Deleted on the upgrade that replaces it, **by exact filename and never a
glob**, so a file of the Operator's own is never swept up.

Application Support is the re-creatable half of the product and models were never part of
the portable unit — the Homebrew cask already deletes that directory on uninstall for the
same reason. History is never touched.

**Blocked by:** 03, 05.

**Status:** ready-for-agent

- [ ] The superseded model file is removed once, on upgrade, matched by exact filename
- [ ] Nothing else in the models directory is touched, and History is untouched
- [ ] An Operator who never had the old model is unaffected
- [ ] The session's Briefing evidence is recorded against M5's criterion with its limits stated — a reader who twice inferred that local models mean no network traffic, who is neither a stranger nor was reading the text at the time. It does not make the criterion met
- [ ] The local gate is green
