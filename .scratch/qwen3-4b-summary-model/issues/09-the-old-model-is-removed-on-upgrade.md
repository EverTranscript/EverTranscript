# 09: The old model is removed on upgrade

**What to build:** The 0.5B that no build can load stops occupying half a gigabyte of the
Operator's disk. Deleted on the upgrade that replaces it, **by exact filename and never a
glob**, so a file of the Operator's own is never swept up.

Application Support is the re-creatable half of the product and models were never part of
the portable unit — the Homebrew cask already deletes that directory on uninstall for the
same reason. History is never touched.

**Blocked by:** 03, 05.

**Status:** done

- [x] Removed by exact filename from a hand-maintained list. A list that must be edited by hand is the point: taking a file off someone's disk should be a deliberate act visible in a diff
- [x] Tested against a registered model and against a file the Operator put there themselves; both survive. There is also a guard that nothing still registered can appear in the superseded list — superseding a model without unregistering it would delete the file the product is about to load
- [x] Removing from an empty directory removes nothing and reports nothing
- [x] Recorded against M5's criterion, three times rather than twice, with its limits stated plainly and an explicit note that it does not make the criterion met. It did shape the rewording
- [x] The local gate is green
